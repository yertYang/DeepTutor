//! Uses Lorevia's own credential directory and the same cross-process lock as
//! the existing implementation. It never imports ~/.codex credentials.
use crate::{
    Error, Result,
    config::{Settings, http_client, read_json, safe_path, write_json},
    store::{now, text},
};
use fs2::FileExt;
use serde_json::{Value, json};
use std::{fs::OpenOptions, sync::Arc};

pub async fn credentials(settings: Arc<Settings>) -> Result<Value> {
    let root = settings.user_root.join("private/openai-codex");
    let home = settings.user_root.clone();
    let lock_root = root.clone();
    let _guard = tokio::task::spawn_blocking(move || -> Result<std::fs::File> {
        safe_path(&home, &lock_root.join("auth.lock"))?;
        if !lock_root.is_dir() {
            return Err(Error::invalid(
                "Sign in to Codex before choosing a Codex model.",
            ));
        }
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(lock_root.join("auth.lock"))?;
        file.lock_exclusive()?;
        Ok(file)
    })
    .await
    .map_err(|_| Error::conflict("Credential lock failed."))??;
    for name in ["credentials.v1.json", "state.v1.json"] {
        safe_path(&settings.user_root, &root.join(name))?;
    }
    let mut credential = read_json(&root.join("credentials.v1.json"), Value::Null)?;
    let state = read_json(&root.join("state.v1.json"), json!({"generation":0}))?;
    if credential["schema_version"] != 1
        || credential["generation"] != state["generation"]
        || !credential["generation"].is_u64()
        || text(&credential, "access_token").is_empty()
        || text(&credential, "account_id").is_empty()
    {
        return Err(Error::invalid(
            "Codex credentials are missing or no longer current. Sign in again.",
        ));
    }
    if state["rejected_generation"] == credential["generation"] {
        return Err(Error::invalid(
            "Codex authentication was rejected. Sign in again.",
        ));
    }
    if credential["expires_at"].as_f64().unwrap_or(0.0) > now() + 60.0 {
        return Ok(credential);
    }
    if text(&credential, "refresh_token").is_empty() {
        return Err(Error::invalid("Codex login has expired. Sign in again."));
    }
    let response = http_client()?
        .post("https://auth.openai.com/oauth/token")
        .json(&json!({"grant_type":"refresh_token","client_id":"app_EMoamEEZ73f0CkXaXp7hrann","refresh_token":text(&credential,"refresh_token")}))
        .send()
        .await
        .map_err(|_| Error::provider("Codex token refresh failed."))?;
    if !response.status().is_success() {
        return Err(Error::provider(format!(
            "Codex token refresh returned HTTP {}.",
            response.status().as_u16()
        )));
    }
    let payload = crate::config::response_json(response, 1024 * 1024).await?;
    let refreshed = token_credentials(
        &payload,
        credential["generation"].as_u64().unwrap(),
        Some(&credential),
    )?;
    if refreshed["account_id"] != credential["account_id"] {
        return Err(Error::conflict(
            "Codex refresh changed the account identity. Sign in again.",
        ));
    }
    credential = refreshed;
    write_json(&root.join("credentials.v1.json"), &credential)?;
    Ok(credential)
}

/// Claim decoding is used only on tokens received from the fixed TLS token
/// endpoint, never as authentication of an HTTP client.
pub fn token_credentials(
    payload: &Value,
    generation: u64,
    previous: Option<&Value>,
) -> Result<Value> {
    use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
    let mut result = json!({"schema_version":1,"generation":generation});
    // A refresh must return a new access token. Reusing the previous one while
    // extending its expiry would turn a malformed response into a valid login.
    result["access_token"] = json!(
        payload["access_token"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| Error::provider("Codex returned no access token."))?
    );
    for field in ["refresh_token", "id_token"] {
        let value = payload[field]
            .as_str()
            .filter(|s| !s.is_empty())
            .or_else(|| previous.and_then(|p| p[field].as_str()))
            .ok_or_else(|| Error::provider("Codex returned incomplete authentication data."))?;
        result[field] = json!(value);
    }
    let mut account = payload["account_id"]
        .as_str()
        .or_else(|| payload["chatgpt_account_id"].as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_owned);
    let mut expiry = payload["expires_at"]
        .as_u64()
        .filter(|n| *n > 0)
        .or_else(|| {
            payload["expires_in"]
                .as_u64()
                .filter(|n| *n > 0)
                .and_then(|n| (now() as u64).checked_add(n))
        });
    for field in ["access_token", "id_token"] {
        if let Some(encoded) = text(&result, field).split('.').nth(1)
            && let Ok(bytes) = URL_SAFE_NO_PAD.decode(encoded.trim_end_matches('='))
            && let Ok(claims) = serde_json::from_slice::<Value>(&bytes)
        {
            if account.is_none() {
                account = claims["https://api.openai.com/auth"]["chatgpt_account_id"]
                    .as_str()
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned);
            }
            // ID tokens describe authentication, not access-token lifetime.
            // In refresh responses the retained ID token may already be expired.
            if field == "access_token"
                && let Some(expires) = claims["exp"].as_u64().filter(|n| *n > 0)
            {
                expiry = Some(expiry.map_or(expires, |value| value.min(expires)));
            }
        }
    }
    if account.is_none() {
        account = previous
            .and_then(|p| p["account_id"].as_str())
            .map(str::to_owned);
    }
    result["account_id"] =
        json!(account.ok_or_else(|| Error::provider("Codex returned no account identity."))?);
    result["expires_at"] = json!(
        expiry
            .filter(|n| *n > now() as u64)
            .ok_or_else(|| Error::provider("Codex token is expired or has no expiry."))?
    );
    Ok(result)
}

pub async fn login(settings: Arc<Settings>) -> Result<()> {
    crate::oauth_web::cli_login(settings).await
}

/// Match the account catalog's visibility rules. `supported_in_api` refers to
/// the public API and must not hide a model that is selectable in Codex.
pub fn parse_models(payload: &Value) -> Result<Vec<Value>> {
    use sha2::{Digest, Sha256};
    let raw = payload["models"]
        .as_array()
        .ok_or_else(|| Error::provider("Codex catalog has no model list."))?;
    if raw.len() > 512 {
        return Err(Error::provider("Codex catalog exceeds the model limit."));
    }
    let mut visible = Vec::new();
    for model in raw {
        if !model.is_object() {
            return Err(Error::provider("Codex returned an invalid model entry."));
        }
        if model["visibility"] != "list" {
            continue;
        }
        if text(model, "slug").is_empty() {
            return Err(Error::provider("Codex returned an empty model identifier."));
        }
        visible.push(model);
    }
    visible.sort_by_key(|m| (m["priority"].as_i64().unwrap_or(0), text(m, "slug")));
    let mut seen = std::collections::HashSet::new();
    let mut models = Vec::new();
    for model in visible {
        let slug = text(model, "slug");
        if !seen.insert(slug) {
            continue;
        }
        let mut levels = Vec::new();
        for level in model["supported_reasoning_levels"]
            .as_array()
            .into_iter()
            .flatten()
        {
            if let Some(effort) = level.as_str().or_else(|| level["effort"].as_str())
                && !effort.is_empty()
                && !levels.contains(&effort)
            {
                levels.push(effort);
            }
        }
        if slug == "gpt-5.6-luna" && !levels.contains(&"none") {
            levels.insert(0, "none");
        }
        let name = model["display_name"]
            .as_str()
            .filter(|s| !s.is_empty())
            .unwrap_or(slug);
        let summary = model["supports_reasoning_summary_parameter"]
            .as_bool()
            .unwrap_or(model["supports_reasoning_summaries"] == true);
        models.push(json!({
            "id": format!("llm-model-openai-codex-{}", &format!("{:x}", Sha256::digest(slug.as_bytes()))[..16]),
            "name": name,
            "model": slug,
            "managed_by": "openai_codex_oauth",
            "codex_supported_reasoning_levels": levels,
            "codex_default_reasoning_level": model["default_reasoning_level"],
            "codex_supports_reasoning_summary": summary,
            "context_window": model["context_window"].as_u64().filter(|n| *n > 0),
        }));
    }
    if models.is_empty() {
        return Err(Error::provider(
            "Codex returned no selectable models; the previous catalog was kept.",
        ));
    }
    Ok(models)
}

pub async fn refresh_models(settings: Arc<Settings>) -> Result<Value> {
    use deeptutor_codex::headers::{CLIENT_VERSION, ClientProfile};
    use sha2::{Digest, Sha256};
    let token = credentials(settings.clone()).await?;
    let installation = crate::codex_session::installation(settings.clone(), &token).await?;
    let profile = ClientProfile::default();
    let response = http_client()?
        .get("https://chatgpt.com/backend-api/codex/models")
        .query(&[("client_version", CLIENT_VERSION)])
        .bearer_auth(text(&token, "access_token"))
        .header("chatgpt-account-id", text(&token, "account_id"))
        .header("originator", profile.originator)
        .header("user-agent", profile.user_agent)
        .header("version", profile.version)
        .header("x-codex-installation-id", installation)
        .send()
        .await
        .map_err(|_| Error::provider("Codex model catalog request failed."))?;
    if !response.status().is_success() {
        return Err(Error::provider(format!(
            "Codex catalog returned HTTP {}.",
            response.status().as_u16()
        )));
    }
    let payload = crate::config::response_json(response, 8 * 1024 * 1024).await?;
    let mut models = parse_models(&payload)?;
    let count = models.len();
    let root = settings.user_root.join("private/openai-codex");
    tokio::task::spawn_blocking(move||->Result<()> {
        safe_path(&settings.user_root,&root.join("auth.lock"))?;
        let lock=OpenOptions::new().read(true).write(true).open(root.join("auth.lock"))?;lock.lock_exclusive()?;
        let current=read_json(&root.join("state.v1.json"),Value::Null)?;
        if current["generation"]!=token["generation"]{return Err(Error::conflict("Codex account changed during model discovery."));}
        let _guard = settings.settings_lock()?;
        let mut catalog=settings.catalog_locked()?;
        let profile_id="llm-profile-openai-codex-managed";
        let binding=format!("{:x}",Sha256::digest(text(&token,"account_id").as_bytes()));
        let llm=&mut catalog["services"]["llm"];
        let profiles=llm["profiles"].as_array_mut().ok_or_else(||Error::conflict("Invalid LLM catalog."))?;
        if let Some(old)=profiles.iter().find(|p|p["id"]==profile_id&&p["codex_account_binding"]==binding){
            for model in &mut models {if let Some(prior)=old["models"].as_array().into_iter().flatten().find(|m|m["id"]==model["id"])
                && model["codex_supported_reasoning_levels"].as_array().is_some_and(|v|v.contains(&prior["reasoning_effort"])){model["reasoning_effort"]=prior["reasoning_effort"].clone();}}
        }
        let default_model=models[0]["id"].clone();
        let profile=json!({"id":profile_id,"name":"OpenAI Codex","binding":"openai_codex","base_url":"https://chatgpt.com/backend-api","api_key":"","managed_by":"openai_codex_oauth","read_only":true,"owner_bound":true,"codex_account_binding":binding,"models":models});
        if let Some(old)=profiles.iter_mut().find(|p|p["id"]==profile_id){*old=profile;}else{profiles.push(profile)}
        if llm["active_profile_id"].is_null()||text(llm,"active_profile_id").is_empty(){llm["active_profile_id"]=json!(profile_id);llm["active_model_id"]=default_model.clone();}
        if llm["active_profile_id"]==profile_id&&!llm["profiles"].as_array().unwrap().iter().find(|p|p["id"]==profile_id).unwrap()["models"].as_array().unwrap().iter().any(|m|m["id"]==llm["active_model_id"]){llm["active_model_id"]=default_model;}
        settings.save_catalog_locked(catalog)?;Ok(())
    }).await.map_err(|_|Error::conflict("Codex catalog commit failed."))??;
    Ok(json!({"models_imported":count}))
}

/// The settings panel can inspect a CLI-created login without refreshing tokens
/// or sending a model request. No credential fields cross this boundary.
pub fn status(settings: &Settings) -> Result<Value> {
    use sha2::{Digest, Sha256};
    let root = settings.user_root.join("private/openai-codex");
    for name in ["credentials.v1.json", "state.v1.json"] {
        safe_path(&settings.user_root, &root.join(name))?;
    }
    let credential = read_json(&root.join("credentials.v1.json"), Value::Null)?;
    let state = read_json(&root.join("state.v1.json"), json!({"generation":0}))?;
    let connected = credential["schema_version"] == 1
        && credential["generation"].is_u64()
        && credential["generation"] == state["generation"]
        && state["rejected_generation"] != credential["generation"]
        && !text(&credential, "access_token").is_empty()
        && !text(&credential, "account_id").is_empty();
    let binding = format!(
        "{:x}",
        Sha256::digest(text(&credential, "account_id").as_bytes())
    );
    let catalog = settings.catalog()?;
    let llm = &catalog["services"]["llm"];
    let profile = llm["profiles"].as_array().into_iter().flatten().find(|p| {
        p["managed_by"] == "openai_codex_oauth"
            && p["codex_account_binding"] == binding
            && connected
    });
    let models=profile.into_iter().flat_map(|p|p["models"].as_array().into_iter().flatten()).map(|m|json!({"model":m["model"],"name":m["name"],"supported_reasoning_levels":m["codex_supported_reasoning_levels"].as_array().cloned().unwrap_or_default(),"reasoning_effort":m["reasoning_effort"]})).collect::<Vec<_>>();
    let active = profile
        .filter(|p| p["id"] == llm["active_profile_id"])
        .and_then(|p| p["models"].as_array())
        .and_then(|m| m.iter().find(|m| m["id"] == llm["active_model_id"]));
    Ok(
        json!({"connection":if connected{"connected"}else{"disconnected"},"operation_id":null,"operation_state":null,
        "authorize_url":null,"expires_in":null,"callback_port":null,"callback_forward_port":null,"redirect_uri":null,
        "model_count":models.len(),"catalog_source":null,"catalog_fetched_at":null,"active_model":active.map(|m|m["model"].clone()),
        "models":models,"activated":active.is_some(),"error_code":null}),
    )
}

pub fn set_reasoning(settings: &Settings, payload: &Value) -> Result<()> {
    let slug = text(payload, "model");
    if slug.is_empty()
        || (!payload["reasoning_effort"].is_null() && !payload["reasoning_effort"].is_string())
    {
        return Err(Error::invalid("Invalid Codex model reasoning selection."));
    }
    let _guard = settings.settings_lock()?;
    let mut catalog = settings.catalog_locked()?;
    let model = catalog["services"]["llm"]["profiles"]
        .as_array_mut()
        .into_iter()
        .flatten()
        .filter(|p| p["managed_by"] == "openai_codex_oauth")
        .flat_map(|p| p["models"].as_array_mut().into_iter().flatten())
        .find(|m| m["model"] == slug)
        .ok_or_else(|| Error::invalid("Refresh the Codex catalog before choosing this model."))?;
    if !payload["reasoning_effort"].is_null()
        && !model["codex_supported_reasoning_levels"]
            .as_array()
            .is_some_and(|v| v.contains(&payload["reasoning_effort"]))
    {
        return Err(Error::invalid(
            "This reasoning effort is not supported by the Codex model.",
        ));
    }
    model["reasoning_effort"] = payload["reasoning_effort"].clone();
    settings.save_catalog_locked(catalog)?;
    Ok(())
}
