//! Shared CLI/Web OAuth flow. The verifier and tokens never enter HTTP responses.
use crate::{
    Error, Result,
    config::{Settings, http_client, read_json, safe_path, write_json},
    store::{now, text},
};
use axum::{Router, extract::RawQuery, http::StatusCode, routing::get};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use fs2::FileExt;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{fs::OpenOptions, sync::Arc};
use tokio::sync::{Mutex, mpsc, watch};

pub(crate) fn locked<T>(
    settings: &Settings,
    action: impl FnOnce(&std::path::Path, Value) -> Result<T>,
) -> Result<T> {
    let root = settings.user_root.join("private/openai-codex");
    safe_path(&settings.user_root, &root.join("auth.lock"))?;
    std::fs::create_dir_all(&root)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for dir in [root.parent().unwrap(), root.as_path()] {
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
        }
    }
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(root.join("auth.lock"))?;
    file.lock_exclusive()?;
    for name in ["state.v1.json", "credentials.v1.json"] {
        safe_path(&settings.user_root, &root.join(name))?;
    }
    let state = read_json(
        &root.join("state.v1.json"),
        json!({"schema_version":1,"generation":0}),
    )?;
    action(&root, state)
}
fn invalidate(settings: &Settings, id: &str) -> Result<()> {
    locked(settings, |root, mut state| {
        if state["pending_operation"] == id {
            state["pending_operation"] = Value::Null;
            write_json(&root.join("state.v1.json"), &state)?;
        }
        Ok(())
    })
}
fn commit(settings: &Settings, id: &str, generation: u64, credential: &Value) -> Result<()> {
    locked(settings, |root, state| {
        if state["generation"] != generation || state["pending_operation"] != id {
            return Err(Error::conflict(
                "Codex authentication changed during sign-in.",
            ));
        }
        let next = generation
            .checked_add(1)
            .ok_or_else(|| Error::conflict("Codex authentication generation overflow."))?;
        if credential["generation"] != next {
            return Err(Error::conflict("Invalid credential generation."));
        }
        write_json(&root.join("credentials.v1.json"), credential)?;
        write_json(
            &root.join("state.v1.json"),
            &json!({"schema_version":1,"generation":next}),
        )?;
        Ok(())
    })
}
fn code(query: &str, expected: &str) -> Result<String> {
    use subtle::ConstantTimeEq;
    let mut pairs = std::collections::HashMap::new();
    if query.len() > 32768 {
        return Err(Error::invalid("Invalid authentication callback."));
    }
    for (key, value) in reqwest::Url::parse(&format!("http://localhost/?{query}"))
        .map_err(|_| Error::invalid("Invalid authentication callback."))?
        .query_pairs()
    {
        if pairs.insert(key.into_owned(), value.into_owned()).is_some() {
            return Err(Error::invalid("Duplicate callback parameters."));
        }
    }
    if !pairs
        .get("state")
        .is_some_and(|s| s.as_bytes().ct_eq(expected.as_bytes()).into())
    {
        return Err(Error::invalid("Invalid authentication callback state."));
    }
    if pairs.contains_key("error") {
        return Err(Error::new(
            StatusCode::BAD_REQUEST,
            "oauth_denied",
            "Codex sign-in was denied.",
        ));
    }
    pairs
        .remove("code")
        .filter(|s| !s.is_empty() && s.len() <= 16384)
        .ok_or_else(|| Error::invalid("Missing authorization code."))
}
struct Servers(Vec<tokio::task::JoinHandle<()>>);
impl Drop for Servers {
    fn drop(&mut self) {
        for server in &self.0 {
            server.abort();
        }
    }
}
pub(crate) struct Flow {
    pub id: String,
    pub authorize_url: String,
    pub redirect: String,
    pub port: u16,
    pub expires_at: f64,
    state: String,
    verifier: String,
    generation: u64,
    tx: mpsc::Sender<Result<String>>,
    rx: mpsc::Receiver<Result<String>>,
    _servers: Servers,
}
impl Flow {
    pub async fn prepare(settings: Arc<Settings>) -> Result<Self> {
        let mut listener = None;
        for port in [1455, 1457] {
            if let Ok(bound) = tokio::net::TcpListener::bind(("127.0.0.1", port)).await {
                listener = Some(bound);
                break;
            }
        }
        let listener = listener
            .ok_or_else(|| Error::conflict("Codex callback ports 1455 and 1457 are occupied."))?;
        let port = listener.local_addr()?.port();
        let redirect = format!("http://localhost:{port}/auth/callback");
        let id = uuid::Uuid::new_v4().simple().to_string();
        let pending = id.clone();
        let generation = crate::application::blocking(move || {
            locked(&settings, |root, mut state| {
                let generation = state["generation"]
                    .as_u64()
                    .ok_or_else(|| Error::conflict("Invalid Codex authentication generation."))?;
                state["pending_operation"] = json!(pending);
                write_json(&root.join("state.v1.json"), &state)?;
                Ok(generation)
            })
        })
        .await?;
        let verifier = (0..3)
            .map(|_| uuid::Uuid::new_v4().simple().to_string())
            .collect::<String>();
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        let state = uuid::Uuid::new_v4().simple().to_string();
        let (tx, rx) = mpsc::channel(1);
        let send = tx.clone();
        let expected = state.clone();
        let callback = Router::new().route(
            "/auth/callback",
            get(move |RawQuery(query): RawQuery| {
                let send = send.clone();
                let expected = expected.clone();
                async move {
                    let result = code(query.as_deref().unwrap_or(""), &expected);
                    if result
                        .as_ref()
                        .is_err_and(|error| error.code != "oauth_denied")
                    {
                        return (StatusCode::BAD_REQUEST, "Invalid authentication callback.");
                    }
                    if send.try_send(result).is_err() {
                        return (StatusCode::CONFLICT, "Callback already received.");
                    }
                    (
                        StatusCode::OK,
                        "Sign-in callback received. Return to the application.",
                    )
                }
            }),
        );
        let mut servers = Servers(vec![]);
        if let Ok(ipv6) = tokio::net::TcpListener::bind(("::1", port)).await {
            let app = callback.clone();
            servers.0.push(tokio::spawn(async move {
                let _ = axum::serve(ipv6, app).await;
            }));
        }
        servers.0.push(tokio::spawn(async move {
            let _ = axum::serve(listener, callback).await;
        }));
        let mut url = reqwest::Url::parse("https://auth.openai.com/oauth/authorize").unwrap();
        url.query_pairs_mut().extend_pairs([
            ("response_type", "code"),
            ("client_id", "app_EMoamEEZ73f0CkXaXp7hrann"),
            ("redirect_uri", redirect.as_str()),
            (
                "scope",
                "openid profile email offline_access api.connectors.read api.connectors.invoke",
            ),
            ("code_challenge", challenge.as_str()),
            ("code_challenge_method", "S256"),
            ("state", state.as_str()),
            ("originator", "codex_cli_rs"),
            ("id_token_add_organizations", "true"),
            ("codex_cli_simplified_flow", "true"),
        ]);
        Ok(Self {
            id,
            authorize_url: url.to_string(),
            redirect,
            port,
            expires_at: now() + 300.0,
            state,
            verifier,
            generation,
            tx,
            rx,
            _servers: servers,
        })
    }
    pub fn description(&self) -> Value {
        json!({"operation_id":self.id,"authorize_url":self.authorize_url,"expires_in":300,"callback_port":self.port,"callback_forward_port":self.port,"redirect_uri":self.redirect,"ssh_forward_command":format!("ssh -N -L {}:127.0.0.1:{} <ssh-user>@<server-host>",self.port,self.port)})
    }
    async fn finish(mut self, settings: Arc<Settings>, snapshot: Arc<Mutex<Value>>) -> Result<()> {
        let code = tokio::time::timeout(std::time::Duration::from_secs(300), self.rx.recv())
            .await
            .map_err(|_| Error::invalid("Codex sign-in timed out."))?
            .ok_or_else(|| Error::invalid("Codex sign-in cancelled."))??;
        snapshot.lock().await["operation_state"] = json!("exchanging");
        let response = http_client()?
            .post("https://auth.openai.com/oauth/token")
            .form(&[
                ("grant_type", "authorization_code"),
                ("client_id", "app_EMoamEEZ73f0CkXaXp7hrann"),
                ("code", code.as_str()),
                ("redirect_uri", self.redirect.as_str()),
                ("code_verifier", self.verifier.as_str()),
            ])
            .send()
            .await
            .map_err(|_| Error::provider("Codex token exchange failed."))?;
        if !response.status().is_success() {
            return Err(Error::provider("Codex token exchange was rejected."));
        }
        let payload = crate::config::response_json(response, 1024 * 1024).await?;
        let next = self
            .generation
            .checked_add(1)
            .ok_or_else(|| Error::conflict("Codex authentication generation overflow."))?;
        let credential = crate::oauth::token_credentials(&payload, next, None)?;
        let id = self.id.clone();
        let generation = self.generation;
        crate::application::blocking(move || commit(&settings, &id, generation, &credential)).await
    }
}
struct Operation {
    id: String,
    port: u16,
    state: String,
    tx: mpsc::Sender<Result<String>>,
    expires_at: f64,
    snapshot: Arc<Mutex<Value>>,
    cancel: watch::Sender<bool>,
    task: tokio::task::JoinHandle<()>,
}
#[derive(Default)]
pub struct WebLogin {
    operation: Mutex<Option<Operation>>,
    closing: std::sync::atomic::AtomicBool,
}
impl WebLogin {
    pub async fn start(&self, settings: Arc<Settings>) -> Result<Value> {
        let mut slot = self.operation.lock().await;
        if self.closing.load(std::sync::atomic::Ordering::Acquire) {
            return Err(Error::conflict("Server is shutting down."));
        }
        if slot.as_ref().is_some_and(|op| !op.task.is_finished()) {
            return Err(Error::conflict("A Codex sign-in is already in progress."));
        }
        let flow = Flow::prepare(settings.clone()).await?;
        let description = flow.description();
        let snapshot = Arc::new(Mutex::new(description.clone()));
        snapshot.lock().await["operation_state"] = json!("waiting");
        let (cancel, mut cancelled) = watch::channel(false);
        let status = snapshot.clone();
        let id = flow.id.clone();
        let pending = id.clone();
        let port = flow.port;
        let state = flow.state.clone();
        let tx = flow.tx.clone();
        let expires_at = flow.expires_at;
        let task = tokio::spawn(async move {
            let result = tokio::select! {value=flow.finish(settings.clone(),status.clone())=>value,_=cancelled.changed()=>Err(Error::invalid("Codex sign-in cancelled."))};
            let was_cancelled = *cancelled.borrow();
            let settings_for_clear = settings.clone();
            let _ = crate::application::blocking(move || invalidate(&settings_for_clear, &pending))
                .await;
            let mut s = status.lock().await;
            s["authorize_url"] = Value::Null;
            if was_cancelled {
                s["operation_state"] = json!("cancelled");
                return;
            }
            if result.is_err() {
                s["operation_state"] = json!(if now() >= expires_at {
                    "expired"
                } else {
                    "failed"
                });
                s["error_code"] = json!(if now() >= expires_at {
                    "login_timeout"
                } else {
                    "token_exchange_failed"
                });
                return;
            }
            s["operation_state"] = json!("fetching_models");
            drop(s);
            let refreshed = tokio::select! {value=crate::oauth::refresh_models(settings)=>value,_=cancelled.changed()=>Err(Error::invalid("Codex model refresh cancelled."))};
            let mut s = status.lock().await;
            s["operation_state"] = json!("completed");
            if refreshed.is_err() {
                s["error_code"] = json!("catalog_unavailable");
            }
        });
        *slot = Some(Operation {
            id,
            port,
            state,
            tx,
            expires_at,
            snapshot,
            cancel,
            task,
        });
        Ok(description)
    }
    pub async fn status(&self, settings: Arc<Settings>) -> Result<Value> {
        let mut result =
            crate::application::blocking(move || crate::oauth::status(&settings)).await?;
        if let Some(op) = self.operation.lock().await.as_ref() {
            let snapshot = op.snapshot.lock().await;
            for key in [
                "operation_id",
                "operation_state",
                "authorize_url",
                "callback_port",
                "callback_forward_port",
                "redirect_uri",
                "error_code",
            ] {
                if let Some(value) = snapshot.get(key) {
                    result[key] = value.clone();
                }
            }
            result["expires_in"] = json!((op.expires_at - now()).max(0.0).ceil() as u64);
            if matches!(
                text(&result, "operation_state"),
                "waiting" | "exchanging" | "fetching_models"
            ) {
                result["connection"] = json!("authorizing");
            } else if matches!(text(&result, "operation_state"), "failed" | "expired")
                && result["connection"] != "connected"
            {
                result["connection"] = json!("error");
            }
        }
        Ok(result)
    }
    pub async fn complete(&self, callback: &str) -> Result<()> {
        let slot = self.operation.lock().await;
        let op = slot
            .as_ref()
            .filter(|op| !op.task.is_finished())
            .ok_or_else(|| Error::conflict("No Codex sign-in is waiting."))?;
        let url =
            reqwest::Url::parse(callback).map_err(|_| Error::invalid("Invalid callback URL."))?;
        if callback.len() > 32768
            || url.scheme() != "http"
            || !matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"))
            || url.port() != Some(op.port)
            || url.path() != "/auth/callback"
            || !url.username().is_empty()
            || url.password().is_some()
            || url.fragment().is_some()
        {
            return Err(Error::invalid("Invalid callback URL."));
        }
        if op.snapshot.lock().await["operation_state"] != "waiting" || now() >= op.expires_at {
            return Err(Error::conflict("This Codex sign-in is no longer waiting."));
        }
        let code = code(url.query().unwrap_or(""), &op.state)?;
        op.tx
            .try_send(Ok(code))
            .map_err(|_| Error::conflict("Callback already received."))?;
        Ok(())
    }
    pub async fn cancel(&self, settings: Arc<Settings>) -> Result<()> {
        let mut slot = self.operation.lock().await;
        if let Some(op) = slot.as_mut() {
            let id = op.id.clone();
            crate::application::blocking(move || invalidate(&settings, &id)).await?;
            if !op.task.is_finished() {
                let _ = op.cancel.send(true);
                let _ = (&mut op.task).await;
            }
        }
        Ok(())
    }
    pub async fn shutdown(&self, settings: Arc<Settings>) {
        self.closing
            .store(true, std::sync::atomic::Ordering::Release);
        let _ = self.cancel(settings).await;
        if let Some(op) = self.operation.lock().await.take()
            && !op.task.is_finished()
        {
            let _ = op.task.await;
        }
    }
    pub async fn logout(&self, settings: Arc<Settings>) -> Result<()> {
        let mut slot = self.operation.lock().await;
        if let Some(op) = slot.as_mut() {
            let id = op.id.clone();
            let home = settings.clone();
            crate::application::blocking(move || invalidate(&home, &id)).await?;
            if !op.task.is_finished() {
                let _ = op.cancel.send(true);
                let _ = (&mut op.task).await;
            }
        }
        crate::application::blocking(move || logout(&settings)).await?;
        *slot = None;
        Ok(())
    }
}
pub async fn cli_login(settings: Arc<Settings>) -> Result<()> {
    let flow = Flow::prepare(settings.clone()).await?;
    eprintln!("Open this URL to sign in to Codex:\n{}", flow.authorize_url);
    let id = flow.id.clone();
    let result = tokio::select! {result=flow.finish(settings.clone(),Arc::new(Mutex::new(json!({}))))=>result,_=tokio::signal::ctrl_c()=>Err(Error::invalid("Codex sign-in cancelled."))};
    crate::application::blocking(move || invalidate(&settings, &id)).await?;
    result
}

pub fn logout(settings: &Settings) -> Result<()> {
    locked(settings, |root, state| {
        let next = state["generation"]
            .as_u64()
            .and_then(|n| n.checked_add(1))
            .ok_or_else(|| Error::conflict("Invalid Codex authentication generation."))?;
        write_json(
            &root.join("state.v1.json"),
            &json!({"schema_version":1,"generation":next}),
        )?;
        write_json(&root.join("credentials.v1.json"), &Value::Null)?;
        let _guard = settings.settings_lock()?;
        let mut catalog = settings.catalog_locked()?;
        let llm = &mut catalog["services"]["llm"];
        if let Some(profiles) = llm["profiles"].as_array_mut() {
            let removed = profiles
                .iter()
                .filter(|p| p["managed_by"] == "openai_codex_oauth")
                .map(|p| p["id"].clone())
                .collect::<Vec<_>>();
            profiles.retain(|p| p["managed_by"] != "openai_codex_oauth");
            if removed.contains(&llm["active_profile_id"]) {
                llm["active_profile_id"] = Value::Null;
                llm["active_model_id"] = Value::Null;
            }
        }
        settings.save_catalog_locked(catalog)?;
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn callback_validation_requires_unique_parameters_and_constant_state() {
        assert_eq!(code("state=expected&code=abc", "expected").unwrap(), "abc");
        for query in [
            "state=wrong&code=abc",
            "state=expected&state=expected&code=abc",
            "state=expected&code=a&code=b",
            "state=expected",
            "state=expected&code=",
        ] {
            assert!(code(query, "expected").is_err());
        }
        assert_eq!(
            code("state=expected&error=access_denied", "expected")
                .unwrap_err()
                .code,
            "oauth_denied"
        );
    }
    #[tokio::test]
    async fn web_login_cancels_releases_ports_and_fences_obsolete_callbacks() {
        let dir = tempfile::tempdir().unwrap();
        let settings = Arc::new(Settings::open(dir.path()).unwrap());
        let manager = WebLogin::default();
        let flow = manager.start(settings.clone()).await.unwrap();
        let url = reqwest::Url::parse(text(&flow, "authorize_url")).unwrap();
        let params = url
            .query_pairs()
            .collect::<std::collections::HashMap<_, _>>();
        assert_eq!(params["code_challenge_method"], "S256");
        assert!(flow.get("verifier").is_none());
        let state = params["state"].to_string();
        let callback = format!("{}?state={state}&code=test", text(&flow, "redirect_uri"));
        assert!(manager.start(settings.clone()).await.is_err());
        for invalid in [
            callback.replace("http:", "https:"),
            callback.replace("localhost", "attacker.invalid"),
            callback.replace("/auth/callback", "/other"),
            format!("{callback}#fragment"),
            format!("{callback}&code=duplicate"),
            callback.replace(&state, "wrong-state"),
        ] {
            assert!(manager.complete(&invalid).await.is_err());
        }
        // Invalid and denied callbacks never contact the token endpoint.
        let client = reqwest::Client::new();
        let port = flow["callback_port"].as_u64().unwrap();
        assert_eq!(
            client
                .get(format!(
                    "http://127.0.0.1:{port}/auth/callback?state=bad&code=test"
                ))
                .send()
                .await
                .unwrap()
                .status(),
            400
        );
        assert_eq!(
            manager.status(settings.clone()).await.unwrap()["operation_state"],
            "waiting"
        );
        manager.cancel(settings.clone()).await.unwrap();
        assert_eq!(
            manager.status(settings.clone()).await.unwrap()["operation_state"],
            "cancelled"
        );
        assert!(manager.complete(&callback).await.is_err());
        let pending = read_json(
            &settings
                .user_root
                .join("private/openai-codex/state.v1.json"),
            Value::Null,
        )
        .unwrap();
        assert!(pending["pending_operation"].is_null());
        assert!(
            commit(
                &settings,
                text(&flow, "operation_id"),
                0,
                &json!({"generation":1})
            )
            .is_err()
        );
        let second = manager.start(settings.clone()).await.unwrap();
        assert_ne!(second["operation_id"], flow["operation_id"]);
        let second_url = reqwest::Url::parse(text(&second, "authorize_url")).unwrap();
        let second_state = second_url
            .query_pairs()
            .find(|(k, _)| k == "state")
            .unwrap()
            .1
            .into_owned();
        client
            .get(format!(
                "http://127.0.0.1:{}/auth/callback?state={second_state}&error=access_denied",
                second["callback_port"]
            ))
            .send()
            .await
            .unwrap();
        for _ in 0..100 {
            if manager.status(settings.clone()).await.unwrap()["operation_state"] == "failed" {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert_eq!(
            manager.status(settings.clone()).await.unwrap()["operation_state"],
            "failed"
        );
        manager.shutdown(settings.clone()).await;
        // A synthetic token commit exercises only local storage, never account login.
        locked(&settings, |root, mut state| {
            state["pending_operation"] = json!("fresh");
            write_json(&root.join("state.v1.json"), &state)
        })
        .unwrap();
        commit(&settings,"fresh",0,&json!({"schema_version":1,"generation":1,"access_token":"synthetic","account_id":"synthetic","expires_at":now()+300.0})).unwrap();
        assert_eq!(
            crate::oauth::status(&settings).unwrap()["connection"],
            "connected"
        );
        logout(&settings).unwrap();
        assert_eq!(
            crate::oauth::status(&settings).unwrap()["connection"],
            "disconnected"
        );
        assert!(commit(&settings, "fresh", 0, &json!({"generation":1})).is_err());
        assert!(
            read_json(
                &settings
                    .user_root
                    .join("private/openai-codex/credentials.v1.json"),
                Value::Null
            )
            .unwrap()
            .is_null()
        );
    }
}
