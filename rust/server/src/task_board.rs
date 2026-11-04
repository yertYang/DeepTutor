//! Workspace-scoped task cards and local conversation links.
use crate::{Error, Result, calendar as data, store::text};
use chrono::NaiveDate;
use rusqlite::{Connection, params};
use serde_json::{Value, json};
fn kind(scope: &str, part: &str) -> String {
    format!("tasks:{part}:{scope}")
}
fn fields(v: &Value, allowed: &[&str]) -> Result<()> {
    if v.as_object()
        .is_none_or(|o| o.keys().any(|k| !allowed.contains(&k.as_str())))
    {
        return Err(Error::invalid("Unexpected task field."));
    }
    Ok(())
}
fn label(v: &Value, key: &str, max: usize, required: bool) -> Result<String> {
    crate::learning::string(v, key, max, required)
}
fn card(c: &Connection, scope: &str, id: &str) -> Result<Value> {
    data::optional(c, &kind(scope, "card"), id)?.ok_or_else(Error::missing)
}
pub fn board(c: &Connection, scope: &str) -> Result<Value> {
    let cards = data::rows(c, &kind(scope, "card"))?;
    let colors = data::optional(c, &kind(scope, "settings"), "colors")?
        .map(|v| v["colors"].clone())
        .unwrap_or_else(|| json!({"todo":"#2563eb","doing":"#a16207","done":"#15803d"}));
    Ok(
        json!({"cards":cards,"colors":colors,"session_links":data::rows(c,&kind(scope,"session"))?,"revision":data::revision(c)?}),
    )
}
pub fn exists(c: &Connection, scope: &str, id: &str) -> Result<()> {
    card(c, scope, id).map(|_| ())
}
pub fn change(c: &Connection, scope: &str, input: &Value) -> Result<Value> {
    fields(input, &["action", "id", "changes"])?;
    let action = text(input, "action");
    let id = text(input, "id");
    let changes = &input["changes"];
    match action {
        "create" | "update" => {
            fields(
                changes,
                if action == "create" {
                    &["title", "due_date"]
                } else {
                    &[
                        "title",
                        "note",
                        "status",
                        "archived",
                        "workspace_id",
                        "due_date",
                    ]
                },
            )?;
            let mut v = if action == "create" {
                if data::rows(c, &kind(scope, "card"))?.len() >= 5000 {
                    return Err(Error::invalid("At most 5000 tasks per workspace."));
                }
                json!({"id":uuid::Uuid::new_v4().simple().to_string(),"title":"","note":"","status":"todo","archived":false,"workspace_id":scope,"due_date":null,"created_at":chrono::Utc::now().to_rfc3339()})
            } else {
                card(c, scope, id)?
            };
            v.as_object_mut()
                .unwrap()
                .extend(changes.as_object().unwrap().clone());
            label(&v, "title", 1000, true)?;
            label(&v, "note", 16000, false)?;
            if !["todo", "doing", "done"].contains(&text(&v, "status"))
                || !v["archived"].is_boolean()
            {
                return Err(Error::invalid("Invalid task status."));
            }
            if !v["due_date"].is_null() {
                let date = label(&v, "due_date", 10, true)?;
                let day = NaiveDate::parse_from_str(&date, "%Y-%m-%d")
                    .map_err(|_| Error::invalid("Invalid task due date."))?;
                if day.format("%Y-%m-%d").to_string() != date {
                    return Err(Error::invalid("Use YYYY-MM-DD for task due date."));
                }
            }
            let destination = if v["workspace_id"].is_null() {
                ""
            } else {
                v["workspace_id"]
                    .as_str()
                    .filter(|s| s.len() <= 256)
                    .ok_or_else(|| Error::invalid("Invalid task workspace."))?
            }
            .to_owned();
            if destination != scope {
                // Moving a card never leaves a cross-workspace conversation or calendar link.
                for mut link in data::rows(c, &kind(scope, "session"))? {
                    link["task_ids"]
                        .as_array_mut()
                        .unwrap()
                        .retain(|x| x != &v["id"]);
                    data::put(c, &kind(scope, "session"), &link)?;
                }
                c.execute("UPDATE native_calendar_objects SET payload_json=json_set(payload_json,'$.task_id',NULL,'$.task_workspace_id',NULL) WHERE kind='event' AND json_extract(payload_json,'$.task_id')=? AND COALESCE(json_extract(payload_json,'$.task_workspace_id'),'')=?",params![text(&v,"id"),scope])?;
                data::remove(c, &kind(scope, "card"), text(&v, "id"))?;
            }
            v["workspace_id"] = json!(destination);
            v["updated_at"] = json!(chrono::Utc::now().to_rfc3339());
            data::put(c, &kind(&destination, "card"), &v)?;
        }
        "colors" => {
            fields(changes, &["todo", "doing", "done"])?;
            for key in ["todo", "doing", "done"] {
                let color = label(changes, key, 7, true)?;
                if color.len() != 7
                    || !color.starts_with('#')
                    || !color[1..].bytes().all(|b| b.is_ascii_hexdigit())
                {
                    return Err(Error::invalid("Use six-digit task colors."));
                }
            }
            data::put(
                c,
                &kind(scope, "settings"),
                &json!({"id":"colors","colors":changes}),
            )?;
        }
        "links" | "status" => {
            fields(
                changes,
                if action == "links" {
                    &["workspace_id", "task_ids"]
                } else {
                    &["workspace_id", "enabled"]
                },
            )?;
            if text(changes, "workspace_id") != scope {
                return Err(Error::invalid("Conflicting task workspace."));
            }
            let present:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM sessions WHERE id=? AND deleted_at IS NULL AND COALESCE(json_extract(preferences_json,'$.workspace_id'),'')=?)",params![id,scope],|r|r.get(0))?;
            if !present {
                return Err(Error::missing());
            }
            let mut v=data::optional(c,&kind(scope,"session"),id)?.unwrap_or_else(||json!({"id":id,"session_id":id,"workspace_id":scope,"task_ids":[],"status_link_enabled":true}));
            if action == "links" {
                let ids = changes["task_ids"]
                    .as_array()
                    .filter(|v| v.len() <= 100)
                    .ok_or_else(|| Error::invalid("Link at most 100 tasks."))?;
                let mut unique = std::collections::HashSet::new();
                for id in ids {
                    let id = id
                        .as_str()
                        .ok_or_else(|| Error::invalid("Invalid task ID."))?;
                    exists(c, scope, id)?;
                    if !unique.insert(id) {
                        return Err(Error::invalid("Duplicate linked task."));
                    }
                }
                v["task_ids"] = json!(ids);
            } else {
                if !changes["enabled"].is_boolean() {
                    return Err(Error::invalid("Invalid status-link flag."));
                }
                v["status_link_enabled"] = changes["enabled"].clone();
            }
            data::put(c, &kind(scope, "session"), &v)?;
        }
        _ => return Err(Error::invalid("Unknown task operation.")),
    }
    board(c, scope)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn db() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(include_str!("schema.sql")).unwrap();
        c
    }
    fn change(c: &Connection, s: &str, action: &str, id: &str, changes: Value) -> Value {
        let tx = c.unchecked_transaction().unwrap();
        let v = data::mutate(
            &tx,
            data::revision(&tx).unwrap(),
            &uuid::Uuid::new_v4().to_string(),
            "tasks",
            s,
            &json!({"action":action,"id":id,"changes":changes}),
        )
        .unwrap();
        tx.commit().unwrap();
        v
    }
    #[test]
    fn cards_due_dates_scope_movement_and_colors_persist() {
        let c = db();
        let v = change(
            &c,
            "a",
            "create",
            "",
            json!({"title":"Read","due_date":"2026-11-04"}),
        );
        let id = text(&v["cards"][0], "id");
        assert_eq!(board(&c, "b").unwrap()["cards"], json!([]));
        assert!(card(&c, "b", id).is_err());
        let v = change(
            &c,
            "a",
            "update",
            id,
            json!({"status":"doing","note":"Review chapter"}),
        );
        assert_eq!(v["cards"][0]["note"], "Review chapter");
        change(
            &c,
            "a",
            "colors",
            "",
            json!({"todo":"#123456","doing":"#654321","done":"#abcdef"}),
        );
        assert_eq!(board(&c, "a").unwrap()["colors"]["done"], "#abcdef");
        change(&c, "a", "update", id, json!({"workspace_id":"b"}));
        assert_eq!(board(&c, "a").unwrap()["cards"], json!([]));
        assert_eq!(
            board(&c, "b").unwrap()["cards"][0]["due_date"],
            "2026-11-04"
        );
    }
    #[test]
    fn links_require_real_same_workspace_tasks_and_sessions() {
        let c = db();
        c.execute("INSERT INTO sessions(id,title,created_at,updated_at,preferences_json) VALUES('chat','Test',0,0,'{\"workspace_id\":\"a\"}')",[]).unwrap();
        let v = change(&c, "a", "create", "", json!({"title":"Read"}));
        let id = text(&v["cards"][0], "id");
        let linked = change(
            &c,
            "a",
            "links",
            "chat",
            json!({"workspace_id":"a","task_ids":[id]}),
        );
        assert_eq!(linked["session_links"][0]["task_ids"], json!([id]));
        assert!(super::change(&c,"b",&json!({"action":"links","id":"chat","changes":{"workspace_id":"b","task_ids":[id]}})).is_err());
        change(
            &c,
            "a",
            "status",
            "chat",
            json!({"workspace_id":"a","enabled":false}),
        );
        assert_eq!(
            board(&c, "a").unwrap()["session_links"][0]["status_link_enabled"],
            false
        );
        change(&c, "a", "update", id, json!({"workspace_id":"b"}));
        assert_eq!(
            board(&c, "a").unwrap()["session_links"][0]["task_ids"],
            json!([])
        );
    }
    #[test]
    fn delayed_retry_returns_current_board() {
        let c = db();
        let input = json!({"action":"create","id":"","changes":{"title":"Read"}});
        let first = data::mutate(&c, 0, "original", "tasks", "a", &input).unwrap();
        let id = text(&first["cards"][0], "id");
        change(&c, "a", "update", id, json!({"status":"done"}));
        let retried = data::mutate(&c, 0, "original", "tasks", "a", &input).unwrap();
        assert_eq!(retried["cards"][0]["status"], "done");
        assert_eq!(retried["revision"], 2);
    }
}
