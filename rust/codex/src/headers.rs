//! Wire contract checked against zyycn/codex-proxy-rs at
//! v3.39.1, providers/openai/transport/headers.rs.
//! HTTP SSE and upstream WebSocket headers are different contracts.

use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use serde::{Deserialize, Serialize};

use crate::Error;

pub const CLIENT_VERSION: &str = "0.153.4";

/// Application-owned correlation. Never populate this from model output or
/// untrusted browser headers. Deliberately no Debug: turn_state is opaque.
#[derive(Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct RequestIdentity {
    pub request_id: String,
    pub installation_id: Option<String>,
    pub session_id: Option<String>,
    pub thread_id: Option<String>,
    pub turn_id: Option<String>,
    pub traceparent: Option<String>,
    pub turn_state: Option<String>,
}

impl Default for RequestIdentity {
    fn default() -> Self {
        Self {
            request_id: uuid::Uuid::new_v4().to_string(),
            installation_id: None,
            session_id: None,
            thread_id: None,
            turn_id: None,
            traceparent: None,
            turn_state: None,
        }
    }
}

pub const MAX_TURN_STATE_BYTES: usize = 4096;

pub fn valid_turn_state(value: &str) -> bool {
    !value.trim().is_empty()
        && value.len() <= MAX_TURN_STATE_BYTES
        && value.bytes().all(|b| (b' '..=b'~').contains(&b))
}

fn valid_traceparent(value: &str) -> bool {
    let parts = value.split('-').collect::<Vec<_>>();
    parts.len() == 4
        && parts[0] == "00"
        && parts[1].len() == 32
        && parts[2].len() == 16
        && matches!(parts[3], "00" | "01")
        && parts[1..3].iter().all(|part| {
            part.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                && part.bytes().any(|b| b != b'0')
        })
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct ClientProfile {
    pub originator: String,
    pub version: String,
    pub user_agent: String,
}

impl Default for ClientProfile {
    fn default() -> Self {
        Self {
            originator: "codex_cli_rs".into(),
            version: CLIENT_VERSION.into(),
            user_agent: format!(
                "codex_cli_rs/{CLIENT_VERSION} ({}; {}) Lorevia",
                std::env::consts::OS,
                std::env::consts::ARCH
            ),
        }
    }
}

impl ClientProfile {
    pub fn validate(&self) -> Result<(), Error> {
        let parts: Vec<_> = self.version.split('.').collect();
        if parts.len() != 3
            || parts.iter().any(|part| {
                part.is_empty()
                    || !part.bytes().all(|b| b.is_ascii_digit())
                    || (part.len() > 1 && part.starts_with('0'))
            })
            || self.originator.is_empty()
            || self.originator.len() > 128
            || self.user_agent.len() > 4096
            || !self
                .user_agent
                .starts_with(&format!("{}/{} ", self.originator, self.version))
        {
            return Err(Error::InvalidRequest);
        }
        Ok(())
    }
}

/// Construct identity from the authenticated caller, never from browser headers.
pub fn build_headers(
    profile: &ClientProfile,
    access_token: &str,
    account_id: &str,
    session_id: Option<&str>,
    model: &str,
    service_tier: Option<&str>,
) -> Result<HeaderMap, Error> {
    build_request_headers(
        profile,
        access_token,
        account_id,
        model,
        service_tier,
        &RequestIdentity {
            session_id: session_id.filter(|s| !s.is_empty()).map(str::to_owned),
            thread_id: session_id.filter(|s| !s.is_empty()).map(str::to_owned),
            ..Default::default()
        },
    )
}

pub(crate) fn build_request_headers(
    profile: &ClientProfile,
    access_token: &str,
    account_id: &str,
    model: &str,
    service_tier: Option<&str>,
    identity: &RequestIdentity,
) -> Result<HeaderMap, Error> {
    profile.validate()?;
    if access_token.trim().is_empty()
        || model.trim().is_empty()
        || uuid::Uuid::parse_str(&identity.request_id).is_err()
        || identity
            .installation_id
            .as_ref()
            .is_some_and(|id| uuid::Uuid::parse_str(id).is_err())
        || identity
            .turn_state
            .as_deref()
            .is_some_and(|s| !valid_turn_state(s))
        || identity
            .traceparent
            .as_deref()
            .is_some_and(|s| !valid_traceparent(s))
    {
        return Err(Error::InvalidRequest);
    }
    let mut headers = HeaderMap::new();
    let mut insert = |name: &'static str, value: &str| -> Result<(), Error> {
        let mut header = HeaderValue::from_str(value).map_err(|_| Error::InvalidRequest)?;
        if matches!(
            name,
            "authorization" | "chatgpt-account-id" | "x-codex-turn-state"
        ) {
            header.set_sensitive(true);
        }
        headers.insert(HeaderName::from_static(name), header);
        Ok(())
    };
    insert("authorization", &format!("Bearer {access_token}"))?;
    insert("originator", &profile.originator)?;
    insert("user-agent", &profile.user_agent)?;
    insert("version", &profile.version)?;
    insert("accept", "text/event-stream")?;
    insert("content-type", "application/json")?;
    if !account_id.is_empty() {
        insert("chatgpt-account-id", account_id)?;
    }
    insert("x-client-request-id", &identity.request_id)?;
    for (name, value) in [
        (
            "x-codex-installation-id",
            identity.installation_id.as_deref(),
        ),
        ("session-id", identity.session_id.as_deref()),
        ("thread-id", identity.thread_id.as_deref()),
        ("x-codex-turn-state", identity.turn_state.as_deref()),
        ("traceparent", identity.traceparent.as_deref()),
    ] {
        if let Some(value) = value {
            if value.is_empty() || value.len() > MAX_TURN_STATE_BYTES {
                return Err(Error::InvalidRequest);
            }
            insert(name, value)?;
        }
    }
    if let Some(turn_id) = &identity.turn_id {
        if turn_id.is_empty() || turn_id.len() > 256 {
            return Err(Error::InvalidRequest);
        }
        let metadata =
            serde_json::json!({"turn_id":turn_id,"installation_id":identity.installation_id});
        insert("x-codex-turn-metadata", &metadata.to_string())?;
    }
    let mut routing = format!("model={model}");
    if let Some(tier) = service_tier.filter(|value| !value.is_empty()) {
        routing.push_str(&format!(";tier={tier}"));
    }
    insert("x-codex-routing-hint", &routing)?;
    Ok(headers)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_in_one_session_have_distinct_request_ids() {
        let build = || {
            build_headers(
                &ClientProfile::default(),
                "synthetic",
                "account",
                Some("session"),
                "model",
                None,
            )
            .unwrap()
        };
        let first = build();
        let second = build();
        assert_ne!(first["x-client-request-id"], second["x-client-request-id"]);
        assert_eq!(first["session-id"], second["session-id"]);
        assert_ne!(first["x-client-request-id"], first["session-id"]);
    }

    #[test]
    fn opaque_state_is_sensitive_and_invalid_context_is_rejected() {
        let mut identity = RequestIdentity {
            turn_state: Some("upstream-only".into()),
            ..Default::default()
        };
        let build = |id: &RequestIdentity| {
            build_request_headers(
                &ClientProfile::default(),
                "synthetic",
                "account",
                "model",
                None,
                id,
            )
        };
        assert!(build(&identity).unwrap()["x-codex-turn-state"].is_sensitive());
        for bad in ["", "\r\nforged: value", "é"] {
            identity.turn_state = Some(bad.into());
            assert!(matches!(build(&identity), Err(Error::InvalidRequest)));
        }
        identity.turn_state = Some("a".repeat(MAX_TURN_STATE_BYTES + 1));
        assert!(build(&identity).is_err());
        identity.turn_state = None;
        identity.traceparent =
            Some("00-00000000000000000000000000000000-1111111111111111-00".into());
        assert!(build(&identity).is_err());
    }
}
