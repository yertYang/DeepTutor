//! Workspace registrations share the existing recovery-aware catalog.
use crate::{
    Error, Result,
    config::{Settings, safe_path},
    store::{now, text},
};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde_json::{Value, json};

fn connect(settings: &Settings) -> Result<Connection> {
    let path = settings.user_root.join("runtime/workspaces.sqlite3");
    safe_path(&settings.user_root, &path)?;
    std::fs::create_dir_all(path.parent().unwrap())?;
    let general = settings.user_root.join("workspace");
    safe_path(&settings.user_root, &general)?;
    std::fs::create_dir_all(general)?;
    let conn = Connection::open(path)?;
    conn.busy_timeout(std::time::Duration::from_secs(10))?;
    conn.execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE IF NOT EXISTS workspaces(id TEXT PRIMARY KEY,payload TEXT NOT NULL);")?;
    Ok(conn)
}
fn read(conn: &Connection, id: &str) -> Result<Option<Value>> {
    let raw: Option<String> = conn
        .query_row("SELECT payload FROM workspaces WHERE id=?", [id], |r| {
            r.get(0)
        })
        .optional()?;
    raw.map(|raw| {
        serde_json::from_str::<Value>(&raw)
            .map_err(|_| Error::conflict("Invalid workspace registration."))
    })
    .transpose()
}
fn general(settings: &Settings) -> Value {
    json!({"workspace_id":"","kind":"general","follows_root":true,"display_name":"General workspace","path":settings.user_root.join("workspace"),"archived":false,"created_at":"","status":"ready","error":"","revision":0,"instructions":""})
}
fn normalize(mut row: Value) -> Result<Value> {
    if !row.is_object() || !row["workspace_id"].is_string() || !row["path"].is_string() {
        return Err(Error::conflict("Invalid workspace registration."));
    }
    if row.get("revision").is_none() {
        row["revision"] = json!(0);
    }
    if row.get("instructions").is_none() {
        row["instructions"] = json!("");
    }
    let path = std::path::Path::new(text(&row, "path"));
    if !path.is_dir() || safe_path(path, path).is_err() {
        row["status"] = json!("invalid");
        row["error"] = json!("Workspace folder is missing or is not a regular directory.");
    } else {
        row["status"] = json!("ready");
        row["error"] = json!("");
    }
    Ok(row)
}
pub fn list(settings: &Settings) -> Result<Value> {
    let conn = connect(settings)?;
    let mut default = read(&conn, "")?.unwrap_or_else(|| general(settings));
    default["path"] = general(settings)["path"].clone();
    let mut rows = vec![normalize(default)?];
    let values = conn
        .prepare("SELECT payload FROM workspaces WHERE id<>'' ORDER BY id")?
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for raw in values {
        let row: Value = serde_json::from_str(&raw)
            .map_err(|_| Error::conflict("Invalid workspace registration."))?;
        if row["kind"] == "workspace" {
            rows.push(normalize(row)?);
        }
    }
    Ok(
        json!({"root":settings.user_root.join("workspaces"),"workspaces":rows,"features":{"registrations":true,"instructions":true,"folder_migration":false,"resource_assignment":false}}),
    )
}
pub fn save(settings: &Settings, id: Option<&str>, input: &Value) -> Result<Value> {
    if input.as_object().is_none_or(|m| {
        m.keys().any(|k| {
            !matches!(
                k.as_str(),
                "name" | "instructions" | "archived" | "expected_revision"
            )
        })
    }) {
        return Err(Error::unsupported(
            "Only workspace name, instructions and archive status can be edited here.",
        ));
    }
    let mut conn = connect(settings)?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let mut row = match id {
        Some(id) => {
            let row = read(&tx, id)?
                .or_else(|| {
                    if id.is_empty() {
                        Some(general(settings))
                    } else {
                        None
                    }
                })
                .ok_or_else(Error::missing)?;
            if input["expected_revision"].as_u64() != Some(row["revision"].as_u64().unwrap_or(0)) {
                return Err(Error::conflict(
                    "Workspace changed elsewhere. Reload before saving.",
                ));
            }
            row
        }
        None => {
            let id = uuid::Uuid::new_v4().simple().to_string();
            let root = settings.user_root.join("workspaces").join(&id);
            safe_path(&settings.user_root, &root)?;
            json!({"workspace_id":id,"kind":"workspace","display_name":"","path":root,"follows_root":true,"archived":false,"created_at":now().to_string(),"revision":0,"instructions":""})
        }
    };
    if let Some(name) = input.get("name") {
        let value = name
            .as_str()
            .map(str::trim)
            .filter(|s| {
                !s.is_empty() && s.chars().count() <= 100 && !s.chars().any(char::is_control)
            })
            .ok_or_else(|| Error::invalid("Workspace name must contain 1-100 characters."))?;
        if row["kind"] != "workspace" {
            return Err(Error::invalid("The general workspace cannot be renamed."));
        }
        row["display_name"] = json!(value);
    }
    if row["display_name"] == "" {
        return Err(Error::invalid("Workspace name is required."));
    }
    if let Some(instructions) = input.get("instructions") {
        let value = instructions
            .as_str()
            .filter(|s| s.len() <= 16 * 1024 && !s.contains('\0'))
            .ok_or_else(|| Error::invalid("Workspace instructions exceed 16 KiB."))?;
        row["instructions"] = json!(value);
    }
    if let Some(archived) = input.get("archived") {
        if row["kind"] != "workspace" || !archived.is_boolean() {
            return Err(Error::invalid("Invalid workspace archive status."));
        }
        row["archived"] = archived.clone();
    }
    row["revision"] = json!(
        row["revision"]
            .as_u64()
            .unwrap_or(0)
            .checked_add(1)
            .ok_or_else(|| Error::conflict("Workspace revision overflow."))?
    );
    if id.is_none() {
        std::fs::create_dir_all(text(&row, "path"))?;
    }
    let row = normalize(row)?;
    tx.execute("INSERT INTO workspaces(id,payload) VALUES (?,?) ON CONFLICT(id) DO UPDATE SET payload=excluded.payload",params![text(&row,"workspace_id"),row.to_string()])?;
    tx.commit()?;
    Ok(json!({"workspace":row}))
}
pub fn instructions(settings: &Settings, id: &str) -> Result<String> {
    let catalog = settings.user_root.join("runtime/workspaces.sqlite3");
    safe_path(&settings.user_root, &catalog)?;
    if !catalog.is_file() {
        return Ok(String::new());
    }
    let conn = Connection::open_with_flags(catalog, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let Some(mut row) = read(&conn, id)? else {
        return Ok(String::new());
    };
    if id.is_empty() {
        row["path"] = general(settings)["path"].clone();
    }
    let row = normalize(row)?;
    if row["status"] != "ready" {
        return Err(Error::conflict(
            "The registered workspace folder is unavailable. Restore it before starting a turn.",
        ));
    }
    if row["archived"] == true {
        return Err(Error::conflict(
            "This workspace is archived. Restore it before starting a new turn.",
        ));
    }
    Ok(text(&row, "instructions").to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn registrations_preserve_identity_reject_stale_edits_and_archive_without_deleting() {
        let temp = tempfile::tempdir().unwrap();
        let settings = Settings::open(temp.path()).unwrap();
        let row = save(
            &settings,
            None,
            &json!({"name":"概率学习","instructions":"请用中文举例"}),
        )
        .unwrap()["workspace"]
            .clone();
        let id = text(&row, "workspace_id");
        let path = text(&row, "path");
        assert!(std::path::Path::new(path).is_dir());
        std::fs::write(std::path::Path::new(path).join("keep.txt"), "preserved").unwrap();
        assert_eq!(instructions(&settings, id).unwrap(), "请用中文举例");
        assert!(
            save(
                &settings,
                Some(id),
                &json!({"name":"stale","expected_revision":0})
            )
            .is_err()
        );
        let archived = save(
            &settings,
            Some(id),
            &json!({"archived":true,"expected_revision":1}),
        )
        .unwrap();
        assert_eq!(archived["workspace"]["workspace_id"], id);
        assert!(instructions(&settings, id).is_err());
        save(
            &settings,
            Some(id),
            &json!({"archived":false,"expected_revision":2}),
        )
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(std::path::Path::new(path).join("keep.txt")).unwrap(),
            "preserved"
        );
        assert_eq!(
            list(&settings).unwrap()["workspaces"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
    }
}
