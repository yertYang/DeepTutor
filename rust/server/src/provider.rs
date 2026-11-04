//! Provider calls use native HTTP; no SDK subprocess or Python bridge.
use crate::{
    Error, Result,
    capability::Capability,
    config::{Model, Settings, http_client},
    provider_protocol::{Collector, Round, Wire},
    store::text,
    tools::ToolCall,
};
use futures_util::StreamExt;
use serde_json::{Value, json};
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot};

#[derive(Debug)]
pub enum Delta {
    Content(String),
    Thinking(String),
    Usage(Value),
    RequestStatus {
        request_id: String,
        trace_id: String,
        http_status: Option<u16>,
    },
    Pause(Box<crate::turn_engine::Pause>),
    ToolStart {
        invocation: String,
        call: ToolCall,
        ack: oneshot::Sender<Result<()>>,
    },
    ToolEnd {
        invocation: String,
        result: crate::tools::ToolOutcome,
        ack: oneshot::Sender<Result<Value>>,
    },
}
/// Canonical token fields used by the existing Web footer and SDK consumers.
/// Missing usage stays unknown; no price or token estimates are fabricated.
pub fn usage_summary(raw: Value) -> Value {
    let input = raw["prompt_tokens"]
        .as_u64()
        .or_else(|| raw["input_tokens"].as_u64());
    let output = raw["completion_tokens"]
        .as_u64()
        .or_else(|| raw["output_tokens"].as_u64());
    if input.is_none() && output.is_none() {
        return json!({});
    }
    let cached = [
        &raw["cache_read_input_tokens"],
        &raw["cached_tokens"],
        &raw["prompt_cache_hit_tokens"],
        &raw["cached_content_token_count"],
        &raw["prompt_tokens_details"]["cached_tokens"],
        &raw["input_tokens_details"]["cached_tokens"],
    ]
    .into_iter()
    .find_map(Value::as_u64);
    let created = raw["cache_creation_input_tokens"].as_u64();
    let mut input = input.unwrap_or(0);
    if raw["prompt_tokens"].is_null()
        && (raw.get("cache_read_input_tokens").is_some() || created.is_some())
    {
        input = input
            .saturating_add(cached.unwrap_or(0))
            .saturating_add(created.unwrap_or(0));
    }
    let output = output.unwrap_or(0);
    let mut summary = json!({
        "prompt_tokens":input,"completion_tokens":output,"total_tokens":input.saturating_add(output),
        "total_calls":1,"estimated_calls":0,"total_cost_usd":null,
        "cache_reported_calls":u32::from(cached.is_some()),
        "cache_input_tokens":if cached.is_some(){input}else{0},
        "cache_hit_rate":cached.filter(|_|input>0).map(|n|n.min(input) as f64 / input as f64),
        "usage":raw,
    });
    if let Some(n) = cached {
        summary["cache_read_input_tokens"] = json!(n.min(input));
    }
    if let Some(n) = created {
        summary["cache_creation_input_tokens"] = json!(n.min(input));
    }
    if let Some(n) = raw["reasoning_tokens"]
        .as_u64()
        .or_else(|| raw["completion_tokens_details"]["reasoning_tokens"].as_u64())
        .or_else(|| raw["output_tokens_details"]["reasoning_tokens"].as_u64())
    {
        summary["reasoning_tokens"] = json!(n);
    }
    summary
}
pub(crate) fn request_body_scoped(
    model: &Model,
    mut history: Vec<Value>,
    session: &str,
    instructions: &str,
    capability: Capability,
    forced: Option<&str>,
    flags: &crate::learning::ToolFlags,
) -> Result<Value> {
    if model.binding == "openai_codex" {
        let input=history.into_iter().map(|m| {
            if m.get("role").is_none() { m } else { json!({"role":m["role"],"content":[{"type":if m["role"]=="assistant"{"output_text"}else{"input_text"},"text":m["content"]}]}) }
        }).collect::<Vec<_>>();
        let mut body = json!({"model":model.model,"stream":true,"store":false,"instructions":instructions,"input":input,"prompt_cache_key":session});
        if let Some(effort) = &model.effort {
            body["reasoning"] = json!({"effort":effort});
        }
        attach_tools_scoped(model, &mut body, capability, forced, flags);
        return Ok(body);
    }
    history.insert(0, json!({"role":"system","content":instructions}));
    if model.format == "anthropic" {
        return anthropic_body(model, history, capability, forced, flags);
    }
    let responses = matches!(model.format.as_str(), "responses" | "openai_responses");
    let mut body = if responses {
        json!({"model":model.model,"stream":true,"store":false,"input":history})
    } else {
        json!({"model":model.model,"stream":true,"messages":history,"stream_options":{"include_usage":true}})
    };
    if let Some(effort) = &model.effort {
        if responses {
            body["reasoning"] = json!({"effort":effort})
        } else if model.binding == "deepseek"
            || (model.binding == "custom" && model.model.to_lowercase().contains("deepseek"))
        {
            body["thinking"] = json!({"type":if matches!(effort.as_str(),"none"|"minimal"){"disabled"}else{"enabled"}});
        } else if model.binding == "dashscope"
            || (model.binding == "custom" && model.model.to_lowercase().contains("qwen"))
        {
            body["enable_thinking"] = json!(!matches!(effort.as_str(), "none" | "minimal"));
        } else {
            body["reasoning_effort"] = json!(effort)
        }
    }
    {
        let max = crate::context::output_reserve(model)?;
        body[if responses {
            "max_output_tokens"
        } else {
            "max_completion_tokens"
        }] = json!(max);
    }
    if let Some(temperature) = model.temperature {
        body["temperature"] = json!(temperature);
    }
    attach_tools_scoped(model, &mut body, capability, forced, flags);
    Ok(body)
}

fn anthropic_body(
    model: &Model,
    history: Vec<Value>,
    capability: Capability,
    forced: Option<&str>,
    flags: &crate::learning::ToolFlags,
) -> Result<Value> {
    let mut messages = Vec::new();
    let mut systems = Vec::new();
    for item in history {
        if item["role"] == "system" || item["role"] == "developer" {
            systems.push(text(&item, "content").to_string())
        } else {
            messages.push(item)
        }
    }
    let mut body = json!({"model":model.model,"stream":true,"max_tokens":crate::context::output_reserve(model)?,"messages":messages});
    if !systems.is_empty() {
        body["system"] = json!(systems.join("\n"));
    }
    if let Some(temperature) = model.temperature {
        body["temperature"] = json!(temperature);
    }
    if let Some(effort) = model.effort.as_deref() {
        match effort {
            "none" => {
                body["thinking"] = json!({"type":"disabled"});
            }
            "adaptive" => {
                body["thinking"] = json!({"type":"adaptive"});
            }
            "low" | "medium" | "high" => {
                let budget = match effort {
                    "low" => 1024,
                    "medium" => 4096,
                    _ => 8192,
                };
                body["thinking"] = json!({"type":"enabled","budget_tokens":budget});
                body.as_object_mut().unwrap().remove("temperature");
            }
            _ => return Err(Error::invalid("Unsupported Anthropic reasoning effort.")),
        }
    }
    attach_tools_scoped(model, &mut body, capability, forced, flags);
    Ok(body)
}

pub(crate) async fn round(
    settings: Arc<Settings>,
    model: &Model,
    body: Value,
    output: mpsc::Sender<Delta>,
    suppress_text: bool,
    context: Option<&crate::codex_session::Context>,
) -> Result<Round> {
    // Waiting-input turns and multi-round tool calls may outlive a grant edit.
    // Recheck the selected model before every new upstream request as well as
    // at admission, so a resumed turn cannot keep using a revoked model.
    if settings.shared.is_some() {
        settings.model(&json!({"profile_id":model.profile_id,"model_id":model.model_id}))?;
    }
    if model.binding == "openai_codex" {
        let credential_settings = if model.profile_id.starts_with("assigned:") {
            settings.shared.as_ref().unwrap_or(&settings).clone()
        } else {
            settings.clone()
        };
        let credentials = crate::oauth::credentials(credential_settings.clone()).await?;
        if let Some(binding) = &model.account_binding {
            use sha2::{Digest, Sha256};
            if *binding
                != format!(
                    "{:x}",
                    Sha256::digest(text(&credentials, "account_id").as_bytes())
                )
            {
                return Err(Error::conflict(
                    "Codex account changed. Refresh the model catalog before using this profile.",
                ));
            }
        }
        let installation =
            crate::codex_session::installation(credential_settings, &credentials).await?;
        let prepared = settings.codex.prepare(
            text(&credentials, "account_id"),
            credentials["generation"].as_u64().unwrap(),
            installation,
            &model.model,
            context,
        );
        let request_id = prepared.identity.request_id.clone();
        let trace_id = prepared.identity.traceparent.as_ref().unwrap()[3..35].to_owned();
        let mut request = deeptutor_codex::Request {
            access_token: text(&credentials, "access_token").into(),
            account_id: text(&credentials, "account_id").into(),
            body,
            profile: Default::default(),
            identity: prepared.identity,
        };
        request
            .apply_identity()
            .map_err(|_| Error::provider("Invalid Codex request identity."))?;
        crate::turn_engine::validate_body(model, &request.body)?;
        send(
            &output,
            Delta::RequestStatus {
                request_id: request_id.clone(),
                trace_id: trace_id.clone(),
                http_status: None,
            },
        )
        .await?;
        let collector = std::sync::Mutex::new(Collector::new(Wire::Responses));
        let mut http_status = None;
        let result = deeptutor_codex::CodexClient::new(true)
            .map_err(|_| Error::provider("Codex transport could not start."))?
            .stream_with_metadata(
                request,
                |metadata| {
                    http_status = Some(metadata.status);
                    settings
                        .codex
                        .remember(prepared.scope.as_deref(), metadata.turn_state.as_deref());
                },
                |event| {
                    let deltas = collector
                        .lock()
                        .map_err(|_| deeptutor_codex::Error::UpstreamEvent)
                        .and_then(|mut c| {
                            c.push(&event)
                                .map_err(|_| deeptutor_codex::Error::UpstreamEvent)
                        });
                    let tx = output.clone();
                    async move {
                        for delta in deltas? {
                            if suppress_text && matches!(delta, Delta::Content(_)) {
                                continue;
                            }
                            tx.send(delta)
                                .await
                                .map_err(|_| deeptutor_codex::Error::OutputClosed)?;
                        }
                        Ok(())
                    }
                },
            )
            .await;
        send(
            &output,
            Delta::RequestStatus {
                request_id: request_id.clone(),
                trace_id,
                http_status,
            },
        )
        .await?;
        result.map_err(|e| Error::provider(format!("{e} Request ID: {request_id}.")))?;
        return collector
            .into_inner()
            .map_err(|_| {
                Error::provider(format!(
                    "Codex response assembly failed. Request ID: {request_id}."
                ))
            })?
            .finish()
            .map_err(|error| Error::provider(format!("{error} Request ID: {request_id}.")));
    }
    if model.format == "anthropic" {
        return anthropic(model, body, output, suppress_text).await;
    }
    let responses = matches!(model.format.as_str(), "responses" | "openai_responses");
    let suffix = if responses {
        "responses"
    } else {
        "chat/completions"
    };
    let endpoint = if model.base_url.ends_with(&format!("/{suffix}")) {
        model.base_url.clone()
    } else {
        format!("{}/{suffix}", model.base_url)
    };
    let mut request = http_client()?.post(endpoint).json(&body);
    if !model.api_key.is_empty() {
        request = request.bearer_auth(&model.api_key);
    }
    if let Some(headers) = model.extra_headers.as_object() {
        for (k, v) in headers {
            request = request.header(
                k,
                v.as_str()
                    .ok_or_else(|| Error::invalid("Header values must be strings."))?,
            );
        }
    }
    let response = request
        .send()
        .await
        .map_err(|_| Error::provider("Model connection failed."))?;
    if !response.status().is_success() {
        return Err(Error::provider(format!(
            "Model returned HTTP {}.",
            response.status().as_u16()
        )));
    }
    if !response
        .headers()
        .get("content-type")
        .and_then(|h| h.to_str().ok())
        .is_some_and(|s| {
            s.split(';')
                .next()
                .unwrap_or("")
                .trim()
                .eq_ignore_ascii_case("text/event-stream")
        })
    {
        return Err(Error::provider("Model did not return an event stream."));
    }
    consume(
        response,
        if responses {
            Wire::Responses
        } else {
            Wire::Chat
        },
        output,
        suppress_text,
    )
    .await
}

async fn consume(
    response: reqwest::Response,
    wire: Wire,
    output: mpsc::Sender<Delta>,
    suppress_text: bool,
) -> Result<Round> {
    let mut bytes = response.bytes_stream();
    let mut decoder = Decoder::default();
    let mut collector = Collector::new(wire);
    while let Some(chunk) = bytes.next().await {
        for event in
            decoder.push(&chunk.map_err(|_| Error::provider("Model connection interrupted."))?)?
        {
            match event {
                None => {
                    if wire == Wire::Chat && collector.finished {
                        send(&output, Delta::Usage(collector.chat_usage())).await?;
                    }
                    return collector.finish();
                }
                Some(event) => {
                    for delta in collector.push(&event)? {
                        if suppress_text && matches!(delta, Delta::Content(_)) {
                            continue;
                        }
                        send(&output, delta).await?;
                    }
                    if wire != Wire::Chat && collector.finished {
                        return collector.finish();
                    }
                }
            }
        }
    }
    Err(Error::provider(
        "Model stream ended before its terminal marker.",
    ))
}
async fn send(tx: &mpsc::Sender<Delta>, delta: Delta) -> Result<()> {
    tx.send(delta)
        .await
        .map_err(|_| Error::conflict("Turn consumer stopped."))
}

/// Generic SSE decoder: unlike the Codex-specific decoder, Chat Completions
/// objects have no `type` field and their [DONE] marker must remain observable.
#[derive(Default)]
pub struct Decoder {
    framer: deeptutor_codex::sse::Framer,
}
impl Decoder {
    pub fn push(&mut self, bytes: &[u8]) -> Result<Vec<Option<Value>>> {
        self.framer
            .push(bytes)
            .map_err(|_| Error::provider("Model event exceeds size limit."))?
            .into_iter()
            .map(|data| {
                let data = std::str::from_utf8(&data)
                    .map_err(|_| Error::provider("Invalid stream encoding."))?
                    .trim();
                if data == "[DONE]" {
                    return Ok(None);
                }
                let value: Value = serde_json::from_str(data)
                    .map_err(|_| Error::provider("Invalid stream JSON."))?;
                if !value.is_object() {
                    return Err(Error::provider("Invalid stream event."));
                }
                Ok(Some(value))
            })
            .collect()
    }
}

async fn anthropic(
    model: &Model,
    body: Value,
    output: mpsc::Sender<Delta>,
    suppress_text: bool,
) -> Result<Round> {
    let url = if model.base_url.ends_with("/messages") {
        model.base_url.clone()
    } else {
        format!("{}/messages", model.base_url)
    };
    let mut request = http_client()?
        .post(url)
        .header("x-api-key", &model.api_key)
        .header("anthropic-version", "2023-06-01")
        .json(&body);
    if let Some(headers) = model.extra_headers.as_object() {
        for (k, v) in headers {
            request = request.header(
                k,
                v.as_str()
                    .ok_or_else(|| Error::invalid("Header values must be strings."))?,
            );
        }
    }
    let response = request
        .send()
        .await
        .map_err(|_| Error::provider("Anthropic connection failed."))?;
    if !response.status().is_success() {
        return Err(Error::provider(format!(
            "Anthropic returned HTTP {}.",
            response.status().as_u16()
        )));
    }
    if !response
        .headers()
        .get("content-type")
        .and_then(|h| h.to_str().ok())
        .is_some_and(|s| s.starts_with("text/event-stream"))
    {
        return Err(Error::provider("Anthropic did not return an event stream."));
    }
    consume(response, Wire::Anthropic, output, suppress_text).await
}

fn attach_tools_scoped(
    model: &Model,
    body: &mut Value,
    capability: Capability,
    forced: Option<&str>,
    flags: &crate::learning::ToolFlags,
) {
    let responses = model.binding == "openai_codex"
        || matches!(model.format.as_str(), "responses" | "openai_responses");
    if responses {
        // Stateless reasoning continuations need the provider's opaque payload.
        body["include"] = json!(["reasoning.encrypted_content"]);
    }
    let tools = crate::tools::definitions_scoped(capability, flags).into_iter().map(|definition| {
        if model.format == "anthropic" && model.binding != "openai_codex" {
            json!({"name":definition["name"],"description":definition["description"],"input_schema":definition["parameters"]})
        } else if responses {
            let mut tool = definition;
            tool["type"] = json!("function");
            tool
        } else {
            json!({"type":"function","function":definition})
        }
    }).collect::<Vec<_>>();
    body["tools"] = json!(tools);
    if let Some(name) = forced {
        if model.format == "anthropic" && model.binding != "openai_codex" {
            if matches!(
                body["thinking"]["type"].as_str(),
                Some("enabled" | "adaptive")
            ) {
                body["tool_choice"] = json!({"type":"auto"});
            } else {
                body["tool_choice"] = json!({"type":"tool","name":name});
            }
        } else if responses {
            body["tool_choice"] = json!({"type":"function","name":name});
        } else {
            body["tool_choice"] = json!({"type":"function","function":{"name":name}});
        }
    }
}

fn tool_output(result: &Value) -> String {
    // Trace metadata stays durable without being repeated in every model input.
    let mut output = json!({"success":result["success"],"content":result["content"]});
    if let Some(error) = result.get("error") {
        output["error"] = error.clone();
    }
    output.to_string()
}

pub(crate) fn append_results(
    model: &Model,
    history: &mut Vec<Value>,
    results: Vec<(ToolCall, Value)>,
) {
    if model.binding == "openai_codex"
        || matches!(model.format.as_str(), "responses" | "openai_responses")
    {
        history.extend(results.into_iter().map(|(call,result)|json!({"type":"function_call_output","call_id":call.id,"output":tool_output(&result)})));
    } else if model.format == "anthropic" {
        history.push(json!({"role":"user","content":results.into_iter().map(|(call,result)|json!({"type":"tool_result","tool_use_id":call.id,"content":tool_output(&result),"is_error":result["success"]!=true})).collect::<Vec<_>>()}));
    } else {
        history.extend(results.into_iter().map(|(call,result)|json!({"role":"tool","tool_call_id":call.id,"content":tool_output(&result)})));
    }
}
