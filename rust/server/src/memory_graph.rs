//! Consistent, workspace-scoped provenance graph derived from committed memory.
//! Only explicit citations and successful tool receipts create graph edges.
use crate::{Result, learning, store::text};
use rusqlite::{Connection, params};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

fn citations(line: &str) -> Vec<String> {
    let mut refs = Vec::new();
    let mut rest = line;
    while let Some((_, tail)) = rest.split_once("[^") {
        let Some((id, next)) = tail.split_once(']') else {
            break;
        };
        if id.len() <= 128 && !id.contains(['[', '\n']) {
            refs.push(id.to_owned());
        }
        rest = next;
    }
    refs
}
fn parsed(content: &str) -> Value {
    let mut title = String::new();
    let mut section = String::new();
    let mut entries = Vec::new();
    let mut footnotes = BTreeMap::<String, Vec<String>>::new();
    let mut fenced = false;
    let mut seen = BTreeSet::new();
    for (index, raw) in content.lines().enumerate() {
        let line = raw.trim();
        if line.starts_with("```") || line.starts_with("~~~") {
            fenced = !fenced;
            continue;
        }
        if fenced || line.is_empty() || line == "---" {
            continue;
        }
        if let Some(value) = line.strip_prefix("# ") {
            if title.is_empty() {
                title = value.to_owned();
            }
            section = value.to_owned();
            continue;
        }
        if line.starts_with('#') {
            section = line.trim_start_matches('#').trim().to_owned();
            continue;
        }
        if let Some(tail) = line.strip_prefix("[^")
            && let Some((label, value)) = tail.split_once("]:")
        {
            footnotes.insert(
                label.to_owned(),
                value
                    .split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned)
                    .collect(),
            );
            continue;
        }
        if entries.len() >= 200 {
            continue;
        }
        let bullet = line
            .strip_prefix("- ")
            .or_else(|| line.strip_prefix("* "))
            .unwrap_or(line);
        let marker = bullet
            .rsplit_once("<!--")
            .and_then(|(_, s)| s.strip_suffix("-->"));
        let legacy_id = citations(bullet)
            .into_iter()
            .find(|id| learning::valid_memory_id(id));
        let id = marker
            .map(str::trim)
            .filter(|s| learning::valid_memory_id(s))
            .map(str::to_owned)
            .or(legacy_id)
            .unwrap_or_else(|| {
                format!(
                    "native_{:x}",
                    Sha256::digest(format!("{index}:{line}").as_bytes())
                )
            });
        if !seen.insert(id.clone()) {
            continue;
        }
        let refs = citations(bullet);
        let mut body = bullet.split("<!--").next().unwrap_or(bullet).to_owned();
        for reference in &refs {
            body = body.replace(&format!("[^{reference}]"), "");
        }
        entries.push(json!({"id":id,"section":section,"text":body.trim().chars().take(2000).collect::<String>(),"refs":refs,"line":index+1}));
    }
    for entry in &mut entries {
        let mut refs = BTreeSet::new();
        for reference in entry["refs"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
        {
            if let Some(values) = footnotes.get(reference) {
                refs.extend(values.iter().cloned());
            }
        }
        entry["refs"] = json!(refs);
    }
    json!({"title":title,"entries":entries})
}
fn scoped_href(path: &str, scope: &str) -> String {
    let mut url = reqwest::Url::parse("http://localhost").unwrap();
    url.set_path(path);
    url.query_pairs_mut().append_pair("dt_workspace", scope);
    url.as_str()["http://localhost".len()..].to_owned()
}
pub fn snapshot(conn: &Connection, scope: &str) -> Result<Value> {
    let mut l1 = serde_json::Map::new();
    let mut l2 = serde_json::Map::new();
    let mut l3 = serde_json::Map::new();
    for surface in learning::SURFACES {
        l1.insert(surface.into(), json!([]));
        l2.insert(
            surface.into(),
            parsed(text(
                &learning::read_memory(conn, scope, "L2", surface)?,
                "content",
            )),
        );
    }
    for slot in learning::SLOTS {
        l3.insert(
            slot.into(),
            parsed(text(
                &learning::read_memory(conn, scope, "L3", slot)?,
                "content",
            )),
        );
    }
    let receipts=conn.prepare("SELECT json_extract(i.result_json,'$.write_memory.entry_id'),s.id,s.title,t.created_at FROM native_tool_invocations i JOIN turns t ON t.id=i.turn_id JOIN sessions s ON s.id=t.session_id WHERE i.name='write_memory' AND i.status='completed' AND json_extract(i.result_json,'$.success')=1 AND COALESCE(json_extract(s.preferences_json,'$.workspace_id'),'')=? AND s.deleted_at IS NULL ORDER BY i.finished_at DESC LIMIT 500")?
        .query_map([scope],|r|Ok((r.get::<_,Option<String>>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,f64>(3)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
    let mut sources = BTreeMap::<String, Value>::new();
    for (entry_id, id, label, created) in receipts {
        let Some(entry_id) = entry_id else {
            continue;
        };
        let mut referenced = false;
        for doc in l2.values_mut().chain(l3.values_mut()) {
            for entry in doc["entries"]
                .as_array_mut()
                .into_iter()
                .flatten()
                .filter(|entry| entry["id"] == entry_id)
            {
                let reference = json!(format!("chat:{id}"));
                let refs = entry["refs"].as_array_mut().unwrap();
                if !refs.contains(&reference) {
                    refs.push(reference);
                }
                referenced = true;
            }
        }
        if referenced {
            sources.insert(id.clone(),json!({"id":id,"label":label,"ts":created.to_string(),"content":"Saved by a committed memory tool invocation.","href":scoped_href(&format!("/chat/{id}"),scope)}));
        }
    }
    // Historical explicit citations may refer to existing conversations or authored pages.
    let refs = l2
        .values()
        .chain(l3.values())
        .flat_map(|doc| doc["entries"].as_array().into_iter().flatten())
        .flat_map(|entry| {
            entry["refs"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
        })
        .map(str::to_owned)
        .collect::<BTreeSet<_>>();
    let mut pages = BTreeMap::new();
    for reference in refs {
        if let Some(id) = reference.strip_prefix("chat:") {
            let row=conn.query_row("SELECT title,created_at FROM sessions WHERE id=? AND COALESCE(json_extract(preferences_json,'$.workspace_id'),'')=? AND deleted_at IS NULL",params![id,scope],|r|Ok((r.get::<_,String>(0)?,r.get::<_,f64>(1)?)));
            if let Ok((title, created)) = row {
                sources.entry(id.to_owned()).or_insert(json!({"id":id,"label":title,"ts":created.to_string(),"content":"Explicitly cited conversation.","href":scoped_href(&format!("/chat/{id}"),scope)}));
            }
        } else if let Some(id) = reference.strip_prefix("cowriter:")
            && let Ok(page) = crate::documents::read(conn, scope, id)
        {
            pages.insert(id.to_owned(),json!({"id":id,"label":page["title"],"ts":page["created_at"].to_string(),"content":text(&page,"content").chars().take(280).collect::<String>(),"href":scoped_href(&format!("/co-writer/{id}"),scope)}));
        }
    }
    l1["chat"] = json!(sources.into_values().collect::<Vec<_>>());
    l1["cowriter"] = json!(pages.into_values().collect::<Vec<_>>());
    Ok(
        json!({"workspace_id":scope,"l1":l1,"l2":l2,"l3":l3,"limits":{"entries_per_document":200,"tool_receipts":500},"edge_policy":"explicit_citations_and_committed_receipts"}),
    )
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn graph_includes_freeform_memory_and_never_invents_provenance_or_crosses_workspaces() {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(include_str!("schema.sql")).unwrap();
        let page =
            crate::documents::create(&c, "w", &json!({"title":"Source","content":"Source facts"}))
                .unwrap();
        learning::write_memory(&c,"w","L3","preferences",&json!({"content":format!("# Preferences\n- Likes examples [^1]\n[^1]: cowriter:{}\n- Uncited fact\n","ignored"),"expected_revision":0})).unwrap();
        let graph = snapshot(&c, "w").unwrap();
        assert_eq!(
            graph["l3"]["preferences"]["entries"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        assert_eq!(graph["l1"]["cowriter"], json!([]));
        learning::write_memory(&c,"w","L2","cowriter",&json!({"content":format!("- Cites source [^1]\n[^1]: cowriter:{}",page["id"].as_str().unwrap()),"expected_revision":0})).unwrap();
        assert_eq!(
            snapshot(&c, "w").unwrap()["l1"]["cowriter"][0]["label"],
            "Source"
        );
        assert_eq!(
            snapshot(&c, "other").unwrap()["l3"]["preferences"]["entries"],
            json!([])
        );
        assert_eq!(
            parsed("```\n- code\n```\n- fact")["entries"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
    }
    #[test]
    fn graph_connects_only_committed_workspace_owned_memory_receipts() {
        let temp = tempfile::tempdir().unwrap();
        let store = crate::store::Store::open(&temp.path().join("memory.db")).unwrap();
        let admitted=store.admit(&json!({"content":"I prefer examples","workspace_id":"w","client_submission_id":"memory-graph-test"}),"owner").unwrap();
        let call = crate::tools::ToolCall {
            id: "save-preference".into(),
            name: "write_memory".into(),
            arguments: "{}".into(),
        };
        store
            .start_tool(&admitted.turn, "memory-invocation", &call)
            .unwrap();
        let c = store.connect().unwrap();
        assert_eq!(snapshot(&c, "w").unwrap()["l1"]["chat"], json!([]));
        store
            .commit_tool(
                &admitted.turn,
                "memory-invocation",
                crate::tools::ToolOutcome {
                    result: Value::Null,
                    mutation: Some(learning::Mutation::Preference {
                        content: "Prefers examples".into(),
                        target: None,
                    }),
                },
            )
            .unwrap();
        let graph = snapshot(&c, "w").unwrap();
        assert_eq!(graph["l1"]["chat"][0]["id"], admitted.session);
        assert_eq!(
            graph["l3"]["preferences"]["entries"][0]["refs"][0],
            format!("chat:{}", admitted.session)
        );
        assert_eq!(snapshot(&c, "other").unwrap()["l1"]["chat"], json!([]));
    }
}
