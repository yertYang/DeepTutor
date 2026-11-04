//! Local reading records own immutable snapshots, independent revision channels,
//! and durable command receipts. All mutations run in the caller's transaction.
use crate::{
    Error, Result, knowledge,
    learning::string,
    store::{now, text},
};
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

fn object(v: &Value, allowed: &[&str]) -> Result<()> {
    if v.as_object()
        .is_none_or(|o| o.keys().any(|k| !allowed.contains(&k.as_str())))
    {
        return Err(Error::invalid("Unexpected reading field."));
    }
    Ok(())
}
fn decode(raw: String) -> Result<Value> {
    serde_json::from_str(&raw).map_err(|_| Error::conflict("Invalid stored reading record."))
}
fn id() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}
fn revision(v: &Value, input: &Value) -> Result<()> {
    if input["expected_revision"].as_u64().is_none() || input["expected_revision"] != v["revision"]
    {
        return Err(Error::conflict(
            "This reading record changed elsewhere. Reload before saving.",
        ));
    }
    Ok(())
}
fn metadata(c: &Connection, s: &str, item: &str) -> Result<Value> {
    decode(
        c.query_row(
            "SELECT payload_json FROM native_reading_items WHERE workspace_id=? AND id=?",
            params![s, item],
            |r| r.get(0),
        )
        .optional()?
        .ok_or_else(Error::missing)?,
    )
}
pub fn progress(c: &Connection, s: &str, item: &str) -> Result<Value> {
    metadata(c, s, item)?;
    decode(c.query_row(
        "SELECT payload_json FROM native_reading_progress WHERE workspace_id=? AND item_id=?",
        params![s, item],
        |r| r.get(0),
    )?)
}
pub fn read(c: &Connection, s: &str, item: &str) -> Result<Value> {
    let mut v = metadata(c, s, item)?;
    v["progress"] = progress(c, s, item)?;
    Ok(v)
}
pub fn list(c: &Connection, s: &str, offset: u32) -> Result<Value> {
    if offset > 100_000 {
        return Err(Error::invalid("Reading offset is too large."));
    }
    let ids = c.prepare("SELECT i.id FROM native_reading_items i JOIN native_reading_progress p ON p.workspace_id=i.workspace_id AND p.item_id=i.id WHERE i.workspace_id=? ORDER BY max(json_extract(i.payload_json,'$.updated_at'),json_extract(p.payload_json,'$.updated_at')) DESC,i.id LIMIT 101 OFFSET ?")?
        .query_map(params![s,offset], |r|r.get::<_,String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
    let more = ids.len() > 100;
    let items = ids
        .iter()
        .take(100)
        .map(|i| read(c, s, i))
        .collect::<Result<Vec<_>>>()?;
    Ok(json!({"items":items,"next_offset":if more {Some(offset+100)} else {None}}))
}
pub fn dashboard(c: &Connection, s: &str) -> Result<Value> {
    let mut rows = list(c, s, 0)?["items"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    for row in &mut rows {
        row["content_workspace_id"] = json!(s);
        row["updated_at"] = json!(
            row["updated_at"]
                .as_f64()
                .unwrap_or(0.0)
                .max(row["progress"]["updated_at"].as_f64().unwrap_or(0.0))
        );
    }
    Ok(json!(rows))
}
pub fn add(c: &Connection, s: &str, input: &Value) -> Result<Value> {
    object(input, &["kb", "path", "hash"])?;
    let kb = string(input, "kb", 200, true)?;
    let path = string(input, "path", 512, true)?;
    let hash = string(input, "hash", 128, true)?;
    let prior: Option<String> = c.query_row("SELECT id FROM native_reading_items WHERE workspace_id=? AND kb=? AND path=? AND original_hash=?",params![s,kb,path,hash],|r|r.get(0)).optional()?;
    if let Some(prior) = prior {
        return read(c, s, &prior);
    }
    knowledge::require(c, s, &kb)?;
    let content = knowledge::source(c, s, &kb, &path, &hash)?;
    let original = knowledge::original(c, s, &kb, &path, Some(&hash))?;
    if content.len() > knowledge::MAX_FILE || original.len() > knowledge::MAX_BATCH {
        return Err(Error::invalid("Reading source exceeds the import limit."));
    }
    save_snapshot(
        c,
        s,
        &SourceSnapshot {
            kb,
            path: path.clone(),
            hash,
            title: path,
            kind: "local".into(),
            source_url: None,
            mime: "application/octet-stream".into(),
            content,
            original,
        },
    )
}
struct SourceSnapshot {
    kb: String,
    path: String,
    hash: String,
    title: String,
    kind: String,
    source_url: Option<String>,
    mime: String,
    content: String,
    original: Vec<u8>,
}
fn save_snapshot(c: &Connection, s: &str, source: &SourceSnapshot) -> Result<Value> {
    let prior:Option<String>=c.query_row("SELECT id FROM native_reading_items WHERE workspace_id=? AND kb=? AND path=? AND original_hash=?",params![s,source.kb,source.path,source.hash],|r|r.get(0)).optional()?;
    if let Some(prior) = prior {
        return read(c, s, &prior);
    }
    if source.content.len() > knowledge::MAX_FILE || source.original.len() > knowledge::MAX_BATCH {
        return Err(Error::invalid("Reading source exceeds the import limit."));
    }
    let source_id = format!(
        "{:x}",
        Sha256::digest([source.hash.as_bytes(), b"\0", source.content.as_bytes()].concat())
    );
    c.execute(
        "INSERT OR IGNORE INTO native_reading_sources VALUES(?,?,?,?)",
        params![s, source_id, source.content, source.original],
    )?;
    let item = id();
    let at = now();
    let v = json!({"id":item,"source_id":source_id,"kb":source.kb,"path":source.path,"original_hash":source.hash,"title":source.title,"kind":source.kind,"source_url":source.source_url,"mime":source.mime,"line_count":source.content.split('\n').count(),"revision":0,"created_at":at,"updated_at":at});
    c.execute(
        "INSERT INTO native_reading_items VALUES(?,?,?,?,?,?,?)",
        params![
            s,
            item,
            source_id,
            source.kb,
            source.path,
            source.hash,
            v.to_string()
        ],
    )?;
    c.execute(
        "INSERT INTO native_reading_progress VALUES(?,?,?)",
        params![
            s,
            item,
            json!({"line":1,"completed":false,"revision":0,"updated_at":at}).to_string()
        ],
    )?;
    read(c, s, &item)
}
pub fn web_url(input: &Value) -> Result<String> {
    object(input, &["url", "command_id"])?;
    string(input, "command_id", 128, true)?;
    crate::web_reading::canonical_url(&string(input, "url", 4096, true)?)
}
pub fn web_receipt(c: &Connection, s: &str, input: &Value) -> Result<Option<Value>> {
    let url = web_url(input)?;
    let prior:Option<(String,String)>=c.query_row("SELECT request_url,item_id FROM native_reading_web_receipts WHERE workspace_id=? AND command_id=?",params![s,text(input,"command_id")],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
    if let Some((old, item)) = prior {
        if old != url {
            return Err(Error::conflict(
                "Web import command was reused with another URL.",
            ));
        }
        return read(c, s, &item).map(Some).map_err(|e| {
            if e.code == "not_found" {
                Error::conflict(
                    "This reading copy was deleted. Start a new import to save another copy.",
                )
            } else {
                e
            }
        });
    }
    Ok(None)
}
pub(crate) fn add_web(
    c: &Connection,
    s: &str,
    input: &Value,
    snapshot: crate::web_reading::Snapshot,
) -> Result<Value> {
    if let Some(prior) = web_receipt(c, s, input)? {
        return Ok(prior);
    }
    let url = web_url(input)?;
    if snapshot.requested_url != url {
        return Err(Error::conflict(
            "The downloaded page does not match this import.",
        ));
    }
    let hash = format!("{:x}", Sha256::digest(&snapshot.original));
    let v = save_snapshot(
        c,
        s,
        &SourceSnapshot {
            kb: "Web".into(),
            path: snapshot.url.clone(),
            hash,
            title: snapshot.title,
            kind: "web".into(),
            source_url: Some(snapshot.url),
            mime: snapshot.mime,
            content: snapshot.content,
            original: snapshot.original,
        },
    )?;
    c.execute(
        "INSERT INTO native_reading_web_receipts VALUES(?,?,?,?)",
        params![s, text(input, "command_id"), url, text(&v, "id")],
    )?;
    Ok(v)
}

pub fn tutor_context(
    c: &Connection,
    s: &str,
    input: &Value,
) -> Result<crate::study::ProposalContext> {
    object(input, &["item_id", "source_id", "start_line", "end_line"])?;
    let item = string(input, "item_id", 64, true)?;
    let row = metadata(c, s, &item)?;
    if input["source_id"] != row["source_id"] {
        return Err(Error::conflict("The selected source snapshot changed."));
    }
    let start = input["start_line"]
        .as_u64()
        .filter(|n| *n >= 1)
        .ok_or_else(|| Error::invalid("Select the first source line."))?;
    let end = input["end_line"]
        .as_u64()
        .filter(|n| {
            *n >= start && *n - start < 500 && *n <= row["line_count"].as_u64().unwrap_or(0)
        })
        .ok_or_else(|| Error::invalid("Select at most 500 existing source lines."))?;
    let content = source(c, s, &item)?;
    let excerpt = content
        .split('\n')
        .enumerate()
        .skip((start - 1) as usize)
        .take((end - start + 1) as usize)
        .map(|(n, text)| json!({"line":n+1,"text":text}))
        .collect::<Vec<_>>();
    let mut citation = reqwest::Url::parse("http://localhost").expect("constant URL");
    citation.set_path(&format!("/learning/reading/{item}"));
    citation.query_pairs_mut().append_pair("dt_workspace", s);
    let target = json!({"item_id":item,"source_id":row["source_id"],"title":row["title"],"original_hash":row["original_hash"],"source_url":row["source_url"],"start_line":start,"end_line":end,"citation_base":&citation.as_str()["http://localhost".len()..]});
    let source = json!({"reading_source":target,"excerpt":excerpt}).to_string();
    if source.len() > 64 * 1024 {
        return Err(Error::invalid(
            "The excerpt exceeds 64 KiB. Select a smaller line range.",
        ));
    }
    Ok(crate::study::ProposalContext { target, source })
}
pub fn rename(c: &Connection, s: &str, item: &str, input: &Value) -> Result<Value> {
    object(input, &["title", "expected_revision"])?;
    let mut v = metadata(c, s, item)?;
    revision(&v, input)?;
    v["title"] = json!(string(input, "title", 1000, true)?);
    v["revision"] = json!(v["revision"].as_u64().unwrap_or(0) + 1);
    v["updated_at"] = json!(now());
    c.execute(
        "UPDATE native_reading_items SET payload_json=? WHERE workspace_id=? AND id=?",
        params![v.to_string(), s, item],
    )?;
    read(c, s, item)
}
pub fn delete(c: &Connection, s: &str, item: &str, input: &Value) -> Result<Value> {
    object(input, &["expected_revision"])?;
    let v = metadata(c, s, item)?;
    revision(&v, input)?;
    c.execute(
        "DELETE FROM native_reading_items WHERE workspace_id=? AND id=?",
        params![s, item],
    )?;
    c.execute("DELETE FROM native_reading_sources WHERE workspace_id=? AND id=? AND NOT EXISTS(SELECT 1 FROM native_reading_items WHERE workspace_id=? AND source_id=?)",params![s,text(&v,"source_id"),s,text(&v,"source_id")])?;
    Ok(json!({"deleted":true}))
}
pub fn save_progress(c: &Connection, s: &str, item: &str, input: &Value) -> Result<Value> {
    object(input, &["line", "completed", "expected_revision"])?;
    let v = metadata(c, s, item)?;
    let old = progress(c, s, item)?;
    revision(&old, input)?;
    let line = input["line"]
        .as_u64()
        .filter(|n| *n >= 1 && *n <= v["line_count"].as_u64().unwrap_or(0))
        .ok_or_else(|| Error::invalid("Position is outside this source."))?;
    let completed = input["completed"]
        .as_bool()
        .ok_or_else(|| Error::invalid("Specify whether reading is finished."))?;
    let p = json!({"line":line,"completed":completed,"revision":old["revision"].as_u64().unwrap_or(0)+1,"updated_at":now()});
    c.execute(
        "UPDATE native_reading_progress SET payload_json=? WHERE workspace_id=? AND item_id=?",
        params![p.to_string(), s, item],
    )?;
    Ok(p)
}
pub fn source(c: &Connection, s: &str, item: &str) -> Result<String> {
    let v = metadata(c, s, item)?;
    Ok(c.query_row(
        "SELECT content FROM native_reading_sources WHERE workspace_id=? AND id=?",
        params![s, text(&v, "source_id")],
        |r| r.get(0),
    )?)
}
pub fn original(c: &Connection, s: &str, item: &str) -> Result<(String, Vec<u8>)> {
    let v = metadata(c, s, item)?;
    let bytes = c.query_row(
        "SELECT original FROM native_reading_sources WHERE workspace_id=? AND id=?",
        params![s, text(&v, "source_id")],
        |r| r.get(0),
    )?;
    let name = if v["kind"] == "web" {
        if text(&v, "mime").starts_with("text/plain") {
            "web-snapshot.txt"
        } else {
            "web-snapshot.html"
        }
        .to_owned()
    } else {
        text(&v, "path")
            .rsplit('/')
            .next()
            .unwrap_or("document")
            .to_owned()
    };
    Ok((name, bytes))
}
pub fn annotations(c: &Connection, s: &str, item: &str) -> Result<Value> {
    metadata(c, s, item)?;
    let values = c.prepare("SELECT payload_json FROM native_reading_annotations WHERE workspace_id=? AND item_id=? ORDER BY json_extract(payload_json,'$.start_line'),id")?.query_map(params![s,item],|r|r.get::<_,String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(json!({"annotations":values.into_iter().map(decode).collect::<Result<Vec<_>>>()?}))
}
fn annotation(c: &Connection, s: &str, item: &str, a: &str) -> Result<Value> {
    decode(c.query_row("SELECT payload_json FROM native_reading_annotations WHERE workspace_id=? AND item_id=? AND id=?",params![s,item,a],|r|r.get(0)).optional()?.ok_or_else(Error::missing)?)
}
fn note_fields(input: &Value) -> Result<(String, String)> {
    let note = string(input, "note", 8000, false)?;
    let color = string(input, "color", 20, true)?;
    if !["yellow", "green", "blue", "pink"].contains(&color.as_str()) {
        return Err(Error::invalid("Choose a supported highlight color."));
    }
    Ok((note, color))
}
pub fn annotate(c: &Connection, s: &str, item: &str, input: &Value) -> Result<Value> {
    object(
        input,
        &[
            "command_id",
            "source_id",
            "start_line",
            "end_line",
            "note",
            "color",
        ],
    )?;
    let v = metadata(c, s, item)?;
    let command = string(input, "command_id", 128, true)?;
    let digest = format!(
        "{:x}",
        Sha256::digest(json!({"item_id":item,"input":input}).to_string())
    );
    let receipt: Option<(String,String)> = c.query_row("SELECT request_digest,annotation_id FROM native_reading_receipts WHERE workspace_id=? AND command_id=?",params![s,command],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
    if let Some((prior, a)) = receipt {
        if prior != digest {
            return Err(Error::conflict(
                "This annotation command was already used with different content.",
            ));
        }
        return match annotation(c, s, item, &a) {
            Ok(v) => Ok(v),
            Err(e) if e.code == "not_found" => Err(Error::conflict(
                "This annotation was deleted. Start a new annotation to quote this range again.",
            )),
            Err(e) => Err(e),
        };
    }
    if input["source_id"] != v["source_id"] {
        return Err(Error::conflict(
            "The annotation belongs to another source snapshot.",
        ));
    }
    let start = input["start_line"]
        .as_u64()
        .filter(|n| *n > 0)
        .ok_or_else(|| Error::invalid("Select a valid start line."))?;
    let end = input["end_line"]
        .as_u64()
        .filter(|n| *n >= start && *n - start < 500 && *n <= v["line_count"].as_u64().unwrap_or(0))
        .ok_or_else(|| Error::invalid("Select up to 500 lines within the source."))?;
    let count: i64 = c.query_row(
        "SELECT COUNT(*) FROM native_reading_annotations WHERE workspace_id=? AND item_id=?",
        params![s, item],
        |r| r.get(0),
    )?;
    if count >= 500 {
        return Err(Error::invalid(
            "This source already has 500 annotations. Export or remove an annotation first.",
        ));
    }
    let content = source(c, s, item)?;
    let quote = content
        .split('\n')
        .skip((start - 1) as usize)
        .take((end - start + 1) as usize)
        .collect::<Vec<_>>()
        .join("\n");
    if quote.len() > 32 * 1024 {
        return Err(Error::invalid("Select a passage of at most 32 KiB."));
    }
    let (note, color) = note_fields(input)?;
    let a = id();
    let at = now();
    let value = json!({"id":a,"item_id":item,"source_id":v["source_id"],"start_line":start,"end_line":end,"quote":quote,"note":note,"color":color,"revision":0,"created_at":at,"updated_at":at});
    c.execute(
        "INSERT INTO native_reading_annotations VALUES (?,?,?,?)",
        params![s, item, a, value.to_string()],
    )?;
    c.execute(
        "INSERT INTO native_reading_receipts VALUES (?,?,?,?,?)",
        params![s, command, item, digest, a],
    )?;
    Ok(value)
}
pub fn edit_annotation(
    c: &Connection,
    s: &str,
    item: &str,
    a: &str,
    input: &Value,
) -> Result<Value> {
    object(input, &["note", "color", "expected_revision"])?;
    let mut v = annotation(c, s, item, a)?;
    revision(&v, input)?;
    let (note, color) = note_fields(input)?;
    v["note"] = json!(note);
    v["color"] = json!(color);
    v["revision"] = json!(v["revision"].as_u64().unwrap_or(0) + 1);
    v["updated_at"] = json!(now());
    c.execute("UPDATE native_reading_annotations SET payload_json=? WHERE workspace_id=? AND item_id=? AND id=?",params![v.to_string(),s,item,a])?;
    Ok(v)
}
pub fn delete_annotation(
    c: &Connection,
    s: &str,
    item: &str,
    a: &str,
    input: &Value,
) -> Result<Value> {
    object(input, &["expected_revision"])?;
    revision(&annotation(c, s, item, a)?, input)?;
    c.execute(
        "DELETE FROM native_reading_annotations WHERE workspace_id=? AND item_id=? AND id=?",
        params![s, item, a],
    )?;
    Ok(json!({"deleted":true}))
}
pub fn clip(c: &Connection, s: &str, item: &str, a: &str, input: &Value) -> Result<Value> {
    object(input, &["expected_revision"])?;
    let v = annotation(c, s, item, a)?;
    // A receipt returns the current authored page, preserving subsequent human edits.
    let prior: Option<String> = c.query_row("SELECT document_id FROM native_reading_clips WHERE workspace_id=? AND item_id=? AND annotation_id=?",params![s,item,a],|r|r.get(0)).optional()?;
    if let Some(page) = prior {
        return crate::documents::read(c, s, &page).map_err(|e| {
            if e.code == "not_found" {
                Error::conflict(
                    "The clipped note was deleted; it will not be recreated automatically.",
                )
            } else {
                e
            }
        });
    }
    revision(&v, input)?;
    let record = metadata(c, s, item)?;
    let mut url = reqwest::Url::parse("http://localhost").expect("constant URL");
    url.set_path(&format!("/learning/reading/{item}"));
    url.query_pairs_mut().append_pair("dt_workspace", s);
    let link = format!(
        "{}#L{}",
        &url.as_str()["http://localhost".len()..],
        v["start_line"]
    );
    let label = format!(
        "{}:{}–{}",
        text(&record, "title"),
        v["start_line"],
        v["end_line"]
    );
    let page = crate::documents::create(
        c,
        s,
        &json!({"title":label.chars().take(120).collect::<String>(),"blocks":[
            {"id":"reading_quote","type":"quote","props":{},"content":[{"type":"text","text":v["quote"],"styles":{}}],"children":[]},
            {"id":"reading_note","type":"paragraph","props":{},"content":[{"type":"text","text":v["note"],"styles":{}}],"children":[]},
            {"id":"reading_source","type":"paragraph","props":{},"content":[{"type":"link","href":link,"content":[{"type":"text","text":label,"styles":{}}]}],"children":[]}
        ]}),
    )?;
    c.execute(
        "INSERT INTO native_reading_clips VALUES (?,?,?,?)",
        params![s, item, a, text(&page, "id")],
    )?;
    Ok(page)
}
pub fn export(c: &Connection, s: &str, item: &str) -> Result<Value> {
    Ok(
        json!({"format":"lorevia.reading.v1","item":read(c,s,item)?,"content":source(c,s,item)?,"annotations":annotations(c,s,item)?["annotations"],"exported_at":now()}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    fn db() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
        c.execute_batch(include_str!("schema.sql")).unwrap();
        c
    }
    fn seed(c: &Connection, content: &str, create: bool) -> Value {
        knowledge::ingest(
            c,
            "a",
            "教材",
            &[knowledge::Document {
                path: "物理.md".into(),
                content: content.into(),
                original: content.as_bytes().to_vec(),
            }],
            create,
        )
        .unwrap();
        let files = knowledge::files(c, "a", "教材").unwrap();
        add(
            c,
            "a",
            &json!({"kb":"教材","path":"物理.md","hash":files[0]["sha256"]}),
        )
        .unwrap()
    }
    fn command(v: &Value) -> Value {
        json!({"source_id":v["source_id"],"command_id":"create-a","start_line":1,"end_line":2,"note":"我的思考","color":"yellow"})
    }
    #[test]
    fn snapshots_survive_replacement_and_deletion_without_cross_scope_access() {
        let c = db();
        let first = seed(&c, "能量守恒\n不会消失", true);
        let i = text(&first, "id");
        let a = annotate(&c, "a", i, &command(&first)).unwrap();
        let second = seed(&c, "新的版本", false);
        assert_ne!(first["id"], second["id"]);
        assert_eq!(source(&c, "a", i).unwrap(), "能量守恒\n不会消失");
        knowledge::delete_file(&c, "a", "教材", "物理.md").unwrap();
        c.execute("DELETE FROM native_knowledge_bases", []).unwrap();
        assert_eq!(
            original(&c, "a", i).unwrap().1,
            "能量守恒\n不会消失".as_bytes()
        );
        assert_eq!(annotations(&c, "a", i).unwrap()["annotations"][0], a);
        assert!(read(&c, "b", i).is_err());
        assert!(source(&c, "b", i).is_err());
        assert!(original(&c, "b", i).is_err());
        assert!(
            edit_annotation(
                &c,
                "b",
                i,
                text(&a, "id"),
                &json!({"expected_revision":0,"note":"x","color":"blue"})
            )
            .is_err()
        );
        assert!(
            list(&c, "b", 0).unwrap()["items"]
                .as_array()
                .unwrap()
                .is_empty()
        );
    }
    #[test]
    fn revisions_are_independent_and_stale_writes_do_not_change_data() {
        let c = db();
        let v = seed(&c, "一\n二\n三", true);
        let i = text(&v, "id");
        let a = annotate(&c, "a", i, &command(&v)).unwrap();
        let p = json!({"line":2,"completed":false,"expected_revision":0});
        assert_eq!(save_progress(&c, "a", i, &p).unwrap()["revision"], 1);
        assert_eq!(save_progress(&c, "a", i, &p).unwrap_err().code, "conflict");
        assert!(
            save_progress(
                &c,
                "a",
                i,
                &json!({"line":4,"completed":false,"expected_revision":1})
            )
            .is_err()
        );
        rename(&c, "a", i, &json!({"title":"读书","expected_revision":0})).unwrap();
        let update = json!({"note":"改过","color":"blue","expected_revision":0});
        let edited = edit_annotation(&c, "a", i, text(&a, "id"), &update).unwrap();
        assert_eq!(
            edit_annotation(&c, "a", i, text(&a, "id"), &update)
                .unwrap_err()
                .code,
            "conflict"
        );
        assert_eq!(progress(&c, "a", i).unwrap()["line"], 2);
        assert_eq!(annotate(&c, "a", i, &command(&v)).unwrap(), edited);
        assert_eq!(read(&c, "a", i).unwrap()["title"], "读书");
    }
    #[test]
    fn retries_do_not_duplicate_overwrite_or_resurrect_annotations_or_notes() {
        let c = db();
        let v = seed(&c, "原文\n第二行", true);
        let files = knowledge::files(&c, "a", "教材").unwrap();
        assert_eq!(
            add(
                &c,
                "a",
                &json!({"kb":"教材","path":"物理.md","hash":files[0]["sha256"]})
            )
            .unwrap(),
            v
        );
        let i = text(&v, "id");
        let input = command(&v);
        let a = annotate(&c, "a", i, &input).unwrap();
        assert_eq!(annotate(&c, "a", i, &input).unwrap(), a);
        let mut changed = input.clone();
        changed["note"] = json!("changed");
        assert!(annotate(&c, "a", i, &changed).is_err());
        let page = clip(&c, "a", i, text(&a, "id"), &json!({"expected_revision":0})).unwrap();
        let page = crate::documents::update(
            &c,
            "a",
            text(&page, "id"),
            &json!({"title":"人工修改","expected_revision":0}),
        )
        .unwrap();
        assert_eq!(
            clip(&c, "a", i, text(&a, "id"), &json!({"expected_revision":0})).unwrap(),
            page
        );
        assert!(text(&page, "content").contains("原文"));
        delete_annotation(&c, "a", i, text(&a, "id"), &json!({"expected_revision":0})).unwrap();
        assert_eq!(annotate(&c, "a", i, &input).unwrap_err().code, "conflict");
        assert_eq!(
            crate::documents::read(&c, "a", text(&page, "id")).unwrap(),
            page
        );
    }
    #[test]
    fn range_validation_and_transaction_rollback_leave_no_partial_receipts() {
        let mut c = db();
        let v = seed(&c, "一\n二", true);
        let i = text(&v, "id");
        let mut input = command(&v);
        input["end_line"] = json!(3);
        assert!(annotate(&c, "a", i, &input).is_err());
        input["end_line"] = json!(2);
        input["source_id"] = json!("wrong");
        assert!(annotate(&c, "a", i, &input).is_err());
        {
            let tx = c.transaction().unwrap();
            annotate(&tx, "a", i, &command(&v)).unwrap();
        }
        assert!(
            annotations(&c, "a", i).unwrap()["annotations"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        annotate(&c, "a", i, &command(&v)).unwrap();
        delete(&c, "a", i, &json!({"expected_revision":0})).unwrap();
        for table in [
            "native_reading_sources",
            "native_reading_progress",
            "native_reading_annotations",
            "native_reading_receipts",
        ] {
            assert_eq!(
                c.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r
                    .get::<_, i64>(0))
                    .unwrap(),
                0
            );
        }
    }
    #[test]
    fn v5_upgrade_keeps_authored_data_and_backs_up_before_adding_reading_tables() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("native.sqlite3");
        let page;
        {
            let c = Connection::open(&path).unwrap();
            c.execute_batch(
                include_str!("schema.sql")
                    .split("-- Reading owns")
                    .next()
                    .unwrap(),
            )
            .unwrap();
            c.pragma_update(None, "user_version", 5).unwrap();
            page = crate::documents::create(
                &c,
                "a",
                &json!({"title":"迁移前的笔记","content":"保留原文"}),
            )
            .unwrap();
        }
        let store = crate::store::Store::open(&path).unwrap();
        let c = store.connect().unwrap();
        assert_eq!(
            c.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
                .unwrap(),
            crate::backup::SCHEMA_VERSION
        );
        assert_eq!(
            crate::documents::read(&c, "a", text(&page, "id")).unwrap(),
            page
        );
        assert!(
            list(&c, "a", 0).unwrap()["items"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        let backup = std::fs::read_dir(directory.path().join("backups"))
            .unwrap()
            .map(|e| e.unwrap().path())
            .find(|p| p.extension().is_some_and(|x| x == "sqlite3"))
            .unwrap();
        let prior = Connection::open(backup).unwrap();
        assert_eq!(
            prior
                .pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
                .unwrap(),
            5
        );
        assert_eq!(
            crate::documents::read(&prior, "a", text(&page, "id")).unwrap(),
            page
        );
        assert_eq!(
            prior
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_schema WHERE name='native_reading_items'",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
            0
        );
    }
    #[test]
    fn paged_library_deduplicates_snapshots_and_deleting_one_copy_keeps_other_readers() {
        let c = db();
        let docs = (0..100)
            .map(|n| knowledge::Document {
                path: format!("{n}.txt"),
                content: "共享原文".into(),
                original: b"same-original".to_vec(),
            })
            .collect::<Vec<_>>();
        knowledge::ingest(&c, "a", "教材", &docs, true).unwrap();
        let mut items = Vec::new();
        for file in knowledge::files(&c, "a", "教材").unwrap() {
            items.push(
                add(
                    &c,
                    "a",
                    &json!({"kb":"教材","path":file["name"],"hash":file["sha256"]}),
                )
                .unwrap(),
            );
        }
        knowledge::ingest(
            &c,
            "a",
            "教材",
            &[knowledge::Document {
                path: "another.txt".into(),
                content: "共享原文".into(),
                original: b"same-original".to_vec(),
            }],
            false,
        )
        .unwrap();
        let hash = &knowledge::files(&c, "a", "教材")
            .unwrap()
            .into_iter()
            .find(|f| f["name"] == "another.txt")
            .unwrap()["sha256"];
        add(
            &c,
            "a",
            &json!({"kb":"教材","path":"another.txt","hash":hash}),
        )
        .unwrap();
        let first = list(&c, "a", 0).unwrap();
        assert_eq!(first["items"].as_array().unwrap().len(), 100);
        assert_eq!(first["next_offset"], 100);
        assert_eq!(
            list(&c, "a", 100).unwrap()["items"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            c.query_row("SELECT COUNT(*) FROM native_reading_sources", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            1
        );
        delete(
            &c,
            "a",
            text(&items[0], "id"),
            &json!({"expected_revision":0}),
        )
        .unwrap();
        assert_eq!(source(&c, "a", text(&items[1], "id")).unwrap(), "共享原文");
        assert!(list(&c, "a", 100001).is_err());
    }
}

#[cfg(test)]
mod web_and_tutor_tests {
    use super::*;
    fn db() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
        c.execute_batch(include_str!("schema.sql")).unwrap();
        c
    }
    fn snapshot() -> crate::web_reading::Snapshot {
        crate::web_reading::Snapshot {
            requested_url: "https://example.com/article".into(),
            url: "https://example.com/final".into(),
            title: "Web title".into(),
            mime: "text/html".into(),
            content: "Energy is conserved.\nSecond source line.".into(),
            original: b"<article>Energy is conserved.</article>".to_vec(),
        }
    }
    #[test]
    fn web_snapshots_and_receipts_preserve_edits_without_refetching_or_resurrecting() {
        let c = db();
        let input = json!({"url":"https://example.com/article#part","command_id":"web-1"});
        let item = add_web(&c, "a", &input, snapshot()).unwrap();
        assert_eq!(item["source_url"], "https://example.com/final");
        assert_eq!(
            original(&c, "a", text(&item, "id")).unwrap().0,
            "web-snapshot.html"
        );
        let renamed = rename(
            &c,
            "a",
            text(&item, "id"),
            &json!({"title":"Human title","expected_revision":0}),
        )
        .unwrap();
        assert_eq!(web_receipt(&c, "a", &input).unwrap().unwrap(), renamed);
        assert_eq!(
            add_web(
                &c,
                "a",
                &json!({"url":"https://example.com/article","command_id":"web-2"}),
                snapshot()
            )
            .unwrap(),
            renamed
        );
        assert!(
            web_receipt(
                &c,
                "a",
                &json!({"url":"https://other.example.com/article","command_id":"web-1"})
            )
            .is_err()
        );
        assert!(web_receipt(&c, "other", &input).unwrap().is_none());
        delete(&c, "a", text(&item, "id"), &json!({"expected_revision":1})).unwrap();
        assert!(web_receipt(&c, "a", &input).is_err());
    }
    #[test]
    fn tutor_context_is_frozen_numbered_and_workspace_bound() {
        let c = db();
        let item = add_web(
            &c,
            "a",
            &json!({"url":"https://example.com/article","command_id":"web-1"}),
            snapshot(),
        )
        .unwrap();
        let input =
            json!({"item_id":item["id"],"source_id":item["source_id"],"start_line":1,"end_line":1});
        let context = tutor_context(&c, "a", &input).unwrap();
        let data: Value = serde_json::from_str(&context.source).unwrap();
        assert_eq!(
            data["excerpt"],
            json!([{"line":1,"text":"Energy is conserved."}])
        );
        assert!(
            context.target["citation_base"]
                .as_str()
                .unwrap()
                .contains("dt_workspace=a")
        );
        assert!(tutor_context(&c, "other", &input).is_err());
        let mut changed = input.clone();
        changed["source_id"] = json!("wrong");
        assert!(tutor_context(&c, "a", &changed).is_err());
        changed = input;
        changed["end_line"] = json!(3);
        assert!(tutor_context(&c, "a", &changed).is_err());
    }
}
