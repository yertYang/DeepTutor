//! Private, home-local Codex identity and bounded upstream turn state.
//! State is an optional transport hint: restart/expiry falls back to full input,
//! and never authorizes replaying a model request or a tool invocation.
use crate::{
    Error, Result,
    config::{Settings, read_json, safe_path, write_json},
    store::{Admission, text},
};
use deeptutor_codex::headers::{RequestIdentity, valid_turn_state};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

const MAX_STATES: usize = 256;
const RETENTION: Duration = Duration::from_secs(30 * 60);

#[derive(Clone)]
pub(crate) struct Context {
    pub workspace: String,
    pub admission: Admission,
}

struct Entry {
    token: String,
    expires: Instant,
}

#[derive(Default)]
pub(crate) struct Runtime {
    states: Mutex<HashMap<String, Entry>>,
}

pub(crate) struct Prepared {
    pub identity: RequestIdentity,
    pub scope: Option<String>,
}

fn digest(label: &str, parts: &[&str]) -> String {
    let mut hash = Sha256::new();
    for part in std::iter::once(label).chain(parts.iter().copied()) {
        hash.update((part.len() as u64).to_le_bytes());
        hash.update(part.as_bytes());
    }
    format!("{:x}", hash.finalize())
}

impl Runtime {
    pub fn prepare(
        &self,
        account: &str,
        generation: u64,
        installation: String,
        model: &str,
        context: Option<&Context>,
    ) -> Prepared {
        let mut identity = RequestIdentity {
            installation_id: Some(installation.clone()),
            ..Default::default()
        };
        let scope = context.map(|context| {
            let admission = &context.admission;
            let session = digest(
                "session",
                &[&installation, &context.workspace, &admission.session],
            );
            let thread = digest("thread", &[&session, &admission.user_message.to_string()]);
            let scope = digest(
                "turn",
                &[
                    account,
                    &generation.to_string(),
                    &installation,
                    model,
                    &context.workspace,
                    &admission.session,
                    &thread,
                    &admission.turn,
                ],
            );
            identity.session_id = Some(session);
            identity.thread_id = Some(thread);
            identity.turn_id = Some(admission.turn.clone());
            identity.turn_state = self.get_at(&scope, Instant::now());
            scope
        });
        // All requests in an admitted turn share a trace; every HTTP attempt
        // gets its own request ID and span. Auxiliary requests are independent.
        let trace = scope
            .as_deref()
            .map(|s| digest("trace", &[s]))
            .unwrap_or_else(|| uuid::Uuid::new_v4().simple().to_string());
        let span = uuid::Uuid::new_v4().simple().to_string();
        identity.traceparent = Some(format!("00-{}-{}-00", &trace[..32], &span[..16]));
        Prepared { identity, scope }
    }

    fn get_at(&self, scope: &str, now: Instant) -> Option<String> {
        let mut states = self.states.lock().ok()?;
        states.retain(|_, entry| entry.expires > now);
        states.get(scope).map(|entry| entry.token.clone())
    }

    pub fn remember(&self, scope: Option<&str>, token: Option<&str>) {
        if let (Some(scope), Some(token)) = (scope, token) {
            self.remember_at(scope, token, Instant::now());
        }
    }

    fn remember_at(&self, scope: &str, token: &str, now: Instant) {
        if !valid_turn_state(token) {
            return;
        }
        let Ok(mut states) = self.states.lock() else {
            return;
        };
        states.retain(|_, entry| entry.expires > now);
        if states.len() >= MAX_STATES
            && !states.contains_key(scope)
            && let Some(oldest) = states
                .iter()
                .min_by_key(|(_, e)| e.expires)
                .map(|(k, _)| k.clone())
        {
            states.remove(&oldest);
        }
        states.insert(
            scope.into(),
            Entry {
                token: token.into(),
                expires: now + RETENTION,
            },
        );
    }
}

/// Reuse the OAuth cross-process lock, including its generation fence. The
/// account hash names the file; neither credentials nor raw account IDs are
/// stored with installation identity. Logout leaves this identity intact.
pub(crate) async fn installation(settings: Arc<Settings>, credentials: &Value) -> Result<String> {
    let account = text(credentials, "account_id").to_owned();
    let generation = credentials["generation"]
        .as_u64()
        .ok_or_else(|| Error::conflict("Invalid Codex credential generation."))?;
    crate::application::blocking(move || {
        crate::oauth_web::locked(&settings, |root, state| {
            if account.is_empty()
                || state["generation"] != generation
                || state["rejected_generation"] == generation
            {
                return Err(Error::conflict("Codex account changed before the request."));
            }
            let current = read_json(&root.join("credentials.v1.json"), Value::Null)?;
            if text(&current, "account_id") != account || current["generation"] != generation {
                return Err(Error::conflict("Codex account changed before the request."));
            }
            let path = root.join(format!(
                "installation-{}.v1.json",
                digest("installation", &[&account])
            ));
            safe_path(&settings.user_root, &path)?;
            let saved = read_json(&path, Value::Null)?;
            if path.try_exists()? {
                let id = text(&saved, "installation_id");
                if saved["schema_version"] != 1 || uuid::Uuid::parse_str(id).is_err() {
                    return Err(Error::conflict("Invalid Codex installation identity."));
                }
                return Ok(id.into());
            }
            let id = uuid::Uuid::new_v4().to_string();
            write_json(&path, &json!({"schema_version":1,"installation_id":id}))?;
            Ok(id)
        })
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context() -> Context {
        Context {
            workspace: "workspace-a".into(),
            admission: Admission {
                session: "session-a".into(),
                turn: "turn-a".into(),
                user_message: 12,
                replay: false,
                capability: Default::default(),
            },
        }
    }

    #[test]
    fn state_reuse_requires_matching_account_generation_model_workspace_branch_and_turn() {
        let runtime = Runtime::default();
        let installation = uuid::Uuid::new_v4().to_string();
        let context = context();
        let prepare = |account, generation, model, context: &Context| {
            runtime.prepare(
                account,
                generation,
                installation.clone(),
                model,
                Some(context),
            )
        };
        let first = prepare("account-a", 1, "model-a", &context);
        runtime.remember(first.scope.as_deref(), Some("observed-only"));
        let next = prepare("account-a", 1, "model-a", &context);
        assert_eq!(next.identity.turn_state.as_deref(), Some("observed-only"));
        assert_ne!(first.identity.request_id, next.identity.request_id);
        assert_eq!(
            &first.identity.traceparent.as_ref().unwrap()[3..35],
            &next.identity.traceparent.as_ref().unwrap()[3..35]
        );
        assert_ne!(first.identity.traceparent, next.identity.traceparent);
        for (account, generation, model) in [
            ("account-b", 1, "model-a"),
            ("account-a", 2, "model-a"),
            ("account-a", 1, "model-b"),
        ] {
            assert!(
                prepare(account, generation, model, &context)
                    .identity
                    .turn_state
                    .is_none()
            );
        }
        for field in ["workspace", "session", "branch", "turn"] {
            let mut other = context.clone();
            match field {
                "workspace" => other.workspace = "workspace-b".into(),
                "session" => other.admission.session = "session-b".into(),
                "branch" => other.admission.user_message = 13,
                _ => other.admission.turn = "turn-b".into(),
            }
            assert!(
                prepare("account-a", 1, "model-a", &other)
                    .identity
                    .turn_state
                    .is_none(),
                "{field}"
            );
        }
        assert!(
            runtime
                .prepare(
                    "account-a",
                    1,
                    uuid::Uuid::new_v4().to_string(),
                    "model-a",
                    Some(&context)
                )
                .identity
                .turn_state
                .is_none()
        );
        let auxiliary = runtime.prepare("account-a", 1, installation, "model-a", None);
        assert!(auxiliary.scope.is_none());
        assert!(auxiliary.identity.turn_state.is_none());
        assert!(auxiliary.identity.session_id.is_none());
    }

    #[test]
    fn states_expire_and_memory_is_bounded_even_with_many_turns() {
        let runtime = Runtime::default();
        let start = Instant::now();
        runtime.remember_at("old", "old-token", start);
        for i in 0..MAX_STATES {
            runtime.remember_at(
                &format!("scope-{i}"),
                "token",
                start + Duration::from_millis((i + 1) as u64),
            );
        }
        assert_eq!(runtime.states.lock().unwrap().len(), MAX_STATES);
        assert!(runtime.get_at("old", start).is_none());
        assert_eq!(runtime.get_at("scope-0", start).as_deref(), Some("token"));
        assert!(
            runtime
                .get_at("scope-0", start + RETENTION + Duration::from_secs(1))
                .is_none()
        );
        assert!(runtime.states.lock().unwrap().is_empty());
        for token in ["\r\nforged", "", &"a".repeat(4097)] {
            runtime.remember_at("invalid", token, start);
        }
        assert!(runtime.states.lock().unwrap().is_empty());
    }

    fn sign_in(settings: &Settings, account: &str, generation: u64) -> Value {
        let credentials = json!({"schema_version":1,"account_id":account,"generation":generation,
            "access_token":"synthetic-only","expires_at":crate::store::now()+3600.0});
        crate::oauth_web::locked(settings, |root, _| {
            write_json(
                &root.join("state.v1.json"),
                &json!({"schema_version":1,"generation":generation}),
            )?;
            write_json(&root.join("credentials.v1.json"), &credentials)
        })
        .unwrap();
        credentials
    }

    #[tokio::test]
    async fn installation_survives_restart_and_logout_and_is_isolated_per_account_and_home() {
        let dir = tempfile::tempdir().unwrap();
        let settings = Arc::new(Settings::open(dir.path()).unwrap());
        let a = sign_in(&settings, "account-a", 1);
        let (one, two) = tokio::join!(
            installation(settings.clone(), &a),
            installation(settings.clone(), &a)
        );
        let one = one.unwrap();
        assert_eq!(one, two.unwrap());
        drop(settings);
        let settings = Arc::new(Settings::open(dir.path()).unwrap());
        assert_eq!(one, installation(settings.clone(), &a).await.unwrap());
        let b = sign_in(&settings, "account-b", 2);
        assert!(installation(settings.clone(), &a).await.is_err());
        let other = installation(settings.clone(), &b).await.unwrap();
        assert_ne!(one, other);
        crate::oauth_web::logout(&settings).unwrap();
        assert!(installation(settings.clone(), &b).await.is_err());
        let a_again = sign_in(&settings, "account-a", 4);
        assert_eq!(one, installation(settings.clone(), &a_again).await.unwrap());
        let second_dir = tempfile::tempdir().unwrap();
        let second_home = Arc::new(Settings::open(second_dir.path()).unwrap());
        let a = sign_in(&second_home, "account-a", 1);
        assert_ne!(one, installation(second_home, &a).await.unwrap());
        let root = settings.user_root.join("private/openai-codex");
        let path = std::fs::read_dir(&root)
            .unwrap()
            .map(|e| e.unwrap().path())
            .find(|p| {
                p.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("installation-")
                    && std::fs::read_to_string(p).unwrap().contains(&one)
            })
            .unwrap();
        let persisted = std::fs::read_to_string(&path).unwrap();
        assert!(!persisted.contains("account-a"));
        assert!(!persisted.contains("synthetic-only"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        write_json(&path, &Value::Null).unwrap();
        assert!(installation(settings, &a_again).await.is_err());
    }
}
