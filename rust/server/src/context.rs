//! Text-only input budgeting. Estimate one token per UTF-8 byte and reserve
//! framing per message. Provider-reported usage remains authoritative for billing.
use crate::{Error, Result, config::Model, store::text};
use serde_json::{Value, json};

fn cost(content: &str) -> u64 {
    content.len() as u64 + 32
}

pub fn output_reserve(model: &Model) -> Result<u64> {
    let mut default = (model.context_window / 4).clamp(1, 8192);
    let mut minimum = 1;
    if model.format == "anthropic" {
        let thinking = match model.effort.as_deref() {
            None | Some("none" | "adaptive") => 0,
            Some("low") => 1024,
            Some("medium") => 4096,
            Some("high") => 8192,
            _ => return Err(Error::invalid("Unsupported Anthropic reasoning effort.")),
        };
        minimum = thinking + 1;
        let target = if thinking == 0 { 8192 } else { 16384 };
        default = target.min((model.context_window / 2).max(minimum));
    }
    let reserved = model.max_tokens.unwrap_or(default);
    if reserved < minimum {
        return Err(Error::invalid(
            "Output limit is too small for the selected reasoning effort.",
        ));
    }
    Ok(reserved)
}

pub fn remaining_scoped(
    model: &Model,
    instructions: &str,
    question: &str,
    capability: crate::capability::Capability,
    flags: &crate::learning::ToolFlags,
) -> Result<u64> {
    let reserved = output_reserve(model)?;
    model.context_window.checked_sub(reserved)
        .and_then(|n| n.checked_sub(384 + crate::tools::definitions_scoped(capability,flags).iter().map(|v|v.to_string().len() as u64).sum::<u64>() + cost(instructions) + cost(question)))
        .ok_or_else(|| Error::invalid("Current question and instructions exceed the model context window after reserving output. Shorten the question or adjust the model settings."))
}

pub fn assemble(
    mut branch: Vec<(i64, Value)>,
    session: &Value,
    mut budget: u64,
) -> Result<(Vec<Value>, Value)> {
    let initial_budget = budget;
    let (_, current) = branch
        .pop()
        .ok_or_else(|| Error::conflict("Missing current question."))?;
    let mut messages = vec![current];
    let mut start = branch.len();
    // All native completed turns have exactly one user and one assistant.
    // Stop at incompatible legacy tool or system records to avoid partial turns.
    while start >= 2 {
        let user = &branch[start - 2].1;
        let assistant = &branch[start - 1].1;
        if user["role"] != "user" || assistant["role"] != "assistant" {
            break;
        }
        let price = cost(text(user, "content")) + cost(text(assistant, "content"));
        if price > budget {
            break;
        }
        budget -= price;
        start -= 2;
    }
    messages.splice(0..0, branch[start..].iter().map(|(_, m)| m.clone()));
    let summary = text(session, "compressed_summary");
    let mut summary_bytes = 0;
    if !summary.is_empty()
        && branch[..start]
            .iter()
            .any(|(id, _)| Some(*id) == session["summary_up_to_msg_id"].as_i64())
    {
        let prefix = format!("Earlier conversation summary:\n{summary}\n\n");
        if prefix.len() as u64 <= budget {
            budget -= prefix.len() as u64;
            messages[0]["content"] = json!(prefix + text(&messages[0], "content"));
            summary_bytes = summary.len();
        }
    }
    Ok((
        messages,
        json!({"estimator":"utf8_bytes_plus_framing","estimated_history_tokens":initial_budget-budget,"omitted_messages":start,"reused_summary_bytes":summary_bytes}),
    ))
}
