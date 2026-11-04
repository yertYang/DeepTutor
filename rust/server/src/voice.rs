//! PersonaPlex's binary Opus protocol. Voice never executes tools or resumes an
//! interrupted call. Private configuration is home/workspace scoped; audio and
//! text are forwarded with bounded buffers and are not persisted.
use crate::{
    Application, Error, Result,
    application::blocking,
    config::{Settings, read_json, safe_path, write_json},
    data_api::Scope,
};
use axum::{
    Json, Router,
    extract::{
        Path, Query, State, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    http::HeaderMap,
    response::Response,
    routing::{get, post, put},
};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use subtle::ConstantTimeEq;
use tokio::sync::{OwnedSemaphorePermit, Semaphore, watch};
use tokio_tungstenite::{
    connect_async_with_config,
    tungstenite::{Message as UpMessage, client::IntoClientRequest, protocol::WebSocketConfig},
};

const DEFAULT_PROMPT: &str = "You are a wise and friendly English conversation coach. Have a natural conversation. Ask one question at a time, keep answers concise, and gently help the learner improve their spoken English.";
const MAX_PACKET: usize = 64 * 1024;
const TICKET_TTL: Duration = Duration::from_secs(30);
const MAX_CALL: Duration = Duration::from_secs(30 * 60);
const READY_TIMEOUT: Duration = Duration::from_secs(120);
const IDLE_TIMEOUT: Duration = Duration::from_secs(30);
pub(crate) const VOICES: &[&str] = &[
    "NATF0.pt", "NATF1.pt", "NATF2.pt", "NATF3.pt", "NATM0.pt", "NATM1.pt", "NATM2.pt", "NATM3.pt",
    "VARF0.pt", "VARF1.pt", "VARF2.pt", "VARF3.pt", "VARF4.pt", "VARM0.pt", "VARM1.pt", "VARM2.pt",
    "VARM3.pt", "VARM4.pt",
];

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    schema_version: u32,
    revision: u64,
    enabled: bool,
    endpoint: String,
    voice: String,
    prompt: String,
    api_key: String,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            schema_version: 1,
            revision: 0,
            enabled: false,
            endpoint: "ws://127.0.0.1:8998/api/chat".into(),
            voice: "NATF2.pt".into(),
            prompt: DEFAULT_PROMPT.into(),
            api_key: String::new(),
        }
    }
}
impl Config {
    fn path(settings: &Settings, scope: &str) -> Result<std::path::PathBuf> {
        let path = settings
            .user_root
            .join("private/voice")
            .join(format!("{:x}.json", Sha256::digest(scope.as_bytes())));
        safe_path(&settings.user_root, &path)?;
        Ok(path)
    }
    fn load(settings: &Settings, scope: &str) -> Result<Self> {
        let path = Self::path(settings, scope)?;
        if !path.try_exists()? {
            return Ok(Self::default());
        }
        let value = read_json(&path, Value::Null)?;
        let config: Self = serde_json::from_value(value)
            .map_err(|_| Error::conflict("Invalid voice configuration."))?;
        config.validate()?;
        Ok(config)
    }
    fn validate(&self) -> Result<()> {
        if self.schema_version != 1
            || !VOICES.contains(&self.voice.as_str())
            || self.prompt.trim().is_empty()
            || self.prompt.len() > 8192
            || self.api_key.len() > 4096
            || self.endpoint.len() > 2048
        {
            return Err(Error::invalid("Invalid PersonaPlex configuration."));
        }
        let url = reqwest::Url::parse(&self.endpoint).map_err(|_| {
            Error::invalid(
                "Enter a ws:// or wss:// PersonaPlex service address ending in /api/chat.",
            )
        })?;
        let local = url.host_str().is_some_and(|s| {
            s == "localhost"
                || s.trim_matches(['[', ']'])
                    .parse::<std::net::IpAddr>()
                    .is_ok_and(|ip| ip.is_loopback())
        });
        if !matches!(url.scheme(), "ws" | "wss")
            || url.host_str().is_none()
            || (!local && url.scheme() != "wss")
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
            || !url.path().ends_with("/api/chat")
        {
            return Err(Error::invalid(
                "Use wss:// for a remote PersonaPlex service, or ws:// on loopback. The path must end in /api/chat; credentials and query parameters belong in the separate fields.",
            ));
        }
        if !self.api_key.is_empty() {
            axum::http::HeaderValue::from_str(&format!("Bearer {}", self.api_key))
                .map_err(|_| Error::invalid("Invalid voice service API key."))?;
        }
        Ok(())
    }
    fn public(&self) -> Value {
        json!({"revision":self.revision,"enabled":self.enabled,"endpoint":self.endpoint,"voice":self.voice,"prompt":self.prompt,"has_api_key":!self.api_key.is_empty()})
    }
    fn request(&self) -> Result<tokio_tungstenite::tungstenite::http::Request<()>> {
        let mut url = reqwest::Url::parse(&self.endpoint)
            .map_err(|_| Error::invalid("Invalid voice service address."))?;
        url.query_pairs_mut()
            .append_pair("voice_prompt", &self.voice)
            .append_pair("text_prompt", &self.prompt);
        let mut request = url
            .as_str()
            .into_client_request()
            .map_err(|_| Error::invalid("Invalid voice service address."))?;
        if !self.api_key.is_empty() {
            let mut value = axum::http::HeaderValue::from_str(&format!("Bearer {}", self.api_key))
                .map_err(|_| Error::invalid("Invalid voice service API key."))?;
            value.set_sensitive(true);
            request.headers_mut().insert("authorization", value);
        }
        Ok(request)
    }
}

struct Pending {
    scope: String,
    config: Config,
    ticket: [u8; 32],
    expires: Instant,
    permit: OwnedSemaphorePermit,
}
struct Active {
    scope: String,
    cancel: watch::Sender<bool>,
}
#[derive(Default)]
struct Calls {
    pending: HashMap<String, Pending>,
    active: HashMap<String, Active>,
}
pub(crate) struct Runtime {
    serial: Mutex<()>,
    calls: Mutex<Calls>,
    capacity: Arc<Semaphore>,
}
impl Default for Runtime {
    fn default() -> Self {
        Self {
            serial: Mutex::new(()),
            calls: Mutex::new(Calls::default()),
            capacity: Arc::new(Semaphore::new(1)),
        }
    }
}
struct Lease {
    runtime: Arc<Runtime>,
    id: String,
    config: Config,
    cancel: watch::Receiver<bool>,
    _permit: OwnedSemaphorePermit,
}
impl Drop for Lease {
    fn drop(&mut self) {
        if let Ok(mut calls) = self.runtime.calls.lock() {
            calls.active.remove(&self.id);
        }
    }
}
impl Runtime {
    fn reserve(&self, scope: String, config: Config) -> Result<Value> {
        let mut calls = self
            .calls
            .lock()
            .map_err(|_| Error::conflict("Voice sessions are unavailable."))?;
        calls.pending.retain(|_, p| p.expires > Instant::now());
        let permit = self.capacity.clone().try_acquire_owned().map_err(|_| {
            Error::conflict(
                "A voice conversation is already active. End it before starting another.",
            )
        })?;
        let id = uuid::Uuid::new_v4().to_string();
        let ticket = uuid::Uuid::new_v4().to_string();
        calls.pending.insert(
            id.clone(),
            Pending {
                scope,
                config,
                ticket: Sha256::digest(ticket.as_bytes()).into(),
                expires: Instant::now() + TICKET_TTL,
                permit,
            },
        );
        Ok(
            json!({"id":id,"ticket":ticket,"expires_in":TICKET_TTL.as_secs(),"websocket_path":format!("/ws/voice/{id}")}),
        )
    }
    fn take(self: &Arc<Self>, scope: &str, id: &str, ticket: &str) -> Result<Lease> {
        let mut calls = self
            .calls
            .lock()
            .map_err(|_| Error::conflict("Voice sessions are unavailable."))?;
        calls.pending.retain(|_, p| p.expires > Instant::now());
        let candidate = calls.pending.get(id).ok_or_else(Error::missing)?;
        let hash: [u8; 32] = Sha256::digest(ticket.as_bytes()).into();
        if candidate.scope != scope || !bool::from(candidate.ticket.ct_eq(&hash)) {
            return Err(Error::missing());
        }
        let pending = calls.pending.remove(id).unwrap();
        let (cancel, receiver) = watch::channel(false);
        calls.active.insert(
            id.into(),
            Active {
                scope: scope.into(),
                cancel,
            },
        );
        Ok(Lease {
            runtime: self.clone(),
            id: id.into(),
            config: pending.config,
            cancel: receiver,
            _permit: pending.permit,
        })
    }
    fn cancel_scope(&self, scope: &str) {
        if let Ok(mut calls) = self.calls.lock() {
            calls.pending.retain(|_, p| p.scope != scope);
            for active in calls.active.values().filter(|p| p.scope == scope) {
                active.cancel.send_replace(true);
            }
        }
    }
}

pub(crate) fn router() -> Router<Arc<Application>> {
    Router::new()
        .route("/api/voice", get(state))
        .route("/api/voice/config", put(save))
        .route("/api/voice/sessions", post(start))
        .route("/api/voice/sessions/{id}/cancel", post(cancel))
        .route("/ws/voice/{id}", get(socket))
}
async fn state(
    State(app): State<Arc<Application>>,
    Query(scope): Query<Scope>,
) -> Result<Json<Value>> {
    let scope = scope.id()?;
    Ok(Json(blocking(move || Ok(json!({"config":Config::load(&app.settings,&scope)?.public(),"voices":VOICES,"provider":"personaplex","languages":["en"],"audio_storage":false}))).await?))
}
async fn save(
    State(app): State<Arc<Application>>,
    Query(scope): Query<Scope>,
    headers: HeaderMap,
    Json(input): Json<Value>,
) -> Result<Json<Value>> {
    let scope = scope.id()?;
    Ok(Json(
        blocking(move || {
            let _serial = app
                .voice
                .serial
                .lock()
                .map_err(|_| Error::conflict("Voice configuration is busy."))?;
            let old = Config::load(&app.settings, &scope)?;
            if input.as_object().is_none_or(|m| {
                m.keys().any(|k| {
                    ![
                        "expected_revision",
                        "enabled",
                        "endpoint",
                        "voice",
                        "prompt",
                        "api_key",
                        "clear_api_key",
                    ]
                    .contains(&k.as_str())
                })
            }) {
                return Err(Error::invalid("Invalid voice configuration fields."));
            }
            if input["expected_revision"].as_u64() != Some(old.revision) {
                return Err(Error::conflict(
                    "Voice configuration changed. Reload before saving.",
                ));
            }
            let mut config = Config {
                revision: old
                    .revision
                    .checked_add(1)
                    .ok_or_else(|| Error::conflict("Voice revision overflow."))?,
                enabled: input["enabled"]
                    .as_bool()
                    .ok_or_else(|| Error::invalid("Choose whether voice is enabled."))?,
                endpoint: crate::learning::string(&input, "endpoint", 2048, true)?,
                voice: crate::learning::string(&input, "voice", 64, true)?,
                prompt: crate::learning::string(&input, "prompt", 8192, true)?,
                api_key: old.api_key,
                schema_version: 1,
            };
            if input["clear_api_key"] == true {
                config.api_key.clear();
            }
            if let Some(key) = input.get("api_key") {
                config.api_key = key
                    .as_str()
                    .ok_or_else(|| Error::invalid("Invalid voice service API key."))?
                    .into();
            }
            config.validate()?;
            let endpoint = reqwest::Url::parse(&config.endpoint).unwrap();
            for (name, prefix) in [("host", "http://"), ("origin", "")] {
                if let Some(url) = headers
                    .get(name)
                    .and_then(|h| h.to_str().ok())
                    .and_then(|h| reqwest::Url::parse(&format!("{prefix}{h}")).ok())
                    && endpoint.host_str() == url.host_str()
                    && endpoint.port_or_known_default() == url.port_or_known_default()
                {
                    return Err(Error::invalid(
                        "Choose a separate PersonaPlex service address.",
                    ));
                }
            }
            let path = Config::path(&app.settings, &scope)?;
            std::fs::create_dir_all(path.parent().unwrap())?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(
                    path.parent().unwrap(),
                    std::fs::Permissions::from_mode(0o700),
                )?;
            }
            write_json(
                &path,
                &serde_json::to_value(&config)
                    .map_err(|_| Error::invalid("Invalid voice configuration."))?,
            )?;
            app.voice.cancel_scope(&scope);
            Ok(config.public())
        })
        .await?,
    ))
}
async fn start(
    State(app): State<Arc<Application>>,
    Query(scope): Query<Scope>,
    Json(input): Json<Value>,
) -> Result<Json<Value>> {
    let scope = scope.id()?;
    if *app.shutdown_receiver().borrow() {
        return Err(Error::conflict("Server is shutting down."));
    }
    Ok(Json(
        blocking(move || {
            let _serial = app
                .voice
                .serial
                .lock()
                .map_err(|_| Error::conflict("Voice configuration is busy."))?;
            let config = Config::load(&app.settings, &scope)?;
            if !config.enabled {
                return Err(Error::invalid(
                    "Configure and enable PersonaPlex before starting a voice conversation.",
                ));
            }
            if input.as_object().is_none_or(|m| m.len() != 1)
                || input["expected_revision"].as_u64() != Some(config.revision)
            {
                return Err(Error::conflict(
                    "Voice configuration changed. Reload before starting.",
                ));
            }
            app.voice.reserve(scope, config)
        })
        .await?,
    ))
}
async fn cancel(
    State(app): State<Arc<Application>>,
    Query(scope): Query<Scope>,
    Path(id): Path<String>,
) -> Result<Json<Value>> {
    let scope = scope.id()?;
    let mut calls = app
        .voice
        .calls
        .lock()
        .map_err(|_| Error::conflict("Voice sessions are unavailable."))?;
    if calls.pending.get(&id).is_some_and(|p| p.scope == scope) {
        calls.pending.remove(&id);
    } else if let Some(active) = calls.active.get(&id).filter(|p| p.scope == scope) {
        active.cancel.send_replace(true);
    } else {
        return Err(Error::missing());
    }
    Ok(Json(json!({"cancelled":true})))
}
#[derive(Deserialize)]
struct SocketQuery {
    #[serde(flatten)]
    scope: Scope,
    ticket: String,
}
async fn socket(
    State(app): State<Arc<Application>>,
    Path(id): Path<String>,
    Query(query): Query<SocketQuery>,
    ws: WebSocketUpgrade,
) -> Result<Response> {
    let scope = query.scope.id()?;
    if query.ticket.len() > 128 {
        return Err(Error::missing());
    }
    let lease = app.voice.take(&scope, &id, &query.ticket)?;
    let shutdown = app.shutdown_receiver();
    Ok(ws
        .max_message_size(MAX_PACKET)
        .max_frame_size(MAX_PACKET)
        .on_upgrade(move |socket| bridge(socket, lease, shutdown)))
}
fn valid_upstream(data: &[u8], ready: bool) -> bool {
    match data.first() {
        Some(0) => !ready && data.len() == 1,
        Some(1) => ready && data.len() > 1 && data.len() <= MAX_PACKET,
        Some(2) => ready && data.len() <= 4097 && std::str::from_utf8(&data[1..]).is_ok(),
        _ => false,
    }
}
async fn failure(socket: &mut WebSocket, message: &str) {
    let _ = tokio::time::timeout(
        Duration::from_secs(2),
        socket.send(Message::Text(
            json!({"type":"error","message":message}).to_string().into(),
        )),
    )
    .await;
    let _ = tokio::time::timeout(Duration::from_secs(2), socket.close()).await;
}
async fn bridge(mut socket: WebSocket, mut lease: Lease, mut shutdown: watch::Receiver<bool>) {
    if *lease.cancel.borrow() || *shutdown.borrow() {
        return;
    }
    let request = match lease.config.request() {
        Ok(r) => r,
        Err(_) => {
            failure(&mut socket, "Invalid voice service configuration.").await;
            return;
        }
    };
    let limits = WebSocketConfig::default()
        .max_message_size(Some(MAX_PACKET))
        .max_frame_size(Some(MAX_PACKET));
    let connect = connect_async_with_config(request, Some(limits), false);
    let connected = tokio::select! {
        _=lease.cancel.changed()=>return, _=shutdown.changed()=>return,
        result=tokio::time::timeout(Duration::from_secs(15),connect)=>result,
    };
    let (upstream, _) = match connected {
        Ok(Ok(value)) => value,
        _ => {
            failure(&mut socket,"Could not connect to PersonaPlex. Check the service address, certificate and API key.").await;
            return;
        }
    };
    let (mut up_send, mut up_recv) = upstream.split();
    let (mut down_send, mut down_recv) = socket.split();
    let limit = tokio::time::sleep(MAX_CALL);
    tokio::pin!(limit);
    let deadline = tokio::time::sleep(READY_TIMEOUT);
    tokio::pin!(deadline);
    let mut ready = false;
    let mut rate_at = Instant::now();
    let mut rate_bytes = 0usize;
    let mut text_bytes = 0usize;
    let error = loop {
        tokio::select! {
            _=lease.cancel.changed()=>break None, _=shutdown.changed()=>break None,
            _=&mut limit=>break Some("Voice conversation reached its 30 minute limit."),
            _=&mut deadline=>break Some(if ready {"Voice service stopped sending audio. Start a new conversation."} else {"PersonaPlex did not become ready in time. It may be busy loading the model."}),
            message=down_recv.next()=>match message {
                Some(Ok(Message::Binary(data))) if ready && data.first()==Some(&1) && data.len()>1=>{
                    if rate_at.elapsed()>=Duration::from_secs(1) { rate_at=Instant::now();rate_bytes=0; }
                    rate_bytes=rate_bytes.saturating_add(data.len());
                    if data.len()>MAX_PACKET || rate_bytes>512*1024 { break Some("Microphone audio exceeded the streaming limit."); }
                    if !matches!(tokio::time::timeout(Duration::from_secs(2),up_send.send(UpMessage::Binary(data))).await,Ok(Ok(()))) { break Some("Voice service is not accepting audio."); }
                },
                Some(Ok(Message::Ping(_)|Message::Pong(_)))=>{},
                Some(Ok(Message::Close(_)))|None=>break None,
                _=>break Some("Invalid microphone audio stream."),
            },
            message=up_recv.next()=>match message {
                Some(Ok(UpMessage::Binary(data))) if valid_upstream(&data,ready)=>{
                    if data[0]==0 { ready=true; }
                    if data[0]==2 { text_bytes+=data.len();if text_bytes>256*1024 { break Some("Voice transcript exceeded its limit."); } }
                    deadline.as_mut().reset(tokio::time::Instant::now()+IDLE_TIMEOUT);
                    if !matches!(tokio::time::timeout(Duration::from_secs(2),down_send.send(Message::Binary(data))).await,Ok(Ok(()))) { break None; }
                },
                Some(Ok(UpMessage::Ping(data)))=>{ let _=tokio::time::timeout(Duration::from_secs(2),up_send.send(UpMessage::Pong(data))).await; },
                Some(Ok(UpMessage::Pong(_)))=>{},
                Some(Ok(UpMessage::Close(_)))|None=>break Some("PersonaPlex ended the conversation."),
                _=>break Some("PersonaPlex returned an invalid audio stream."),
            }
        }
    };
    if let Some(message) = error {
        let _ = tokio::time::timeout(
            Duration::from_secs(2),
            down_send.send(Message::Text(
                json!({"type":"error","message":message}).to_string().into(),
            )),
        )
        .await;
    }
    let _ = tokio::time::timeout(Duration::from_secs(2), up_send.close()).await;
    let _ = tokio::time::timeout(Duration::from_secs(2), down_send.close()).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configuration_rejects_unsafe_addresses_and_voice_paths() {
        for endpoint in [
            "https://example.com/api/chat",
            "ws://example.com/api/chat",
            "ws://u:p@localhost/api/chat",
            "wss://example.com/api/chat?token=secret",
            "ws://localhost/ws",
        ] {
            let config = Config {
                endpoint: endpoint.into(),
                ..Default::default()
            };
            assert!(config.validate().is_err(), "{endpoint}");
        }
        let mut config = Config {
            endpoint: "wss://voice.example.com/api/chat".into(),
            ..Default::default()
        };
        assert!(config.validate().is_ok());
        config.voice = "../../credentials.json".into();
        assert!(config.validate().is_err());
        for (packet, ready, valid) in [
            (vec![0], false, true),
            (vec![0], true, false),
            (vec![1, 3], false, false),
            (vec![1, 3], true, true),
            (vec![2, 255], true, false),
            (vec![9], true, false),
        ] {
            assert_eq!(valid_upstream(&packet, ready), valid);
        }
    }

    #[test]
    fn tickets_are_scoped_single_use_expiring_and_capacity_is_released() {
        let runtime = Arc::new(Runtime::default());
        let first = runtime.reserve("a".into(), Config::default()).unwrap();
        let id = first["id"].as_str().unwrap();
        let ticket = first["ticket"].as_str().unwrap();
        assert!(runtime.take("b", id, ticket).is_err());
        assert!(runtime.take("a", id, "wrong").is_err());
        let lease = runtime.take("a", id, ticket).unwrap();
        assert!(runtime.take("a", id, ticket).is_err());
        assert!(runtime.reserve("b".into(), Config::default()).is_err());
        runtime.cancel_scope("b");
        assert!(!*lease.cancel.borrow());
        runtime.cancel_scope("a");
        assert!(*lease.cancel.borrow());
        drop(lease);
        let second = runtime.reserve("b".into(), Config::default()).unwrap();
        runtime
            .calls
            .lock()
            .unwrap()
            .pending
            .get_mut(second["id"].as_str().unwrap())
            .unwrap()
            .expires = Instant::now() - Duration::from_secs(1);
        let third = runtime.reserve("c".into(), Config::default()).unwrap();
        assert!(
            runtime
                .take(
                    "b",
                    second["id"].as_str().unwrap(),
                    second["ticket"].as_str().unwrap()
                )
                .is_err()
        );
        runtime.cancel_scope("c");
        assert!(
            runtime
                .take(
                    "c",
                    third["id"].as_str().unwrap(),
                    third["ticket"].as_str().unwrap()
                )
                .is_err()
        );
    }

    #[tokio::test]
    async fn native_api_proxies_protocol_and_fences_secrets_revisions_and_workspaces() {
        async fn mock(
            Query(query): Query<HashMap<String, String>>,
            headers: HeaderMap,
            ws: WebSocketUpgrade,
        ) -> Response {
            assert_eq!(query["voice_prompt"], "NATF2.pt");
            assert_eq!(query["text_prompt"], "Synthetic English coach");
            assert!(!query.contains_key("api_key"));
            assert_eq!(headers["authorization"], "Bearer synthetic-secret");
            ws.on_upgrade(|mut socket| async move {
                socket.send(Message::Binary(vec![0].into())).await.unwrap();
                while let Some(Ok(Message::Binary(data))) = socket.next().await {
                    if socket.send(Message::Binary(data)).await.is_err() {
                        break;
                    }
                    if socket
                        .send(Message::Binary(b"\x02Hello learner".to_vec().into()))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
            })
        }
        let upstream = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let upstream_address = upstream.local_addr().unwrap();
        let upstream_task = tokio::spawn(async move {
            axum::serve(upstream, Router::new().route("/api/chat", get(mock)))
                .await
                .unwrap()
        });
        let dir = tempfile::tempdir().unwrap();
        let app = Application::open(dir.path()).unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server =
            tokio::spawn(axum::serve(listener, crate::api::router(app.clone())).into_future());
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        let base = format!("http://{address}");
        let input = json!({"expected_revision":0,"enabled":true,"endpoint":format!("ws://{upstream_address}/api/chat"),"voice":"NATF2.pt","prompt":"Synthetic English coach","api_key":"synthetic-secret"});
        let response = client
            .put(format!("{base}/api/voice/config?dt_workspace=a"))
            .json(&input)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        let value: Value = response.json().await.unwrap();
        assert_eq!(value["has_api_key"], true);
        assert!(!value.to_string().contains("synthetic-secret"));
        let path = Config::path(&app.settings, "a").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        assert_eq!(
            client
                .put(format!("{base}/api/voice/config?dt_workspace=a"))
                .json(&input)
                .send()
                .await
                .unwrap()
                .status(),
            409
        );
        let other: Value = client
            .get(format!("{base}/api/voice?dt_workspace=b"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(other["config"]["enabled"], false);
        assert_eq!(other["config"]["has_api_key"], false);
        let reservation: Value = client
            .post(format!("{base}/api/voice/sessions?dt_workspace=a"))
            .json(&json!({"expected_revision":1}))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let id = reservation["id"].as_str().unwrap();
        let ticket = reservation["ticket"].as_str().unwrap();
        let wrong = format!("ws://{address}/ws/voice/{id}?dt_workspace=b&ticket={ticket}");
        assert!(tokio_tungstenite::connect_async(wrong).await.is_err());
        let url = format!("ws://{address}/ws/voice/{id}?dt_workspace=a&ticket={ticket}");
        let (mut socket, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
        assert_eq!(
            socket.next().await.unwrap().unwrap().into_data().as_ref(),
            [0]
        );
        assert!(tokio_tungstenite::connect_async(&url).await.is_err());
        socket
            .send(UpMessage::Binary(b"\x01OggSsynthetic".to_vec().into()))
            .await
            .unwrap();
        assert_eq!(
            socket.next().await.unwrap().unwrap().into_data().as_ref(),
            b"\x01OggSsynthetic"
        );
        assert_eq!(
            socket.next().await.unwrap().unwrap().into_data().as_ref(),
            b"\x02Hello learner"
        );
        assert_eq!(
            client
                .post(format!(
                    "{base}/api/voice/sessions/{id}/cancel?dt_workspace=b"
                ))
                .send()
                .await
                .unwrap()
                .status(),
            404
        );
        assert_eq!(
            client
                .post(format!(
                    "{base}/api/voice/sessions/{id}/cancel?dt_workspace=a"
                ))
                .send()
                .await
                .unwrap()
                .status(),
            200
        );
        let _ = tokio::time::timeout(Duration::from_secs(3), socket.next())
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                if app.voice.capacity.available_permits() == 1 {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(
            app.store
                .connect()
                .unwrap()
                .query_row("SELECT COUNT(*) FROM native_tool_invocations", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
        app.shutdown().await;
        server.abort();
        upstream_task.abort();
    }
}
