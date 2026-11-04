//! Opt-in periodic maintenance generates deterministic review proposals only.
//! Original documents are changed exclusively by the revision-checked apply API.
use crate::{
    Application, Error, Result, learning,
    store::{Store, now, text},
    study,
};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{collections::BTreeSet, sync::Arc};

pub fn policy(conn: &Connection, scope: &str) -> Result<Value> {
    let raw: Option<String> = conn
        .query_row(
            "SELECT payload_json FROM native_memory_maintenance WHERE workspace_id=?",
            [scope],
            |r| r.get(0),
        )
        .optional()?;
    raw.map(|s|serde_json::from_str(&s).map_err(|_|Error::conflict("Invalid memory maintenance policy."))).unwrap_or_else(||Ok(json!({"enabled":false,"revision":0,"last_scan_at":null,"proposal_ids":[],"interval_seconds":60,"mode":"reviewed_exact_duplicates"})))
}
fn save(conn: &Connection, scope: &str, value: &Value) -> Result<()> {
    conn.execute("INSERT INTO native_memory_maintenance VALUES (?,?) ON CONFLICT(workspace_id) DO UPDATE SET payload_json=excluded.payload_json",params![scope,value.to_string()])?;
    Ok(())
}
pub fn update(conn: &Connection, scope: &str, input: &Value) -> Result<Value> {
    if input.as_object().is_none_or(|o| {
        o.keys()
            .any(|k| !matches!(k.as_str(), "enabled" | "expected_revision"))
    }) || !input["enabled"].is_boolean()
    {
        return Err(Error::invalid(
            "enabled and expected_revision are required.",
        ));
    }
    let mut current = policy(conn, scope)?;
    if current["revision"] != input["expected_revision"] {
        return Err(Error::conflict(
            "Memory maintenance settings changed. Reload before saving.",
        ));
    }
    let next = current["revision"]
        .as_u64()
        .and_then(|n| n.checked_add(1))
        .ok_or_else(|| Error::conflict("Policy revision overflow."))?;
    current["enabled"] = input["enabled"].clone();
    current["revision"] = json!(next);
    save(conn, scope, &current)?;
    Ok(current)
}
fn deduplicate(content: &str) -> Option<String> {
    let mut seen = BTreeSet::new();
    let mut lines = Vec::new();
    let mut fence: Option<(char, usize)> = None;
    let mut frontmatter = content
        .lines()
        .next()
        .is_some_and(|line| line.trim() == "---");
    let mut removed = 0;
    for (index, line) in content.lines().enumerate() {
        let trimmed = line.trim();
        if frontmatter {
            if index > 0 && trimmed == "---" {
                frontmatter = false;
            }
            lines.push(line);
            continue;
        }
        let delimiter = trimmed
            .chars()
            .next()
            .filter(|c| matches!(c, '`' | '~'))
            .and_then(|c| {
                let length = trimmed.chars().take_while(|value| *value == c).count();
                (length >= 3).then_some((c, length))
            });
        if let Some((kind, length)) = fence {
            if delimiter.is_some_and(|(c, n)| {
                c == kind && n >= length && trimmed.chars().all(|value| value == kind)
            }) {
                fence = None;
            }
            lines.push(line);
            continue;
        }
        if let Some(delimiter) = delimiter {
            fence = Some(delimiter);
            seen.clear();
            lines.push(line);
            continue;
        }
        if !trimmed.is_empty() && !line.starts_with("- ") {
            seen.clear();
        }
        if line.starts_with("- ") && !seen.insert(line.to_owned()) {
            removed += 1;
            continue;
        }
        lines.push(line);
    }
    if removed == 0 {
        return None;
    }
    let mut result = lines.join("\n");
    if content.ends_with('\n') {
        result.push('\n');
    }
    Some(result)
}
pub fn scan(conn: &Connection, scope: &str) -> Result<Value> {
    let mut state = policy(conn, scope)?;
    let mut proposals = Vec::new();
    let mut skipped = 0;
    for (layer, keys) in [
        ("L2", learning::SURFACES.as_slice()),
        ("L3", learning::SLOTS.as_slice()),
    ] {
        for key in keys {
            let page = learning::read_memory(conn, scope, layer, key)?;
            let content = text(&page, "content");
            if content.len() > 64 * 1024 {
                skipped += 1;
                continue;
            }
            let Some(replacement) = deduplicate(content) else {
                continue;
            };
            if replacement.len() > 32768 {
                skipped += 1;
                continue;
            }
            let digest = format!(
                "{:x}",
                Sha256::digest(
                    json!([scope, layer, key, page["revision"], content])
                        .to_string()
                        .as_bytes()
                )
            );
            let id = format!("memory-cleanup-{digest}");
            let exists = conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM native_study_artifacts WHERE workspace_id=? AND id=?)",
                params![scope, id],
                |r| r.get::<_, bool>(0),
            )?;
            if !exists {
                let context = study::ProposalContext {
                    target: json!({"layer":layer,"key":key,"revision":page["revision"]}),
                    source: content.to_owned(),
                };
                for (operation, input) in [
                    (
                        "study_plan",
                        json!({"title":format!("Memory cleanup · {layer}/{key}"),"objectives":["Remove identical repeated bullet lines while preserving source markers and citations."]}),
                    ),
                    ("study_publish", json!({"content":replacement})),
                ] {
                    study::apply(
                        conn,
                        scope,
                        study::Mutation {
                            turn: id.clone(),
                            kind: "memory_consolidate".into(),
                            operation: operation.into(),
                            input,
                            knowledge: vec![],
                            web: false,
                            target: None,
                            proposal: Some(context.clone()),
                        },
                    )?;
                }
            }
            proposals.push(id);
        }
    }
    state["last_scan_at"] = json!(now());
    state["proposal_ids"] = json!(proposals);
    state["skipped_documents"] = json!(skipped);
    save(conn, scope, &state)?;
    Ok(state)
}
fn scan_due(store: &Store) -> Result<()> {
    let mut conn = store.connect()?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let scopes=tx.prepare("SELECT workspace_id FROM native_memory_maintenance WHERE json_extract(payload_json,'$.enabled')=1 AND COALESCE(json_extract(payload_json,'$.last_scan_at'),0)<? ORDER BY COALESCE(json_extract(payload_json,'$.last_scan_at'),0) LIMIT 100")?.query_map([now()-60.0],|r|r.get::<_,String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
    for scope in scopes {
        scan(&tx, &scope)?;
    }
    tx.commit()?;
    Ok(())
}
#[derive(Default)]
pub struct Scheduler {
    task: tokio::sync::Mutex<Option<tokio::task::JoinHandle<()>>>,
}
impl Scheduler {
    pub async fn start(&self, app: &Arc<Application>) {
        let mut task = self.task.lock().await;
        if task.is_some() {
            return;
        }
        let weak = Arc::downgrade(app);
        let mut stopping = app.shutdown_receiver();
        *task = Some(tokio::spawn(async move {
            let mut timer = tokio::time::interval(std::time::Duration::from_secs(60));
            timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {_=stopping.changed()=>break,_=timer.tick()=>{if *stopping.borrow(){break;}let Some(app)=weak.upgrade()else{break;};let store=app.store.clone();drop(app);let _=crate::application::blocking(move||scan_due(&store)).await;}}
            }
        }));
    }
    pub async fn drain(&self) {
        if let Some(task) = self.task.lock().await.take() {
            let _ = task.await;
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn maintenance_is_opt_in_deduplicated_reviewed_and_revision_checked() {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(include_str!("schema.sql")).unwrap();
        assert_eq!(policy(&c, "w").unwrap()["enabled"], false);
        assert!(update(&c, "w", &json!({"enabled":true,"expected_revision":99})).is_err());
        update(&c, "w", &json!({"enabled":true,"expected_revision":0})).unwrap();
        let page=learning::write_memory(&c,"w","L3","preferences",&json!({"content":"- Likes examples\n- Likes examples\n```\n- source code\n- source code\n```\n","expected_revision":0})).unwrap();
        let scanned = scan(&c, "w").unwrap();
        let id = scanned["proposal_ids"][0].as_str().unwrap();
        assert_eq!(
            scan(&c, "w").unwrap()["proposal_ids"],
            scanned["proposal_ids"]
        );
        assert_eq!(
            learning::read_memory(&c, "w", "L3", "preferences").unwrap(),
            page
        );
        assert!(study::read(&c, "other", id).is_err());
        let applied = study::apply_proposal(
            &c,
            "w",
            id,
            &json!({"expected_revision":2,"expected_target_revision":page["revision"]}),
        )
        .unwrap();
        assert_eq!(
            applied["content"],
            "- Likes examples\n```\n- source code\n- source code\n```\n"
        );
        assert_eq!(scan(&c, "w").unwrap()["proposal_ids"], json!([]));
        assert_eq!(deduplicate("- Same [^1]\n- Same [^2]"), None);
        assert_eq!(deduplicate("# One\n- Same\n# Two\n- Same"), None);
        assert_eq!(deduplicate("---\nlist:\n- Same\n- Same\n---\n"), None);
        assert_eq!(deduplicate("```\n~~~\n- code\n- code\n```"), None);
    }
    #[tokio::test]
    async fn scheduler_resumes_saved_policy_and_drains_on_shutdown() {
        let temp = tempfile::tempdir().unwrap();
        let app = Application::open(temp.path()).unwrap();
        {
            let c = app.store.connect().unwrap();
            update(&c, "w", &json!({"enabled":true,"expected_revision":0})).unwrap();
            learning::write_memory(
                &c,
                "w",
                "L2",
                "chat",
                &json!({"content":"- Same fact\n- Same fact","expected_revision":0}),
            )
            .unwrap();
        }
        app.resume_pending().await.unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                let ready = policy(&app.store.connect().unwrap(), "w").unwrap()["proposal_ids"]
                    .as_array()
                    .is_some_and(|ids| ids.len() == 1);
                if ready {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
        app.shutdown().await;
        let current =
            learning::read_memory(&app.store.connect().unwrap(), "w", "L2", "chat").unwrap();
        assert_eq!(current["content"], "- Same fact\n- Same fact");
        drop(app);
        let restarted = Application::open(temp.path()).unwrap();
        restarted.resume_pending().await.unwrap();
        restarted.shutdown().await;
        let count: i64 = restarted
            .store
            .connect()
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM native_study_artifacts WHERE workspace_id='w'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
    }
}
