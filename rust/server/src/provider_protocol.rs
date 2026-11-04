//! Bounded assembly of one completed provider response. Tool calls cannot leave
//! this collector before the protocol's successful terminal marker.
use crate::{
    Error, Result,
    provider::Delta,
    store::text,
    tools::{MAX_ARGUMENT_BYTES, MAX_CALLS, ToolCall},
};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashSet};

#[derive(Clone, Copy, PartialEq)]
pub enum Wire {
    Chat,
    Responses,
    Anthropic,
}

#[derive(Default)]
pub struct Round {
    pub calls: Vec<ToolCall>,
    pub continuation: Vec<Value>,
    pub has_text: bool,
}

pub struct Collector {
    wire: Wire,
    calls: BTreeMap<usize, ToolCall>,
    blocks: BTreeMap<usize, Value>,
    content: String,
    reasoning: String,
    usage: Value,
    pub finished: bool,
    stop: String,
}
impl Collector {
    pub fn new(wire: Wire) -> Self {
        Self {
            wire,
            calls: BTreeMap::new(),
            blocks: BTreeMap::new(),
            content: String::new(),
            reasoning: String::new(),
            usage: json!({}),
            finished: false,
            stop: String::new(),
        }
    }
    fn index(event: &Value, key: &str) -> Result<usize> {
        event[key]
            .as_u64()
            .filter(|n| *n < 64)
            .map(|n| n as usize)
            .ok_or_else(|| Error::provider("Invalid tool stream index."))
    }
    fn call(&mut self, index: usize) -> Result<&mut ToolCall> {
        if !self.calls.contains_key(&index) && self.calls.len() >= MAX_CALLS {
            return Err(Error::provider("Too many model tool calls."));
        }
        Ok(self.calls.entry(index).or_insert_with(|| ToolCall {
            id: String::new(),
            name: String::new(),
            arguments: String::new(),
        }))
    }
    fn arguments(&mut self, index: usize, delta: &str) -> Result<()> {
        let call = self.call(index)?;
        if call.arguments.len() + delta.len() > MAX_ARGUMENT_BYTES {
            return Err(Error::provider("Tool arguments exceed size limit."));
        }
        call.arguments.push_str(delta);
        Ok(())
    }
    pub fn push(&mut self, event: &Value) -> Result<Vec<Delta>> {
        let deltas = match self.wire {
            Wire::Chat => self.chat(event)?,
            Wire::Responses => self.responses(event)?,
            Wire::Anthropic => self.anthropic(event)?,
        };
        if self.content.len() + self.reasoning.len() > 16 * 1024 * 1024
            || self
                .blocks
                .values()
                .map(|v| v.to_string().len())
                .sum::<usize>()
                > 16 * 1024 * 1024
        {
            return Err(Error::provider("Model round exceeds size limit."));
        }
        Ok(deltas)
    }
    fn chat(&mut self, event: &Value) -> Result<Vec<Delta>> {
        if event.get("error").is_some() {
            return Err(Error::provider("Model reported an error."));
        }
        let mut deltas = Vec::new();
        if !event["usage"].is_null() {
            self.usage = event["usage"].clone();
        }
        let Some(choice) = event["choices"].as_array().and_then(|v| v.first()) else {
            return Ok(deltas);
        };
        if self.finished {
            return Err(Error::provider(
                "Model sent content after its finish marker.",
            ));
        }
        let delta = &choice["delta"];
        if delta.get("function_call").is_some() {
            return Err(Error::unsupported(
                "Legacy function_call output is unsupported.",
            ));
        }
        if let Some(calls) = delta["tool_calls"].as_array() {
            for value in calls {
                let index = Self::index(value, "index")?;
                let call = self.call(index)?;
                call.id.push_str(text(value, "id"));
                call.name.push_str(text(&value["function"], "name"));
                if call.id.len() > 256 || call.name.len() > 128 {
                    return Err(Error::provider("Invalid tool identifier."));
                }
                self.arguments(index, text(&value["function"], "arguments"))?;
            }
        }
        for key in ["content", "refusal"] {
            if let Some(value) = delta[key].as_str() {
                self.content.push_str(value);
                deltas.push(Delta::Content(value.into()));
            }
        }
        if let Some(value) = delta["reasoning_content"].as_str() {
            self.reasoning.push_str(value);
            deltas.push(Delta::Thinking(value.into()));
        }
        if let Some(reason) = choice["finish_reason"].as_str() {
            if !matches!(reason, "stop" | "tool_calls")
                || (reason == "tool_calls") == self.calls.is_empty()
            {
                return Err(Error::provider("Model did not complete the response."));
            }
            self.finished = true;
        }
        Ok(deltas)
    }
    fn response_item(&mut self, index: usize, item: &Value, done: bool) -> Result<()> {
        if !matches!(
            text(item, "type"),
            "function_call" | "message" | "reasoning"
        ) {
            return Err(Error::unsupported("Model requested unsupported output."));
        }
        if item["type"] == "function_call" {
            let call = self.call(index)?;
            for (old, new) in [
                (&mut call.id, text(item, "call_id")),
                (&mut call.name, text(item, "name")),
            ] {
                if !old.is_empty() && !new.is_empty() && old != new {
                    return Err(Error::provider("Tool identity changed in stream."));
                }
                if !new.is_empty() {
                    *old = new.into();
                }
            }
            let args = text(item, "arguments");
            if args.len() > MAX_ARGUMENT_BYTES {
                return Err(Error::provider("Tool arguments exceed size limit."));
            }
            if done && !call.arguments.is_empty() && call.arguments != args {
                return Err(Error::provider("Tool arguments changed in terminal item."));
            }
            if !args.is_empty() {
                call.arguments = args.into();
            }
        }
        self.blocks.insert(index, item.clone());
        Ok(())
    }
    fn responses(&mut self, event: &Value) -> Result<Vec<Delta>> {
        let mut deltas = Vec::new();
        match text(event, "type") {
            "response.output_text.delta" | "response.refusal.delta" => {
                let s = text(event, "delta");
                self.content.push_str(s);
                deltas.push(Delta::Content(s.into()));
            }
            "response.reasoning_summary_text.delta" | "response.reasoning_text.delta" => {
                let s = text(event, "delta");
                self.reasoning.push_str(s);
                deltas.push(Delta::Thinking(s.into()));
            }
            "response.output_item.added" | "response.output_item.done" => {
                let i = Self::index(event, "output_index")?;
                self.response_item(
                    i,
                    &event["item"],
                    event["type"] == "response.output_item.done",
                )?;
            }
            "response.function_call_arguments.delta" => {
                let i = Self::index(event, "output_index")?;
                self.arguments(i, text(event, "delta"))?;
            }
            "response.function_call_arguments.done" => {
                let i = Self::index(event, "output_index")?;
                let call = self.call(i)?;
                if !call.arguments.is_empty() && call.arguments != text(event, "arguments") {
                    return Err(Error::provider("Tool arguments changed in stream."));
                }
                if text(event, "arguments").len() > MAX_ARGUMENT_BYTES {
                    return Err(Error::provider("Tool arguments exceed size limit."));
                }
                call.arguments = text(event, "arguments").into();
            }
            "response.completed" => {
                if event["response"]["status"]
                    .as_str()
                    .is_some_and(|s| s != "completed")
                {
                    return Err(Error::provider("Model did not complete the response."));
                }
                if let Some(items) = event["response"]["output"].as_array() {
                    if items.len() > 64 {
                        return Err(Error::provider("Too many model output items."));
                    }
                    for (i, item) in items.iter().enumerate() {
                        self.response_item(i, item, true)?;
                    }
                    if self
                        .calls
                        .keys()
                        .any(|i| items.get(*i).is_none_or(|v| v["type"] != "function_call"))
                    {
                        return Err(Error::provider(
                            "Terminal output omitted a streamed tool call.",
                        ));
                    }
                }
                self.finished = true;
                deltas.push(Delta::Usage(event["response"]["usage"].clone()));
            }
            "error" | "response.failed" | "response.incomplete" | "response.cancelled" => {
                return Err(Error::provider("Model did not complete the response."));
            }
            _ => {}
        }
        Ok(deltas)
    }
    fn anthropic(&mut self, event: &Value) -> Result<Vec<Delta>> {
        let mut deltas = Vec::new();
        match text(event, "type") {
            "message_start" => self.usage = event["message"]["usage"].clone(),
            "content_block_start" => {
                let i = Self::index(event, "index")?;
                let block = &event["content_block"];
                if self.blocks.contains_key(&i)
                    || !matches!(
                        text(block, "type"),
                        "text" | "thinking" | "redacted_thinking" | "tool_use"
                    )
                {
                    return Err(Error::provider("Invalid Anthropic block."));
                }
                if block["type"] == "tool_use" {
                    let call = self.call(i)?;
                    call.id = text(block, "id").into();
                    call.name = text(block, "name").into();
                }
                self.blocks.insert(i, block.clone());
            }
            "content_block_delta" => {
                let i = Self::index(event, "index")?;
                let block = self
                    .blocks
                    .get_mut(&i)
                    .ok_or_else(|| Error::provider("Missing Anthropic block."))?;
                let (kind, key) = match text(&event["delta"], "type") {
                    "text_delta" => ("text", "text"),
                    "thinking_delta" => ("thinking", "thinking"),
                    "signature_delta" => ("thinking", "signature"),
                    "input_json_delta" => ("tool_use", "partial_json"),
                    _ => return Err(Error::provider("Unknown Anthropic block delta.")),
                };
                if block["type"] != kind {
                    return Err(Error::provider("Anthropic block type changed."));
                }
                let value = text(&event["delta"], key);
                if kind == "tool_use" {
                    self.arguments(i, value)?;
                } else {
                    block[key] = json!(format!("{}{}", text(block, key), value));
                    if key == "text" {
                        deltas.push(Delta::Content(value.into()));
                    }
                    if key == "thinking" {
                        deltas.push(Delta::Thinking(value.into()));
                    }
                }
            }
            "message_delta" => {
                if let Some(usage) = event["usage"].as_object() {
                    if !self.usage.is_object() {
                        self.usage = json!({});
                    }
                    self.usage.as_object_mut().unwrap().extend(usage.clone());
                }
                self.stop = text(&event["delta"], "stop_reason").into();
            }
            "message_stop" => {
                if !matches!(
                    self.stop.as_str(),
                    "end_turn" | "stop_sequence" | "tool_use"
                ) || (self.stop == "tool_use") == self.calls.is_empty()
                {
                    return Err(Error::provider("Anthropic did not complete the response."));
                }
                self.finished = true;
                deltas.push(Delta::Usage(self.usage.clone()));
            }
            "error" => return Err(Error::provider("Anthropic reported an error.")),
            _ => {}
        }
        Ok(deltas)
    }
    pub fn chat_usage(&self) -> Value {
        self.usage.clone()
    }
    pub fn finish(mut self) -> Result<Round> {
        if !self.finished {
            return Err(Error::provider(
                "Model stream ended before its terminal marker.",
            ));
        }
        if self.wire == Wire::Anthropic {
            for (i, call) in &mut self.calls {
                let block = self
                    .blocks
                    .get_mut(i)
                    .ok_or_else(|| Error::provider("Missing tool block."))?;
                if call.arguments.is_empty() {
                    call.arguments = block["input"].to_string();
                } else {
                    block["input"] = serde_json::from_str(&call.arguments)
                        .map_err(|_| Error::provider("Invalid Anthropic tool JSON."))?;
                }
            }
        }
        if self.wire == Wire::Responses {
            for (i, call) in &self.calls {
                let block = self
                    .blocks
                    .get_mut(i)
                    .filter(|b| b["type"] == "function_call")
                    .ok_or_else(|| Error::provider("Missing streamed function call item."))?;
                block["arguments"] = json!(call.arguments);
            }
        }
        let mut ids = HashSet::new();
        for call in self.calls.values() {
            call.validate()?;
            if !ids.insert(&call.id) {
                return Err(Error::provider("Duplicate model tool identifier."));
            }
        }
        let calls: Vec<_> = self.calls.into_values().collect();
        let has_text = !self.content.trim().is_empty()
            || self.blocks.values().any(|block| {
                (block["type"] == "text" && !text(block, "text").trim().is_empty())
                    || (block["type"] == "message"
                        && block["content"].as_array().is_some_and(|parts| {
                            parts.iter().any(|part| {
                                !text(part, "text").trim().is_empty()
                                    || !text(part, "refusal").trim().is_empty()
                            })
                        }))
            });
        let continuation = match self.wire {
            Wire::Chat => {
                let tool_calls=calls.iter().map(|call|json!({"id":call.id,"type":"function","function":{"name":call.name,"arguments":call.arguments}})).collect::<Vec<_>>();
                let mut message =
                    json!({"role":"assistant","content":self.content,"tool_calls":tool_calls});
                if !self.reasoning.is_empty() {
                    message["reasoning_content"] = json!(self.reasoning);
                }
                vec![message]
            }
            Wire::Responses => self.blocks.into_values().collect(),
            Wire::Anthropic => vec![
                json!({"role":"assistant","content":self.blocks.into_values().collect::<Vec<_>>()}),
            ],
        };
        Ok(Round {
            has_text,
            calls,
            continuation,
        })
    }
}
