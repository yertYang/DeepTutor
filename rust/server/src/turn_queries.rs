//! Read-only turn queries. All writes belong to the application's Store transactions.
use crate::{Error, Result};
use rusqlite::{Connection, OptionalExtension, Row, params};
use serde::Deserialize;
use serde_json::{Value, json};

const TURN_SELECT: &str = "SELECT t.*, COALESCE((SELECT MAX(seq) FROM turn_events te WHERE te.turn_id=t.id),0) AS last_seq FROM turns t";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TurnId {
    turn_id: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SessionId {
    session_id: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SubmissionId {
    submission_id: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Events {
    turn_id: String,
    #[serde(default)]
    after_seq: i64,
    limit: Option<u32>,
}

fn parse<T: serde::de::DeserializeOwned>(args: Value) -> Result<T> {
    serde_json::from_value(args).map_err(|_| Error::invalid("Invalid turn query arguments."))
}

fn turn_row(row: &Row<'_>) -> rusqlite::Result<Value> {
    let text = |key: &str| -> rusqlite::Result<String> {
        Ok(row.get::<_, Option<String>>(key)?.unwrap_or_default())
    };
    let id = text("id")?;
    Ok(json!({
        "id":id, "turn_id":id, "session_id":text("session_id")?,
        "capability":text("capability")?, "status":text("status")?, "error":text("error")?,
        "created_at":row.get::<_, f64>("created_at")?, "updated_at":row.get::<_, f64>("updated_at")?,
        "finished_at":row.get::<_, Option<f64>>("finished_at")?,
        "last_seq":match row.as_ref().column_index("last_seq") {
            Ok(index) => row.get::<_, i64>(index)?, Err(_) => 0
        },
        "owner_id":text("owner_id")?, "fencing_token":row.get::<_, i64>("fencing_token")?,
        "state_version":row.get::<_, i64>("state_version")?, "failure_code":text("failure_code")?,
        "retryable":row.get::<_, i64>("retryable")? != 0,
        "assistant_message_id":row.get::<_, Option<i64>>("assistant_message_id")?
    }))
}

fn query_turn(conn: &Connection, sql: &str, args: impl rusqlite::Params) -> Result<Value> {
    Ok(conn
        .query_row(sql, args, turn_row)
        .optional()?
        .unwrap_or(Value::Null))
}

pub(crate) fn read(conn: &Connection, operation: &str, args: Value) -> Result<Value> {
    match operation {
        "submission_turn" => {
            let args: SubmissionId = parse(args)?;
            query_turn(
                conn,
                "SELECT t.* FROM turns t JOIN turn_submissions s ON s.turn_id=t.id WHERE s.client_submission_id=?",
                params![args.submission_id],
            )
        }
        "get_turn" => {
            let args: TurnId = parse(args)?;
            query_turn(
                conn,
                &format!("{TURN_SELECT} WHERE t.id=?"),
                params![args.turn_id],
            )
        }
        "get_active_turn" | "list_active_turns" => {
            let args: SessionId = parse(args)?;
            let rows = conn.prepare(&format!("{TURN_SELECT} WHERE t.session_id=? AND t.status IN ('queued','running','waiting_input') ORDER BY t.updated_at DESC"))?
                .query_map([args.session_id], turn_row)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            if operation == "get_active_turn" {
                Ok(rows.into_iter().next().unwrap_or(Value::Null))
            } else {
                Ok(json!(rows))
            }
        }
        "get_events" => events(conn, parse(args)?),
        _ => Err(Error::invalid("Unknown turn query.")),
    }
}

fn events(conn: &Connection, args: Events) -> Result<Value> {
    let session = conn
        .query_row(
            "SELECT session_id FROM turns WHERE id=?",
            [&args.turn_id],
            |r| r.get::<_, String>(0),
        )
        .optional()?
        .unwrap_or_default();
    let rows = conn.prepare("SELECT * FROM turn_events WHERE turn_id=? AND seq>? ORDER BY seq ASC LIMIT ?")?
        .query_map(params![args.turn_id, args.after_seq.max(0), args.limit.map_or(-1, |n| i64::from(n.clamp(1, 1024)))], |row| {
            let text = |key: &str| -> rusqlite::Result<String> { Ok(row.get::<_, Option<String>>(key)?.unwrap_or_default()) };
            let metadata = row.get::<_, Option<String>>("metadata_json")?
                .and_then(|s| serde_json::from_str::<Value>(&s).ok()).unwrap_or_else(||json!({}));
            Ok(json!({"type":text("type")?,"source":text("source")?,"stage":text("stage")?,"content":text("content")?,
                "metadata":metadata,"seq":row.get::<_,i64>("seq")?,"timestamp":row.get::<_,f64>("timestamp")?,
                "turn_id":text("turn_id")?,"session_id":session}))
        })?.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(json!(rows))
}
