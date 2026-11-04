//! Serialized, cancellable browser processes, scoped to a database and chat.
use super::*;
use std::sync::{Arc, OnceLock};
use tokio::io::{AsyncBufReadExt, BufReader};

struct Session {
    child: Option<tokio::process::Child>,
    stdin: Option<tokio::process::ChildStdin>,
    stdout: BufReader<tokio::process::ChildStdout>,
    leaf: i64,
    used: std::time::Instant,
    revision: u64,
}
impl Drop for Session {
    fn drop(&mut self) {
        // EOF lets the worker close Chromium before Node is reaped. Never kill
        // Node first: Chromium would otherwise survive a cancelled tool call.
        drop(self.stdin.take());
        if let Some(mut child) = self.child.take()
            && let Ok(runtime) = tokio::runtime::Handle::try_current()
        {
            ACTIVE.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            runtime.spawn(async move {
                let _active = Active;
                if tokio::time::timeout(Duration::from_secs(4), child.wait())
                    .await
                    .is_err()
                {
                    let _ = child.kill().await;
                }
            });
        }
    }
}
type Slot = Arc<Mutex<Option<Session>>>;
static SESSIONS: OnceLock<Mutex<HashMap<String, Slot>>> = OnceLock::new();
fn sessions() -> &'static Mutex<HashMap<String, Slot>> {
    SESSIONS.get_or_init(Default::default)
}

pub async fn shutdown() {
    // Dropping stdin asks Node to close Chromium; kill_on_drop reaps Node.
    sessions().lock().await.clear();
}

pub async fn request(
    store: &Store,
    admission: &crate::store::Admission,
    request: Value,
) -> Result<Value> {
    let key = serde_json::to_string(&(store.path.as_os_str(), &admission.session))
        .map_err(|_| Error::invalid("Invalid browser session identity."))?;
    let ancestry = store.branch(&admission.session, admission.user_message)?;
    let slot = {
        let mut sessions = sessions().lock().await;
        sessions.retain(|_, slot| {
            let Ok(mut session) = slot.try_lock() else {
                return true;
            };
            if let Some(active) = session.as_mut()
                && (active.used.elapsed() > Duration::from_secs(600)
                    || active
                        .child
                        .as_mut()
                        .and_then(|child| child.try_wait().ok().flatten())
                        .is_some())
            {
                *session = None;
            }
            session.is_some() || Arc::strong_count(slot) > 1
        });
        if !sessions.contains_key(&key) && sessions.len() >= 6 {
            return Err(Error::conflict(
                "Close another browser session before opening a new one.",
            ));
        }
        sessions.entry(key).or_default().clone()
    };
    let mut slot = slot.lock().await;
    // Ownership stays in this future during I/O. Cancellation drops both pipes
    // and the child, so an interrupted action cannot continue in the background.
    let mut session = slot.take();
    if session.as_ref().is_some_and(|s| {
        s.revision != request["config"]["revision"].as_u64().unwrap_or(0)
            || !ancestry.iter().any(|(id, _)| *id == s.leaf)
    }) {
        session = None;
    }
    if request["operation"] == "close" {
        drop(session);
        return Ok(json!({"ok":true,"closed":true}));
    }
    if session.is_none() && request["operation"] != "open" {
        return Err(Error::conflict(
            "No active browser session. It may have expired, been cancelled or belonged to another branch. Use open(url) to continue.",
        ));
    }
    let mut session = match session {
        Some(session) => session,
        None => {
            let path = worker_path().with_file_name("session.mjs");
            if !path.is_file() {
                return Err(Error::unsupported(
                    "Interactive browser runtime is missing.",
                ));
            }
            let mut child = tokio::process::Command::new(
                std::env::var_os("DEEPTUTOR_BROWSER_NODE").unwrap_or_else(|| "node".into()),
            )
            .arg(path)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .env_remove("ELECTRON_RUN_AS_NODE")
            .kill_on_drop(true)
            .spawn()
            .map_err(|_| {
                Error::unsupported("Browser engine could not start. Install its Node dependencies.")
            })?;
            Session {
                stdin: child.stdin.take(),
                stdout: BufReader::new(child.stdout.take().unwrap()),
                child: Some(child),
                leaf: admission.user_message,
                used: std::time::Instant::now(),
                revision: request["config"]["revision"].as_u64().unwrap_or(0),
            }
        }
    };
    let mut bytes = serde_json::to_vec(&request)
        .map_err(|_| Error::invalid("Invalid interactive browser request."))?;
    bytes.push(b'\n');
    let exchange = async {
        session.stdin.as_mut().unwrap().write_all(&bytes).await?;
        let mut output = Vec::new();
        (&mut session.stdout)
            .take(4 * 1024 * 1024 + 1)
            .read_until(b'\n', &mut output)
            .await?;
        if output.len() > 4 * 1024 * 1024 || output.last() != Some(&b'\n') {
            return Err(Error::conflict(
                "Browser session stopped or returned an oversized result. Reopen the page to continue.",
            ));
        }
        serde_json::from_slice::<Value>(&output)
            .map_err(|_| Error::conflict("Browser session returned an invalid result."))
    };
    let output = tokio::time::timeout(Duration::from_secs(70), exchange)
        .await
        .map_err(|_| Error::conflict("Browser operation timed out. The session was closed; reopen the page before continuing."))??;
    session.leaf = admission.user_message;
    session.used = std::time::Instant::now();
    *slot = Some(session);
    if output["ok"] != true {
        return Err(Error::invalid(
            output["error"]
                .as_str()
                .unwrap_or("Browser operation failed."),
        ));
    }
    Ok(output)
}
