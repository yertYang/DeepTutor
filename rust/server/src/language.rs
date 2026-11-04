//! Reply language policy shared by turn admission and session settings.
use crate::{Error, Result};
use serde_json::Value;

pub fn label(code: &str) -> Option<&'static str> {
    Some(match code {
        "en" => "English",
        "zh" => "Simplified Chinese",
        "zh-tw" => "Traditional Chinese",
        "ja" => "Japanese",
        "ko" => "Korean",
        "es" => "Spanish",
        "fr" => "French",
        "de" => "German",
        "ru" => "Russian",
        "pt" => "Portuguese",
        "it" => "Italian",
        "ar" => "Arabic",
        "pl" => "Polish",
        "uk" => "Ukrainian",
        "ms" => "Malay",
        "vi" => "Vietnamese",
        "tr" => "Turkish",
        _ => return None,
    })
}
pub fn validate_override(value: &Value) -> Result<()> {
    if value.is_null() || value.as_str().and_then(label).is_some() {
        Ok(())
    } else {
        Err(Error::invalid("Unsupported reply language."))
    }
}
pub fn resolve(request: &Value, preferences: &Value, default: &Value) -> Result<String> {
    let fixed = request
        .get("reply_language_override")
        .unwrap_or(&preferences["reply_language_override"]);
    validate_override(fixed)?;
    let language = fixed
        .as_str()
        .or_else(|| {
            request["language"]
                .as_str()
                .filter(|s| !s.trim().is_empty())
        })
        .or_else(|| default.as_str())
        .unwrap_or("en")
        .trim()
        .to_lowercase();
    let language = if language == "zh-cn" { "zh" } else { &language };
    label(language).ok_or_else(|| Error::invalid("Unsupported reply language."))?;
    Ok(language.to_string())
}
pub fn instructions(request: &Value, preferences: &Value, default: &Value) -> Result<String> {
    let fixed = request
        .get("reply_language_override")
        .unwrap_or(&preferences["reply_language_override"]);
    let language = resolve(request, preferences, default)?;
    let label = label(&language).unwrap();
    let policy = if fixed.is_null() {
        "This is the default output language. If the user explicitly asks for another language, honor that request and continue using it in this conversation."
    } else {
        "The conversation has a fixed reply language. Keep using it even when source material or user messages use another language."
    };
    Ok(format!(
        "You are Lorevia, a helpful learning companion.\n\nWrite all reader-facing text in {label}. Keep proper nouns and formula symbols in their original form. {policy}"
    ))
}
