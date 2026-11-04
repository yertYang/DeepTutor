//! Authored pages, independent of saved conversation favorites. A write and its
//! recoverable revision always commit together in the caller's transaction.
use crate::{
    Error, Result,
    config::{Settings, read_json, safe_path},
    store::{Store, now, text},
};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde_json::{Value, json};
use std::collections::HashSet;

const MAX_BODY: usize = 1024 * 1024;
pub const BLOCK_TYPES: [&str; 10] = [
    "paragraph",
    "heading",
    "bulletListItem",
    "numberedListItem",
    "checkListItem",
    "toggleListItem",
    "quote",
    "codeBlock",
    "table",
    "divider",
];

fn valid_id(id: &str) -> bool {
    (8..=32).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_hexdigit())
}
fn string(input: &Value, key: &str, max: usize) -> Result<String> {
    crate::learning::string(input, key, max, false)
}
fn object(input: &Value, allowed: &[&str]) -> Result<()> {
    if input
        .as_object()
        .is_none_or(|o| o.keys().any(|k| !allowed.contains(&k.as_str())))
    {
        return Err(Error::invalid("Unexpected page field."));
    }
    Ok(())
}
fn decode(raw: String) -> Result<Value> {
    serde_json::from_str(&raw).map_err(|_| Error::conflict("Invalid stored page."))
}
pub fn read(conn: &Connection, scope: &str, id: &str) -> Result<Value> {
    if !valid_id(id) {
        return Err(Error::missing());
    }
    let raw: Option<String> = conn
        .query_row(
            "SELECT payload_json FROM native_documents WHERE workspace_id=? AND id=?",
            params![scope, id],
            |r| r.get(0),
        )
        .optional()?;
    decode(raw.ok_or_else(Error::missing)?)
}
fn save(conn: &Connection, scope: &str, page: &Value) -> Result<()> {
    let raw = page.to_string();
    conn.execute("INSERT INTO native_documents(workspace_id,id,payload_json) VALUES (?,?,?) ON CONFLICT(workspace_id,id) DO UPDATE SET payload_json=excluded.payload_json",
        params![scope,text(page,"id"),raw])?;
    conn.execute("INSERT INTO native_document_versions(workspace_id,document_id,revision,payload_json) VALUES (?,?,?,?)",
        params![scope,text(page,"id"),page["revision"].as_i64().ok_or_else(|| Error::conflict("Invalid page revision."))?,raw])?;
    Ok(())
}
fn summary(mut page: Value) -> Value {
    let preview = text(&page, "content").chars().take(160).collect::<String>();
    if let Some(o) = page.as_object_mut() {
        o.remove("blocks");
        o.remove("content");
        o.insert("preview".into(), json!(preview));
    }
    page
}
pub fn list(conn: &Connection, scope: &str) -> Result<Vec<Value>> {
    let rows = conn.prepare("SELECT json_set(json_remove(payload_json,'$.blocks','$.content'),'$.preview',substr(json_extract(payload_json,'$.content'),1,160)) FROM native_documents WHERE workspace_id=? ORDER BY json_extract(payload_json,'$.updated_at') DESC,id")?
        .query_map([scope],|r|r.get::<_,String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
    rows.into_iter().map(decode).collect()
}
fn check_revision(page: &Value, input: &Value) -> Result<()> {
    if input.get("expected_revision").and_then(Value::as_i64) != page["revision"].as_i64() {
        return Err(Error::conflict(
            "This page changed elsewhere. Reload before saving.",
        ));
    }
    Ok(())
}
fn parent(conn: &Connection, scope: &str, id: &str, value: &Value) -> Result<Value> {
    let mut cursor = match value {
        Value::Null => return Ok(Value::Null),
        Value::String(s) if valid_id(s) => s.clone(),
        _ => return Err(Error::invalid("Invalid parent page.")),
    };
    let mut seen = HashSet::from([id.to_string()]);
    while !cursor.is_empty() {
        if !seen.insert(cursor.clone()) || seen.len() > 256 {
            return Err(Error::invalid(
                "Pages cannot contain themselves or exceed 256 ancestors.",
            ));
        }
        let ancestor = read(conn, scope, &cursor)?;
        cursor = text(&ancestor, "parent_id").to_string();
    }
    Ok(value.clone())
}
fn title(value: &Value, content: &str) -> Result<String> {
    let title = if value.is_null() {
        String::new()
    } else {
        string(&json!({"title":value}), "title", 1000)?
    };
    Ok(if title.trim().is_empty() {
        content
            .lines()
            .find(|s| !s.trim().is_empty())
            .map(|s| {
                s.trim()
                    .trim_start_matches('#')
                    .trim()
                    .chars()
                    .take(120)
                    .collect()
            })
            .filter(|s: &String| !s.is_empty())
            .unwrap_or_else(|| "Untitled draft".into())
    } else {
        title.trim().to_string()
    })
}

pub fn safe_link(s: &str) -> bool {
    !s.chars().any(char::is_control)
        && !s.contains('\\')
        && (s.is_empty()
            || (s.starts_with('/') && !s.starts_with("//"))
            || s.starts_with('#')
            || ["http://", "https://", "mailto:"]
                .iter()
                .any(|p| s.to_ascii_lowercase().starts_with(p)))
}
fn inline(value: &Value, depth: usize) -> Result<String> {
    if depth > 16 {
        return Err(Error::invalid("Inline nesting exceeds 16 levels."));
    }
    let items = value
        .as_array()
        .ok_or_else(|| Error::invalid("Invalid inline content."))?;
    let mut result = String::new();
    for item in items {
        let mut s = match text(item, "type") {
            "text" => {
                let s = item["text"]
                    .as_str()
                    .ok_or_else(|| Error::invalid("Invalid text."))?;
                if item.get("styles").is_some_and(|v| !v.is_object()) {
                    return Err(Error::invalid("Invalid text styles."));
                }
                s.to_string()
            }
            "link" if item["href"].as_str().is_some_and(safe_link) => format!(
                "[{}]({})",
                inline(&item["content"], depth + 1)?,
                text(item, "href")
            ),
            _ => return Err(Error::invalid("Unsupported inline content or link.")),
        };
        for (style, mark) in [
            ("code", "`"),
            ("bold", "**"),
            ("italic", "*"),
            ("strike", "~~"),
        ] {
            if item["styles"][style] == true {
                s = format!("{mark}{s}{mark}");
            }
        }
        result.push_str(&s);
    }
    Ok(result)
}
/// Validate the complete tree before deriving the read-only Markdown projection.
pub fn markdown(blocks: &Value) -> Result<String> {
    if blocks.to_string().len() > MAX_BODY {
        return Err(Error::invalid("Page exceeds the 1 MiB block limit."));
    }
    fn visit(blocks: &Value, depth: usize, ids: &mut HashSet<String>) -> Result<String> {
        if depth > 16 {
            return Err(Error::invalid("Block nesting exceeds 16 levels."));
        }
        let blocks = blocks
            .as_array()
            .ok_or_else(|| Error::invalid("Expected a block array."))?;
        let mut output = Vec::new();
        for block in blocks {
            let id = text(block, "id");
            if id.is_empty()
                || id.len() > 128
                || !id
                    .chars()
                    .all(|c| c.is_alphanumeric() || c == '_' || c == '-')
                || !ids.insert(id.into())
                || ids.len() > 10000
            {
                return Err(Error::invalid(
                    "Blocks require unique IDs; at most 10000 blocks.",
                ));
            }
            let kind = text(block, "type");
            if !BLOCK_TYPES.contains(&kind) {
                return Err(Error::invalid(
                    "This block type is not supported by the native editor.",
                ));
            }
            let props = block.get("props").cloned().unwrap_or(json!({}));
            if props.as_object().is_none_or(|o| {
                o.values()
                    .any(|v| !(v.is_string() || v.is_boolean() || v.is_number()))
            }) {
                return Err(Error::invalid("Invalid block properties."));
            }
            let content = block.get("content").cloned().unwrap_or(json!([]));
            let mut body = if matches!(kind, "table" | "divider") {
                String::new()
            } else {
                inline(&content, 0)?
            };
            match kind {
                "heading" => {
                    let level = props
                        .get("level")
                        .unwrap_or(&json!(1))
                        .as_u64()
                        .filter(|n| (1..=6).contains(n))
                        .ok_or_else(|| Error::invalid("Invalid heading level."))?;
                    body = format!("{} {body}", "#".repeat(level as usize));
                }
                "bulletListItem" | "toggleListItem" => body = format!("- {body}"),
                "numberedListItem" => body = format!("1. {body}"),
                "checkListItem" => {
                    if props.get("checked").is_some_and(|v| !v.is_boolean()) {
                        return Err(Error::invalid("Invalid checkbox state."));
                    }
                    body = format!(
                        "- [{}] {body}",
                        if props["checked"] == true { "x" } else { " " }
                    );
                }
                "quote" => {
                    body = body
                        .lines()
                        .map(|l| format!("> {l}"))
                        .collect::<Vec<_>>()
                        .join("\n")
                }
                "codeBlock" => {
                    let longest = body.split(|c| c != '`').map(str::len).max().unwrap_or(0);
                    let fence = "`".repeat(3.max(longest + 1));
                    let language = text(&props, "language").replace(['\n', '\r', '`'], "");
                    body = format!("{fence}{language}\n{body}\n{fence}");
                }
                "divider" => body = "---".into(),
                "table" => {
                    let rows = content["rows"]
                        .as_array()
                        .filter(|r| !r.is_empty() && r.len() <= 1000)
                        .ok_or_else(|| Error::invalid("Invalid table."))?;
                    let mut width = None;
                    for (index, row) in rows.iter().enumerate() {
                        let cells = row["cells"]
                            .as_array()
                            .filter(|c| !c.is_empty() && c.len() <= 100)
                            .ok_or_else(|| Error::invalid("Invalid table row."))?;
                        if width.is_some_and(|w| w != cells.len()) {
                            return Err(Error::invalid("Table rows must have equal width."));
                        }
                        width = Some(cells.len());
                        let cells = cells
                            .iter()
                            .map(|c| {
                                inline(if c.is_object() { &c["content"] } else { c }, 0)
                                    .map(|s| s.replace('|', "\\|").replace('\n', "<br>"))
                            })
                            .collect::<Result<Vec<_>>>()?;
                        body.push_str(&format!("| {} |\n", cells.join(" | ")));
                        if index == 0 {
                            body.push_str(&format!("|{}\n", " --- |".repeat(cells.len())));
                        }
                    }
                }
                _ => {}
            }
            if !body.is_empty() {
                output.push(body);
            }
            if let Some(children) = block.get("children") {
                let nested = visit(children, depth + 1, ids)?;
                if !nested.is_empty() {
                    output.push(
                        nested
                            .lines()
                            .map(|l| format!("  {l}"))
                            .collect::<Vec<_>>()
                            .join("\n"),
                    );
                }
            }
        }
        Ok(output.join("\n\n"))
    }
    let result = visit(blocks, 0, &mut HashSet::new())?;
    if result.len() > MAX_BODY {
        return Err(Error::invalid("Page exceeds the 1 MiB text limit."));
    }
    Ok(result)
}

pub fn create(conn: &Connection, scope: &str, input: &Value) -> Result<Value> {
    object(input, &["title", "content", "blocks", "parent_id"])?;
    let id = uuid::Uuid::new_v4().simple().to_string();
    let blocks = input.get("blocks").cloned().unwrap_or(Value::Null);
    let content = if blocks.is_null() {
        string(input, "content", MAX_BODY)?
    } else {
        markdown(&blocks)?
    };
    let page = json!({"id":id,"title":title(&input["title"],&content)?,"content":content,"blocks":blocks,"schema_version":1,"revision":0,"parent_id":parent(conn,scope,&id,&input["parent_id"])?,"icon":"","cover":"","pinned":false,"created_at":now(),"updated_at":now()});
    save(conn, scope, &page)?;
    Ok(page)
}
pub fn import_docx(
    conn: &Connection,
    scope: &str,
    name: &str,
    bytes: &[u8],
    command: &str,
) -> Result<Value> {
    use sha2::{Digest, Sha256};
    if !name.to_ascii_lowercase().ends_with(".docx")
        || name.len() > 255
        || name.contains(['/', '\\', '\0'])
        || command.is_empty()
        || command.len() > 128
    {
        return Err(Error::invalid(
            "Choose one DOCX file and a valid import submission ID.",
        ));
    }
    let digest = format!(
        "{:x}",
        Sha256::digest([name.as_bytes(), b"\0", bytes].concat())
    );
    let receipt:Option<(String,String)>=conn.query_row("SELECT request_digest,document_id FROM native_document_import_receipts WHERE workspace_id=? AND command_id=?",params![scope,command],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
    if let Some((existing, id)) = receipt {
        if existing != digest {
            return Err(Error::conflict(
                "Import submission ID already belongs to another document.",
            ));
        }
        return read(conn, scope, &id);
    }
    let content = crate::document_parser::parse(name, bytes)?;
    let title = name.get(..name.len() - 5).unwrap_or(name);
    let page = create(conn, scope, &json!({"title":title,"content":content}))?;
    conn.execute(
        "INSERT INTO native_document_import_receipts VALUES (?,?,?,?)",
        params![scope, command, digest, text(&page, "id")],
    )?;
    Ok(page)
}
pub fn update(conn: &Connection, scope: &str, id: &str, input: &Value) -> Result<Value> {
    object(
        input,
        &[
            "title",
            "content",
            "blocks",
            "parent_id",
            "icon",
            "cover",
            "pinned",
            "expected_revision",
        ],
    )?;
    let mut page = read(conn, scope, id)?;
    check_revision(&page, input)?;
    if let Some(blocks) = input.get("blocks") {
        if blocks.is_null() {
            return Err(Error::invalid("Blocks cannot be cleared with null."));
        }
        page["content"] = json!(markdown(blocks)?);
        page["blocks"] = blocks.clone();
    } else if let Some(content) = input.get("content") {
        if !page["blocks"].is_null() {
            return Err(Error::conflict("Edit this page through its block editor."));
        }
        page["content"] = json!(string(&json!({"content":content}), "content", MAX_BODY)?);
    }
    if let Some(value) = input.get("title") {
        page["title"] = json!(title(value, text(&page, "content"))?);
    }
    if let Some(value) = input.get("parent_id") {
        page["parent_id"] = parent(conn, scope, id, value)?;
    }
    for (key, max) in [("icon", 100), ("cover", 2000)] {
        if input.get(key).is_some() {
            page[key] = json!(string(input, key, max)?);
        }
    }
    if let Some(value) = input.get("pinned") {
        if !value.is_boolean() {
            return Err(Error::invalid("Invalid pinned state."));
        }
        page["pinned"] = value.clone();
    }
    page["revision"] = json!(
        page["revision"]
            .as_i64()
            .ok_or_else(|| Error::conflict("Invalid page revision."))?
            .checked_add(1)
            .ok_or_else(|| Error::conflict("Page revision limit reached."))?
    );
    page["updated_at"] = json!(now());
    save(conn, scope, &page)?;
    Ok(page)
}
pub fn delete(conn: &Connection, scope: &str, id: &str) -> Result<Value> {
    let page = read(conn, scope, id)?;
    let children = list(conn, scope)?
        .into_iter()
        .filter(|p| p["parent_id"] == id)
        .collect::<Vec<_>>();
    for child in children {
        update(
            conn,
            scope,
            text(&child, "id"),
            &json!({"parent_id":page["parent_id"],"expected_revision":child["revision"]}),
        )?;
    }
    conn.execute(
        "DELETE FROM native_documents WHERE workspace_id=? AND id=?",
        params![scope, id],
    )?;
    Ok(json!({"deleted":true}))
}
pub fn versions(conn: &Connection, scope: &str, id: &str) -> Result<Value> {
    read(conn, scope, id)?;
    let rows = conn.prepare("SELECT revision,json_extract(payload_json,'$.updated_at'),json_extract(payload_json,'$.title') FROM native_document_versions WHERE workspace_id=? AND document_id=? ORDER BY revision DESC LIMIT 100")?
        .query_map(params![scope,id],|r|Ok(json!({"revision":r.get::<_,i64>(0)?,"updated_at":r.get::<_,f64>(1)?,"title":r.get::<_,String>(2)?})))?.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(json!({"versions":rows}))
}
pub fn restore(
    conn: &Connection,
    scope: &str,
    id: &str,
    revision: i64,
    input: &Value,
) -> Result<Value> {
    object(input, &["expected_revision"])?;
    let current = read(conn, scope, id)?;
    check_revision(&current, input)?;
    let raw: Option<String> = conn.query_row("SELECT payload_json FROM native_document_versions WHERE workspace_id=? AND document_id=? AND revision=?",params![scope,id,revision],|r|r.get(0)).optional()?;
    let mut page = decode(raw.ok_or_else(Error::missing)?)?;
    // Keep the current location; restoring content must not recreate deleted
    // parents or a historical cycle. Historical revisions remain immutable.
    page["parent_id"] = current["parent_id"].clone();
    page["created_at"] = current["created_at"].clone();
    page["revision"] = json!(
        current["revision"]
            .as_i64()
            .and_then(|v| v.checked_add(1))
            .ok_or_else(|| Error::conflict("Invalid page revision."))?
    );
    page["updated_at"] = json!(now());
    save(conn, scope, &page)?;
    Ok(page)
}
pub fn backlinks(conn: &Connection, scope: &str, id: &str) -> Result<Value> {
    read(conn, scope, id)?;
    let marker = format!("/co-writer/{id}");
    let rows = conn.prepare("SELECT payload_json FROM native_documents WHERE workspace_id=? AND id<>? AND instr(json_extract(payload_json,'$.content'),?)>0")?
        .query_map(params![scope,id,marker],|r|r.get::<_,String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(
        json!({"pages":rows.into_iter().map(|r|decode(r).map(summary)).collect::<Result<Vec<_>>>()?}),
    )
}

pub fn bootstrap(settings: &Settings, store: &Store, scope: &str) -> Result<()> {
    let mut conn = store.connect()?;
    if conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM native_document_imports WHERE workspace_id=?)",
        [scope],
        |r| r.get::<_, bool>(0),
    )? {
        return Ok(());
    }
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    if tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM native_document_imports WHERE workspace_id=?)",
        [scope],
        |r| r.get::<_, bool>(0),
    )? {
        return Ok(());
    }
    if let Some(root) = crate::learning::legacy_root(settings, scope)? {
        let directory = root.join("co-writer/documents");
        safe_path(&root, &directory)?;
        if directory.is_dir() {
            let mut total = 0u64;
            let mut count = 0;
            for entry in std::fs::read_dir(&directory)? {
                let dir = entry?.path();
                let Some(id) = dir
                    .file_name()
                    .and_then(|s| s.to_str())
                    .and_then(|s| s.strip_prefix("doc_"))
                else {
                    continue;
                };
                if !valid_id(id) {
                    return Err(Error::conflict("Invalid legacy page directory."));
                }
                let path = dir.join("manifest.json");
                safe_path(&root, &path)?;
                let size = std::fs::metadata(&path)?.len();
                total += size;
                count += 1;
                if size > 4 * 1024 * 1024 || total > 32 * 1024 * 1024 || count > 10000 {
                    return Err(Error::conflict("Legacy pages exceed the import limit."));
                }
                let mut page = read_json(&path, Value::Null)?;
                if page["id"] != id
                    || !page.is_object()
                    || !page["content"].is_string()
                    || !page["title"].is_string()
                    || page
                        .get("blocks")
                        .is_some_and(|b| !b.is_null() && !b.is_array())
                {
                    return Err(Error::conflict(
                        "A legacy page is unreadable. Restore it before importing.",
                    ));
                }
                for key in ["created_at", "updated_at"] {
                    if !page[key].is_number() {
                        return Err(Error::conflict("Invalid legacy page timestamp."));
                    }
                }
                for (key, default) in [
                    ("revision", json!(0)),
                    ("schema_version", json!(1)),
                    ("blocks", Value::Null),
                    ("parent_id", Value::Null),
                    ("icon", json!("")),
                    ("cover", json!("")),
                    ("pinned", json!(false)),
                ] {
                    if page.get(key).is_none() {
                        page[key] = default;
                    }
                }
                if page["revision"].as_i64().is_none_or(|v| v < 0)
                    || !page["icon"].is_string()
                    || !page["cover"].is_string()
                    || !page["pinned"].is_boolean()
                    || (!page["parent_id"].is_null()
                        && !page["parent_id"].as_str().is_some_and(valid_id))
                {
                    return Err(Error::conflict("Invalid legacy page metadata."));
                }
                // Preserve legacy custom blocks and their exact projection.
                // The native editor opens unsupported trees as read-only.
                save(&tx, scope, &page)?;
            }
            for page in list(&tx, scope)? {
                parent(&tx, scope, text(&page, "id"), &page["parent_id"])?;
            }
        }
    }
    tx.execute(
        "INSERT INTO native_document_imports(workspace_id) VALUES (?)",
        [scope],
    )?;
    tx.commit()?;
    Ok(())
}

#[cfg(test)]
mod native_import_tests {
    use super::*;
    use std::io::{Cursor, Write};
    fn docx() -> Vec<u8> {
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        zip.start_file(
            "word/document.xml",
            zip::write::SimpleFileOptions::default(),
        )
        .unwrap();
        zip.write_all(b"<w:document xmlns:w='word'><w:body><w:p><w:r><w:t>Source fact</w:t></w:r></w:p></w:body></w:document>").unwrap();
        zip.finish().unwrap().into_inner()
    }
    #[test]
    fn imported_word_text_is_editable_scoped_and_retry_safe() {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(include_str!("schema.sql")).unwrap();
        let bytes = docx();
        let page = import_docx(&c, "w", "lecture.docx", &bytes, "import-1").unwrap();
        assert_eq!(page["title"], "lecture");
        assert!(text(&page, "content").contains("Source fact"));
        let edited = update(
            &c,
            "w",
            text(&page, "id"),
            &json!({"content":"Human edit","expected_revision":page["revision"]}),
        )
        .unwrap();
        assert_eq!(
            import_docx(&c, "w", "lecture.docx", &bytes, "import-1").unwrap(),
            edited
        );
        assert!(import_docx(&c, "w", "changed.docx", &bytes, "import-1").is_err());
        assert_ne!(
            import_docx(&c, "other", "lecture.docx", &bytes, "import-1").unwrap()["id"],
            page["id"]
        );
        assert!(import_docx(&c, "w", "../lecture.docx", &bytes, "import-2").is_err());
        assert!(import_docx(&c, "w", "invalid.docx", b"not a zip", "import-2").is_err());
        assert_eq!(list(&c, "w").unwrap().len(), 1);
    }
}
