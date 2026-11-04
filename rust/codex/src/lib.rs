//! Native Codex HTTP transport, called in-process by the application.
//! No credential storage or automatic retries. Authentication is request-local.

pub mod headers;
pub mod sse;

use std::{future::Future, time::Duration};

use futures_util::StreamExt;
use reqwest::{Client, Url, redirect::Policy};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use headers::{ClientProfile, RequestIdentity, build_request_headers, valid_turn_state};

pub const RESPONSES_URL: &str = "https://chatgpt.com/backend-api/codex/responses";

/// Deliberately no Debug: the envelope carries a bearer token and conversation.
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub access_token: String,
    #[serde(default)]
    pub account_id: String,
    pub body: Value,
    #[serde(default)]
    pub profile: ClientProfile,
    #[serde(default)]
    pub identity: RequestIdentity,
}

impl Request {
    /// Idempotent so callers can budget the final body before starting I/O.
    pub fn apply_identity(&mut self) -> Result<(), Error> {
        apply_identity(self)
    }
}

/// Reported before status/stream validation, including on failed HTTP replies.
/// Deliberately no Debug: the opaque state must not enter diagnostics.
pub struct ResponseMetadata {
    pub status: u16,
    pub turn_state: Option<String>,
}

impl ResponseMetadata {
    fn from_response(response: &reqwest::Response) -> Self {
        let mut values = response.headers().get_all("x-codex-turn-state").iter();
        let turn_state = values
            .next()
            .and_then(|v| v.to_str().ok())
            .filter(|v| valid_turn_state(v))
            .map(str::to_owned)
            .filter(|_| values.next().is_none());
        Self {
            status: response.status().as_u16(),
            turn_state,
        }
    }
}

/// Error messages exclude URLs, bodies and tokens.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Invalid Codex request or client profile.")]
    InvalidRequest,
    #[error("Codex transport request failed.")]
    Transport,
    #[error("Codex returned HTTP {0}.")]
    Http(u16),
    #[error("Codex returned an invalid event stream.")]
    InvalidStream,
    #[error("Codex stream exceeded the event size limit.")]
    StreamTooLarge,
    #[error("Codex stream ended before a terminal response.")]
    TruncatedStream,
    #[error("Codex response failed.")]
    UpstreamEvent,
    #[error("Codex output consumer disconnected.")]
    OutputClosed,
}

pub struct CodexClient {
    client: Client,
    endpoint: Url,
}

impl CodexClient {
    pub fn new(verify_tls: bool) -> Result<Self, Error> {
        // Keep verified and explicitly unverified transports in separate pools.
        static VERIFIED: std::sync::OnceLock<Result<Client, reqwest::Error>> =
            std::sync::OnceLock::new();
        static UNVERIFIED: std::sync::OnceLock<Result<Client, reqwest::Error>> =
            std::sync::OnceLock::new();
        let cached = if verify_tls { &VERIFIED } else { &UNVERIFIED };
        Ok(Self {
            client: cached
                .get_or_init(|| {
                    Client::builder()
                        .redirect(Policy::none())
                        .connect_timeout(Duration::from_secs(20))
                        .read_timeout(Duration::from_secs(60))
                        .danger_accept_invalid_certs(!verify_tls)
                        .build()
                })
                .as_ref()
                .cloned()
                .map_err(|_| Error::Transport)?,
            endpoint: Url::parse(RESPONSES_URL).expect("constant URL"),
        })
    }

    /// Send once. OAuth recovery and deciding whether to retry belong to the application.
    pub async fn stream<F, Fut>(&self, request: Request, emit: F) -> Result<(), Error>
    where
        F: FnMut(Value) -> Fut,
        Fut: Future<Output = Result<(), Error>>,
    {
        self.stream_with_metadata(request, |_| {}, emit).await
    }

    pub async fn stream_with_metadata<F, Fut>(
        &self,
        mut request: Request,
        mut observe: impl FnMut(ResponseMetadata),
        mut emit: F,
    ) -> Result<(), Error>
    where
        F: FnMut(Value) -> Fut,
        Fut: Future<Output = Result<(), Error>>,
    {
        apply_identity(&mut request)?;
        let body = &request.body;
        let model = body
            .get("model")
            .and_then(Value::as_str)
            .ok_or(Error::InvalidRequest)?;
        if body.get("stream") != Some(&Value::Bool(true))
            || body.get("store") != Some(&Value::Bool(false))
            || !body.get("input").is_some_and(Value::is_array)
            || body.get("previous_response_id").is_some()
        {
            return Err(Error::InvalidRequest);
        }
        let headers = build_request_headers(
            &request.profile,
            &request.access_token,
            &request.account_id,
            model,
            body.get("service_tier").and_then(Value::as_str),
            &request.identity,
        )?;
        let response = self
            .client
            .post(self.endpoint.clone())
            .headers(headers)
            .json(body)
            .send()
            .await
            .map_err(|_| Error::Transport)?;
        observe(ResponseMetadata::from_response(&response));
        if response.status().as_u16() != 200 {
            return Err(Error::Http(response.status().as_u16()));
        }
        if !response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| {
                v.split(';')
                    .next()
                    .unwrap_or("")
                    .trim()
                    .eq_ignore_ascii_case("text/event-stream")
            })
        {
            return Err(Error::InvalidStream);
        }
        let mut bytes = response.bytes_stream();
        let mut decoder = sse::Decoder::default();
        while let Some(chunk) = bytes.next().await {
            let chunk = chunk.map_err(|_| Error::Transport)?;
            for event in decoder.push(&chunk)? {
                let terminal = inspect_event(&event)?;
                emit(event).await?;
                if terminal {
                    return Ok(());
                }
            }
        }
        for event in decoder.finish()? {
            let terminal = inspect_event(&event)?;
            emit(event).await?;
            if terminal {
                return Ok(());
            }
        }
        Err(Error::TruncatedStream)
    }
}

fn apply_identity(request: &mut Request) -> Result<(), Error> {
    let identity = &request.identity;
    let body = request.body.as_object_mut().ok_or(Error::InvalidRequest)?;
    if let Some(session) = &identity.session_id {
        body.insert("prompt_cache_key".into(), Value::String(session.clone()));
    } else if body.get("prompt_cache_key").and_then(Value::as_str) == Some("") {
        body.remove("prompt_cache_key");
    }
    // Native callers own these fields. Do not preserve a different opaque token
    // or installation alias from an otherwise valid full request body.
    for key in [
        "turnState",
        "turn_state",
        "turnMetadata",
        "turn_metadata",
        "turn_id",
        "x-codex-turn-state",
        "x-codex-turn-metadata",
        "x-codex-installation-id",
        "installation_id",
        "installationId",
    ] {
        body.remove(key);
    }
    let metadata = body
        .entry("client_metadata")
        .or_insert_with(|| serde_json::json!({}));
    let metadata = metadata.as_object_mut().ok_or(Error::InvalidRequest)?;
    for key in [
        "turnState",
        "turn_state",
        "turnMetadata",
        "turn_metadata",
        "installation_id",
        "installationId",
        "turn_id",
        "x-codex-turn-metadata",
    ] {
        metadata.remove(key);
    }
    for (key, value) in [
        ("x-codex-installation-id", &identity.installation_id),
        ("x-codex-turn-state", &identity.turn_state),
        ("turn_id", &identity.turn_id),
    ] {
        if let Some(value) = value {
            metadata.insert(key.into(), Value::String(value.clone()));
        } else {
            metadata.remove(key);
        }
    }
    Ok(())
}

fn inspect_event(event: &Value) -> Result<bool, Error> {
    match event["type"].as_str() {
        Some("error" | "response.failed" | "response.cancelled") => Err(Error::UpstreamEvent),
        Some("response.completed" | "response.incomplete") => {
            if matches!(
                event["response"]["status"].as_str(),
                Some("failed" | "cancelled")
            ) {
                return Err(Error::UpstreamEvent);
            }
            Ok(true)
        }
        _ => Ok(false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn request() -> Request {
        Request {
            access_token: "synthetic-only".into(),
            account_id: "synthetic-account".into(),
            body: json!({"model":"synthetic","stream":true,"store":false,"input":[],
                "client_metadata":{"installation_id":"wrong","x-codex-turn-state":"unowned"}}),
            profile: Default::default(),
            identity: RequestIdentity {
                installation_id: Some(uuid::Uuid::new_v4().to_string()),
                session_id: Some("scoped-session".into()),
                thread_id: Some("scoped-branch".into()),
                turn_id: Some("admitted-turn".into()),
                turn_state: Some("verified-token".into()),
                ..Default::default()
            },
        }
    }

    #[test]
    fn rebuilding_a_full_body_drops_unowned_state_and_is_idempotent() {
        let mut request = request();
        request.identity.turn_state = None;
        request.body["turn_id"] = json!("forged-turn");
        request.body["turn_metadata"] = json!({"x-codex-turn-state":"forged"});
        request.body["client_metadata"]["turnMetadata"] = json!({"turn_state":"forged"});
        request.body["client_metadata"]["unrelated"] = json!("preserved");
        let input = json!([{"role":"user","content":"plain user input"}]);
        request.body["input"] = input.clone();
        request.apply_identity().unwrap();
        assert!(request.body.get("turn_id").is_none());
        assert!(request.body.get("turn_metadata").is_none());
        assert!(
            request.body["client_metadata"]
                .get("turnMetadata")
                .is_none()
        );
        assert!(
            request.body["client_metadata"]
                .get("x-codex-turn-state")
                .is_none()
        );
        assert_eq!(request.body["client_metadata"]["turn_id"], "admitted-turn");
        assert_eq!(request.body["client_metadata"]["unrelated"], "preserved");
        assert_eq!(request.body["input"], input);
        let body = request.body.clone();
        request.apply_identity().unwrap();
        assert_eq!(request.body, body);
    }

    #[tokio::test]
    async fn turn_state_does_not_authorize_response_id_continuation() {
        let client = CodexClient {
            client: Client::builder().no_proxy().build().unwrap(),
            endpoint: Url::parse("http://127.0.0.1:1/responses").unwrap(),
        };
        let mut request = request();
        request.body["previous_response_id"] = json!("unowned-response");
        let result = client
            .stream_with_metadata(
                request,
                |_| panic!("must fail before I/O"),
                |_| async { Ok(()) },
            )
            .await;
        assert!(matches!(result, Err(Error::InvalidRequest)));
    }

    async fn fixture(
        status: u16,
        headers: &str,
        body: &str,
    ) -> (CodexClient, tokio::task::JoinHandle<String>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = Url::parse(&format!(
            "http://{}/responses",
            listener.local_addr().unwrap()
        ))
        .unwrap();
        let reply = format!(
            "HTTP/1.1 {status} Fixture\r\n{headers}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut data = Vec::new();
            loop {
                let mut buffer = [0; 8192];
                let n = socket.read(&mut buffer).await.unwrap();
                assert_ne!(n, 0);
                data.extend_from_slice(&buffer[..n]);
                if let Some(end) = data.windows(4).position(|w| w == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&data[..end]).to_lowercase();
                    let length = headers
                        .lines()
                        .find_map(|s| s.strip_prefix("content-length: "))
                        .unwrap()
                        .parse::<usize>()
                        .unwrap();
                    if data.len() >= end + 4 + length {
                        break;
                    }
                }
            }
            socket.write_all(reply.as_bytes()).await.unwrap();
            String::from_utf8(data).unwrap()
        });
        let client = CodexClient {
            client: Client::builder()
                .no_proxy()
                .timeout(Duration::from_secs(5))
                .redirect(Policy::none())
                .build()
                .unwrap(),
            endpoint,
        };
        (client, server)
    }

    #[tokio::test]
    async fn failed_http_reply_reports_state_before_returning_error() {
        let (client, server) = fixture(429, "x-codex-turn-state: failure-token\r\n", "").await;
        let mut seen = None;
        let result = client
            .stream_with_metadata(
                request(),
                |m| seen = Some(m),
                |_| async { panic!("failed HTTP must emit no model event") },
            )
            .await;
        assert!(matches!(result, Err(Error::Http(429))));
        let metadata = seen.unwrap();
        assert_eq!(metadata.status, 429);
        assert_eq!(metadata.turn_state.as_deref(), Some("failure-token"));
        server.await.unwrap();
    }

    #[tokio::test]
    async fn wire_identity_matches_body_and_full_request_does_not_restore_response_id() {
        let (client, server) = fixture(200, "Content-Type: text/event-stream\r\nx-codex-turn-state: next-token\r\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"output\":[]}}\n\n").await;
        let request = request();
        let installation = request.identity.installation_id.clone().unwrap();
        let mut seen = None;
        client
            .stream_with_metadata(request, |m| seen = Some(m), |_| async { Ok(()) })
            .await
            .unwrap();
        assert_eq!(seen.unwrap().turn_state.as_deref(), Some("next-token"));
        let wire = server.await.unwrap();
        let (headers, body) = wire.split_once("\r\n\r\n").unwrap();
        let body: Value = serde_json::from_str(body).unwrap();
        assert!(headers.contains(&format!("x-codex-installation-id: {installation}")));
        assert!(headers.contains("x-codex-turn-state: verified-token"));
        assert!(headers.contains("thread-id: scoped-branch"));
        assert_eq!(
            body["client_metadata"]["x-codex-installation-id"],
            installation
        );
        assert_eq!(
            body["client_metadata"]["x-codex-turn-state"],
            "verified-token"
        );
        assert!(body["client_metadata"].get("installation_id").is_none());
        assert!(body.get("previous_response_id").is_none());
        assert_eq!(body["prompt_cache_key"], "scoped-session");
    }

    #[tokio::test]
    async fn truncated_stream_retains_metadata_without_claiming_completion() {
        let (client, server) = fixture(
            200,
            "Content-Type: text/event-stream\r\nx-codex-turn-state: observed-token\r\n",
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"partial\"}\n\n",
        )
        .await;
        let mut seen = None;
        let result = client
            .stream_with_metadata(request(), |m| seen = Some(m), |_| async { Ok(()) })
            .await;
        assert!(matches!(result, Err(Error::TruncatedStream)));
        assert_eq!(seen.unwrap().turn_state.as_deref(), Some("observed-token"));
        server.await.unwrap();
    }

    #[tokio::test]
    async fn ambiguous_or_oversized_upstream_tokens_are_not_retained() {
        for headers in [
            "x-codex-turn-state: first\r\nx-codex-turn-state: second\r\n".to_owned(),
            format!(
                "x-codex-turn-state: {}\r\n",
                "a".repeat(headers::MAX_TURN_STATE_BYTES + 1)
            ),
        ] {
            let (client, server) = fixture(503, &headers, "").await;
            let mut seen = None;
            assert!(
                client
                    .stream_with_metadata(request(), |m| seen = Some(m), |_| async { Ok(()) })
                    .await
                    .is_err()
            );
            assert!(seen.unwrap().turn_state.is_none());
            server.await.unwrap();
        }
    }
}
