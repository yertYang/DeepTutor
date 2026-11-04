//! Structured questions and replies shared by the native tool and transport.
use crate::{Error, Result, store::text};
use serde_json::{Value, json};
use std::collections::HashSet;

pub fn definition() -> Value {
    json!({"name":"ask_user","description":"Pause this turn to ask 1-4 questions only when a decision requires user input. Bundle questions in one call. After the reply, continue and complete the original request.","parameters":{"type":"object","properties":{"intro":{"type":"string"},"questions":{"type":"array","minItems":1,"maxItems":4,"items":{"type":"object","properties":{"id":{"type":"string"},"prompt":{"type":"string"},"header":{"type":"string"},"options":{"type":"array","maxItems":8,"items":{"type":"object","properties":{"label":{"type":"string"},"description":{"type":"string"}},"required":["label"],"additionalProperties":false}},"multi_select":{"type":"boolean"}},"required":["prompt"],"additionalProperties":false}}},"required":["questions"],"additionalProperties":false}})
}
fn bounded(value: &Value, key: &str, max: usize, required: bool) -> Result<String> {
    let s = match value.get(key) {
        None | Some(Value::Null) if !required => "",
        Some(Value::String(s)) => s.trim(),
        _ => return Err(Error::invalid(format!("Invalid {key}."))),
    };
    if s.chars().count() > max || (required && s.is_empty()) {
        return Err(Error::invalid(format!("Invalid {key} length.")));
    }
    Ok(s.into())
}
pub fn questions(arguments: &str) -> Result<Value> {
    let input: Value = serde_json::from_str(arguments)
        .map_err(|_| Error::invalid("ask_user requires valid JSON."))?;
    let source = input["questions"]
        .as_array()
        .filter(|v| !v.is_empty() && v.len() <= 4)
        .ok_or_else(|| Error::invalid("ask_user requires 1-4 questions."))?;
    let mut ids = HashSet::new();
    let mut questions = Vec::new();
    for (i, q) in source.iter().enumerate() {
        let mut id = bounded(q, "id", 128, false)?;
        if id.is_empty() {
            id = format!("q{}", i + 1);
        }
        if !ids.insert(id.clone()) {
            return Err(Error::invalid("Question identifiers must be unique."));
        }
        let mut options = Vec::new();
        if let Some(source) = q.get("options").filter(|v| !v.is_null()) {
            let source = source
                .as_array()
                .filter(|v| v.len() <= 8)
                .ok_or_else(|| Error::invalid("Invalid question options."))?;
            for option in source {
                options.push(json!({"label":bounded(option,"label",120,true)?,"description":bounded(option,"description",200,false)?}));
            }
        }
        let multi = q
            .get("multi_select")
            .filter(|v| !v.is_null())
            .map(|v| {
                v.as_bool()
                    .ok_or_else(|| Error::invalid("Invalid multi_select."))
            })
            .transpose()?
            .unwrap_or(false);
        questions.push(json!({"id":id,"prompt":bounded(q,"prompt",800,true)?,"header":bounded(q,"header",16,false)?,"options":options,"multi_select":multi,"allow_free_text":true,"placeholder":null}));
    }
    Ok(json!({"intro":bounded(&input,"intro",400,false)?,"questions":questions}))
}

pub fn reply(command: &Value, question: &Value) -> Result<Value> {
    let text = bounded(command, "text", 16384, false)?;
    let mut answers = Vec::new();
    if let Some(source) = command.get("answers").filter(|v| !v.is_null()) {
        let source = source
            .as_array()
            .filter(|a| !a.is_empty() && a.len() <= 4)
            .ok_or_else(|| Error::invalid("Invalid question answers."))?;
        let mut seen = HashSet::new();
        for answer in source {
            let id = bounded(answer, "questionId", 128, true)?;
            if !seen.insert(id.clone())
                || !question["questions"]
                    .as_array()
                    .is_some_and(|q| q.iter().any(|q| q["id"] == id))
            {
                return Err(Error::invalid(
                    "Answer references an unknown or repeated question.",
                ));
            }
            answers.push(json!({"questionId":id,"text":bounded(answer,"text",16384,true)?}));
        }
        if answers.len() != question["questions"].as_array().map_or(0, Vec::len) {
            return Err(Error::invalid("Answer every question in the batch."));
        }
    }
    if text.is_empty() && answers.is_empty() {
        return Err(Error::invalid("Reply requires text or answers."));
    }
    Ok(json!({"success":true,"content":{"text":text,"answers":answers}}))
}

pub fn command_identity(command: &Value) -> Result<()> {
    for key in ["command_id", "turn_id", "tool_call_id"] {
        let s = text(command, key);
        if s.trim().is_empty() || s.len() > 256 {
            return Err(Error::invalid(format!("Reply requires {key}.")));
        }
    }
    Ok(())
}
