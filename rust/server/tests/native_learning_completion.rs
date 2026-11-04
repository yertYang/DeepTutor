use deeptutor_server::{calendar, documents, flashcards};
use rusqlite::{Connection, params};
use serde_json::{Value, json};
fn db() -> Connection {
    let c = Connection::open_in_memory().unwrap();
    c.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
    c.execute_batch(include_str!("../src/schema.sql")).unwrap();
    c
}
fn change(c: &Connection, op: &str, id: &str, input: Value) -> Value {
    let tx = c.unchecked_transaction().unwrap();
    let v = calendar::mutate(
        &tx,
        calendar::revision(&tx).unwrap(),
        &uuid::Uuid::new_v4().to_string(),
        op,
        id,
        &input,
    )
    .unwrap();
    tx.commit().unwrap();
    v
}
#[test]
fn fall_back_single_events_preserve_explicit_offsets_and_duration() {
    let c = db();
    let event = change(
        &c,
        "event_create",
        "",
        json!({"title":"Repeated clock hour","start":"2026-11-01T01:30:00-04:00","end":"2026-11-01T01:30:00-05:00","tzid":"America/New_York"}),
    );
    let list =
        calendar::occurrences(&c, "2026-11-01T00:00:00Z", "2026-11-02T00:00:00Z", "UTC").unwrap();
    assert_eq!(list["occurrences"][0]["start"], event["event"]["start"]);
    assert_eq!(list["occurrences"][0]["end"], event["event"]["end"]);
}
#[test]
fn spring_forward_missing_wall_end_keeps_calendar_usable() {
    let c = db();
    change(
        &c,
        "event_create",
        "",
        json!({"title":"Gap","start":"2026-03-01T06:30:00Z","end":"2026-03-01T07:30:00Z","tzid":"America/New_York","rrule":"FREQ=WEEKLY;COUNT=3"}),
    );
    let rows =
        calendar::occurrences(&c, "2026-03-01T00:00:00Z", "2026-03-25T00:00:00Z", "UTC").unwrap();
    assert_eq!(rows["occurrences"].as_array().unwrap().len(), 3);
    assert_eq!(rows["occurrences"][1]["start"], "2026-03-08T06:30:00.000Z");
    assert_eq!(rows["occurrences"][1]["end"], "2026-03-08T07:30:00.000Z");
}
#[test]
fn until_all_day_includes_last_day() {
    let c = db();
    change(
        &c,
        "event_create",
        "",
        json!({"all_day":true,"start":"2026-11-01","end":"2026-11-02","tzid":"America/New_York","rrule":"FREQ=DAILY;UNTIL=20261103"}),
    );
    let rows =
        calendar::occurrences(&c, "2026-11-01T00:00:00Z", "2026-11-06T00:00:00Z", "UTC").unwrap();
    assert_eq!(rows["occurrences"].as_array().unwrap().len(), 3);
    assert_eq!(rows["occurrences"][2]["start"], "2026-11-03");
}
#[test]
fn undo_detects_new_overrides_without_discarding_them() {
    let c = db();
    let v = change(
        &c,
        "event_create",
        "",
        json!({"title":"Study","start":"2026-11-01T09:00:00Z","end":"2026-11-01T10:00:00Z","rrule":"FREQ=DAILY;COUNT=3"}),
    );
    let id = v["event"]["id"].as_str().unwrap();
    let deleted = change(
        &c,
        "event_delete",
        id,
        json!({"scope":"this","occurrence":"2026-11-02T09:00:00.000Z"}),
    );
    change(
        &c,
        "event_update",
        id,
        json!({"scope":"this","occurrence":"2026-11-03T09:00:00.000Z","changes":{"notes":"newer exception"}}),
    );
    assert!(
        calendar::mutate(
            &c,
            calendar::revision(&c).unwrap(),
            "undo-newer",
            "undo",
            deleted["undo_token"].as_str().unwrap(),
            &json!({})
        )
        .is_err()
    );
    let rows =
        calendar::occurrences(&c, "2026-11-01T00:00:00Z", "2026-11-05T00:00:00Z", "UTC").unwrap();
    assert_eq!(rows["occurrences"].as_array().unwrap().len(), 2);
    assert!(
        rows["occurrences"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v["notes"] == "newer exception")
    );
}
#[test]
fn legacy_flashcards_review_without_modifying_source_blocks() {
    let c = db();
    let mut note = documents::create(
        &c,
        "a",
        &json!({"title":"Old notes","content":"Keep legacy content"}),
    )
    .unwrap();
    let p = note["id"].as_str().unwrap().to_owned();
    note["blocks"] = json!([{"id":"legacy","type":"learning","props":{"kind":"flashcards","data":json!({"cards":[{"front":"Old question","back":"Old answer"}]}).to_string()}}]);
    c.execute(
        "UPDATE native_documents SET payload_json=? WHERE workspace_id='a' AND id=?",
        params![note.to_string(), p],
    )
    .unwrap();
    let deck = flashcards::deck(&c, "a", &p).unwrap();
    assert_eq!(deck["cards"][0]["front"], "Old question");
    assert_eq!(
        flashcards::due(&c, "a").unwrap()["cards"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert!(
        flashcards::export(&c, "a", None)
            .unwrap()
            .contains("Old question\tOld answer")
    );
    let result=flashcards::mutate(&c,"a",&p,true,&json!({"command_id":"legacy-review","expected_revision":0,"card_id":deck["cards"][0]["id"],"remembered":true})).unwrap();
    assert_eq!(result["cards"][0]["reviews"], 1);
    assert_eq!(documents::read(&c, "a", &p).unwrap(), note);
}
