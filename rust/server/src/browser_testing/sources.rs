//! Imported test inputs are immutable originals with versioned XLSX results.
use super::*;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashSet};

pub struct Document {
    pub name: String,
    pub bytes: Vec<u8>,
    pub content: String,
}
pub fn import(conn: &Connection, scope: &str, documents: Vec<Document>) -> Result<Value> {
    let mut imported = Vec::new();
    for document in documents {
        let id = format!(
            "src_{:x}",
            Sha256::digest([document.name.as_bytes(), b"\0", &document.bytes].concat())
        );
        conn.execute("INSERT OR IGNORE INTO native_browser_sources(workspace_id,id,name,content,original_content,original,current,created_at) VALUES (?,?,?,?,?,?,?,?)",params![scope,id,document.name,document.content,document.content,document.bytes,document.bytes,now()])?;
        imported.push(metadata(conn, scope, &id)?);
    }
    Ok(json!({"sources":imported}))
}
pub fn metadata(conn: &Connection, scope: &str, id: &str) -> Result<Value> {
    conn.query_row("SELECT name,revision,created_at,length(original),content FROM native_browser_sources WHERE workspace_id=? AND id=?",params![scope,id],|r| {
        let name:String=r.get(0)?; let content:String=r.get(4)?;
        Ok(json!({"id":id,"name":name,"revision":r.get::<_,i64>(1)?,"created_at":r.get::<_,f64>(2)?,"size":r.get::<_,i64>(3)?,"line_count":content.lines().count(),"workbook":name.to_ascii_lowercase().ends_with(".xlsx")}))
    }).optional()?.ok_or_else(Error::missing)
}
pub fn list(conn: &Connection, scope: &str) -> Result<Value> {
    let ids=conn.prepare("SELECT id FROM native_browser_sources WHERE workspace_id=? ORDER BY created_at DESC,id LIMIT 100")?.query_map([scope],|r|r.get::<_,String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(json!(
        ids.iter()
            .map(|id| metadata(conn, scope, id))
            .collect::<Result<Vec<_>>>()?
    ))
}
pub fn read(conn: &Connection, scope: &str, id: &str, offset: u64, limit: u64) -> Result<Value> {
    if offset > 2_000_000 || !(1..=400).contains(&limit) {
        return Err(Error::invalid(
            "Use offset >= 0 and a source page limit of 1-400 lines.",
        ));
    }
    let mut meta = metadata(conn, scope, id)?;
    let content: String = conn.query_row(
        "SELECT content FROM native_browser_sources WHERE workspace_id=? AND id=?",
        params![scope, id],
        |r| r.get(0),
    )?;
    let mut lines = Vec::new();
    let mut size = 0usize;
    for line in content.lines().skip(offset as usize).take(limit as usize) {
        if size + line.len() > 40000 {
            if lines.is_empty() {
                return Err(Error::invalid(
                    "A source line exceeds the tool page limit. Split long text lines before importing.",
                ));
            }
            break;
        }
        size += line.len();
        lines.push(line);
    }
    let end = offset + lines.len() as u64;
    meta["content"] = json!(lines.join("\n"));
    meta["offset"] = json!(offset);
    meta["next_offset"] = if end < meta["line_count"].as_u64().unwrap_or(0) {
        json!(end)
    } else {
        Value::Null
    };
    Ok(meta)
}
pub fn original(conn: &Connection, scope: &str, id: &str) -> Result<(String, Vec<u8>)> {
    conn.query_row(
        "SELECT name,original FROM native_browser_sources WHERE workspace_id=? AND id=?",
        params![scope, id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )
    .optional()?
    .ok_or_else(Error::missing)
}
pub fn exports(conn: &Connection, scope: &str) -> Result<Value> {
    let rows=conn.prepare("SELECT payload_json FROM native_browser_exports WHERE workspace_id=? ORDER BY created_at DESC LIMIT 100")?.query_map([scope],|r|r.get::<_,String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(json!(
        rows.into_iter().map(decode).collect::<Result<Vec<_>>>()?
    ))
}
pub fn download(conn: &Connection, scope: &str, id: &str) -> Result<(String, Vec<u8>)> {
    conn.query_row(
        "SELECT name,data FROM native_browser_exports WHERE workspace_id=? AND id=?",
        params![scope, id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )
    .optional()?
    .ok_or_else(Error::missing)
}

#[derive(Clone, Debug)]
pub struct Prepared {
    pub source_id: String,
    pub revision: u64,
    pub bytes: Vec<u8>,
    pub content: String,
    pub rows: Value,
    pub session: String,
}

fn row_results(conn: &Connection, scope: &str, session: &str, rows: &Value) -> Result<Vec<Value>> {
    let rows = rows
        .as_array()
        .filter(|rows| !rows.is_empty() && rows.len() <= 200)
        .ok_or_else(|| Error::invalid("Update 1-200 Excel rows per call."))?;
    let mut seen = HashSet::new();
    let mut results = Vec::new();
    for row in rows {
        object(row, &["row", "run_id", "reason"])?;
        let number = row["row"]
            .as_u64()
            .filter(|r| (1..=1048576).contains(r))
            .ok_or_else(|| Error::invalid("Use the actual Excel row number."))?;
        if !seen.insert(number) {
            return Err(Error::invalid("An Excel row was selected twice."));
        }
        if let Some(run_id) = row["run_id"].as_str().filter(|s| !s.is_empty()) {
            let owned:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM native_tool_invocations i JOIN turns t ON t.id=i.turn_id WHERE i.invocation_id=? AND i.name='browser_test' AND i.status='completed' AND t.session_id=?)",params![run_id,session],|r|r.get(0))?;
            if !owned {
                return Err(Error::invalid(
                    "Use a real browser report produced by this conversation.",
                ));
            }
            let run = super::report(conn, scope, run_id)?;
            let status = match text(&run, "status") {
                "passed" => "通过",
                "failed" => "失败",
                "inspected" => "已操作，未断言",
                "cancelled" | "interrupted" => "未完成",
                _ => {
                    return Err(Error::invalid(
                        "Wait for the browser report to finish before updating Excel.",
                    ));
                }
            };
            let report = &run["report"];
            let steps = report["steps"].as_array().cloned().unwrap_or_default();
            let detail = steps
                .iter()
                .map(|s| {
                    format!(
                        "{}: {}{}",
                        text(s, "action"),
                        text(s, "status"),
                        s["error"]
                            .as_str()
                            .map(|v| format!(" — {v}"))
                            .unwrap_or_default()
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            let actual = format!(
                "{}\n{}\n{}{}",
                text(report, "url"),
                text(report, "title"),
                detail,
                report["error"]
                    .as_str()
                    .map(|v| format!("\n{v}"))
                    .unwrap_or_default()
            );
            results.push(json!({"row":number,"status":status,"actual":actual,"report":scoped_link(&format!("/browser-testing?run={run_id}"),scope)}));
        } else {
            let reason = crate::learning::string(row, "reason", 2000, true)?;
            results.push(json!({"row":number,"status":"未执行","actual":reason,"report":""}));
        }
    }
    Ok(results)
}
pub fn prepare(conn: &Connection, scope: &str, session: &str, input: &Value) -> Result<Prepared> {
    object(
        input,
        &[
            "action",
            "source_id",
            "sheet",
            "rows",
            "expected_revision",
            "header_row",
            "status_column",
            "actual_column",
            "report_column",
        ],
    )?;
    let id = crate::learning::string(input, "source_id", 128, true)?;
    let meta = metadata(conn, scope, &id)?;
    if meta["workbook"] != true {
        return Err(Error::invalid(
            "Choose an imported .xlsx workbook for result updates.",
        ));
    }
    revision(input, meta["revision"].as_u64().unwrap_or(0))?;
    let sheet = crate::learning::string(input, "sheet", 128, true)?;
    let header = input["header_row"].as_u64().unwrap_or(1);
    if !(1..=1048576).contains(&header) {
        return Err(Error::invalid("Invalid Excel header row."));
    }
    let source: String = conn.query_row(
        "SELECT content FROM native_browser_sources WHERE workspace_id=? AND id=?",
        params![scope, id],
        |r| r.get(0),
    )?;
    let bytes: Vec<u8> = conn.query_row(
        "SELECT current FROM native_browser_sources WHERE workspace_id=? AND id=?",
        params![scope, id],
        |r| r.get(0),
    )?;
    let mut selected = false;
    let mut found = false;
    let mut maximum = 0u32;
    let mut headers = HashMap::new();
    for line in source.lines() {
        if let Some(name) = line
            .strip_prefix("[Sheet ")
            .and_then(|v| v.strip_suffix(']'))
        {
            selected = name == sheet;
            found |= selected;
            continue;
        }
        if !selected {
            continue;
        }
        if let Some((reference, value)) = line.split_once('\t') {
            let column_name = reference.trim_end_matches(|c: char| c.is_ascii_digit());
            if let Ok(c) = super::workbook::column(column_name) {
                maximum = maximum.max(c);
                if reference == format!("{column_name}{header}") {
                    headers.insert(value.trim().to_owned(), c);
                }
            }
        }
    }
    if !found {
        return Err(Error::invalid(
            "Worksheet was not found. Read the imported workbook first.",
        ));
    }
    let mut columns = Vec::new();
    for (key, label) in [
        ("status_column", "执行状态"),
        ("actual_column", "实际结果"),
        ("report_column", "测试报告"),
    ] {
        let n = if let Some(name) = input[key].as_str().filter(|v| !v.is_empty()) {
            super::workbook::column(name)?
        } else if let Some(n) = headers.get(label) {
            *n
        } else {
            maximum += 1;
            maximum
        };
        if n > 16384 || columns.contains(&n) {
            return Err(Error::invalid(
                "Choose three distinct Excel result columns within A-XFD.",
            ));
        }
        // Never silently overwrite requirements, steps or expectation columns.
        let existing = headers
            .iter()
            .find(|(_, c)| **c == n)
            .map(|(v, _)| v.as_str());
        if existing.is_some_and(|value| {
            !value.is_empty()
                && ![
                    "执行状态",
                    "实际结果",
                    "测试报告",
                    "状态",
                    "测试结果",
                    "实际执行结果",
                    "报告链接",
                    "Status",
                    "Actual result",
                    "Report",
                    "Actual Result",
                    "Test Status",
                ]
                .contains(&value)
        }) {
            return Err(Error::invalid(
                "A selected result column contains a source-data header. Choose empty or existing result columns.",
            ));
        }
        columns.push(n);
    }
    let results = row_results(conn, scope, session, &input["rows"])?;
    let mut changes: BTreeMap<u32, BTreeMap<u32, String>> = BTreeMap::new();
    for (column, label) in columns.iter().zip(["执行状态", "实际结果", "测试报告"]) {
        changes
            .entry(header as u32)
            .or_default()
            .insert(*column, label.into());
    }
    for result in results {
        let row = result["row"].as_u64().unwrap();
        if row == header {
            return Err(Error::invalid("Do not overwrite the Excel header row."));
        }
        for (column, key) in columns.iter().zip(["status", "actual", "report"]) {
            changes
                .entry(row as u32)
                .or_default()
                .insert(*column, text(&result, key).chars().take(32000).collect());
        }
    }
    let bytes = super::workbook::update(&bytes, &sheet, &changes)?;
    let content = crate::document_parser::parse(text(&meta, "name"), &bytes)?;
    Ok(Prepared {
        source_id: id,
        revision: meta["revision"].as_u64().unwrap_or(0),
        bytes,
        content,
        rows: input["rows"].clone(),
        session: session.into(),
    })
}
pub fn apply(conn: &Connection, scope: &str, id: &str, prepared: Prepared) -> Result<Value> {
    let meta = metadata(conn, scope, &prepared.source_id)?;
    if meta["revision"].as_u64() != Some(prepared.revision) {
        return Err(Error::conflict(
            "The workbook changed. Read the latest source revision before updating.",
        ));
    }
    row_results(conn, scope, &prepared.session, &prepared.rows)?;
    let name = format!(
        "{}-测试结果-v{}.xlsx",
        text(&meta, "name")
            .trim_end_matches(|c: char| c.is_whitespace())
            .rsplit_once('.')
            .map(|(stem, _)| stem)
            .unwrap_or("用例"),
        prepared.revision + 1
    );
    let saved = json!({"id":id,"source_id":prepared.source_id,"name":name,"revision":prepared.revision+1,"updated_rows":prepared.rows.as_array().map_or(0,Vec::len),"created_at":now(),"download_url":scoped_link(&format!("/api/browser-testing/exports/{id}"),scope)});
    conn.execute(
        "INSERT INTO native_browser_exports VALUES (?,?,?,?,?,?,?)",
        params![
            scope,
            id,
            prepared.source_id,
            name,
            prepared.bytes,
            saved.to_string(),
            now()
        ],
    )?;
    conn.execute("UPDATE native_browser_sources SET current=?,content=?,revision=revision+1 WHERE workspace_id=? AND id=?",params![prepared.bytes,prepared.content,scope,prepared.source_id])?;
    Ok(saved)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn workbook_results_require_real_reports_and_preserve_originals_and_revisions() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(&root.path().join("chat.db")).unwrap();
        let conn = store.connect().unwrap();
        let bytes = include_bytes!("../../tests/fixtures/browser-cases.xlsx").to_vec();
        let imported = import(
            &conn,
            "a",
            vec![Document {
                name: "cases.xlsx".into(),
                content: crate::document_parser::parse("cases.xlsx", &bytes).unwrap(),
                bytes: bytes.clone(),
            }],
        )
        .unwrap();
        let id = text(&imported["sources"][0], "id");
        assert!(metadata(&conn, "b", id).is_err());
        let turn = store
            .admit(
                &json!({"workspace_id":"a","content":"test","capability":"test_cases"}),
                "test",
            )
            .unwrap();
        let call = crate::tools::ToolCall {
            id: "call".into(),
            name: "browser_test".into(),
            arguments: r#"{"action":"act"}"#.into(),
        };
        store
            .start_tool(&turn.turn, "actual-report", &call)
            .unwrap();
        super::super::record(&conn,"a","actual-report",None,None,json!({"report":{"status":"failed","url":"https://example.com","steps":[{"action":"expect_text","status":"failed","error":"Expected Alice, got Bob"}]}})).unwrap();
        store
            .commit_tool(&turn.turn, "actual-report", json!({"success":true}).into())
            .unwrap();
        let input = json!({"action":"update_workbook","source_id":id,"sheet":"Cases","expected_revision":0,"rows":[{"row":2,"run_id":"actual-report"},{"row":3,"reason":"Missing login credentials"}]});
        assert!(prepare(&conn, "a", "other-conversation", &input).is_err());
        let prepared = prepare(&conn, "a", &turn.session, &input).unwrap();
        let stale = prepared.clone();
        let result = apply(&conn, "a", "export1", prepared).unwrap();
        assert_eq!(result["revision"], 1);
        assert!(apply(&conn, "a", "export2", stale).is_err());
        assert_eq!(original(&conn, "a", id).unwrap().1, bytes);
        let content = read(&conn, "a", id, 0, 200).unwrap();
        assert!(text(&content, "content").contains("G2\t失败"));
        assert!(text(&content, "content").contains("G3\t未执行"));
        assert!(text(&content, "content").contains("Expected Alice, got Bob"));
        assert!(download(&conn, "b", "export1").is_err());
        let mut next = input.clone();
        next["expected_revision"] = json!(1);
        let prepared = prepare(&conn, "a", &turn.session, &next).unwrap();
        let output = apply(&conn, "a", "export2", prepared).unwrap();
        assert_eq!(output["revision"], 2);
        assert!(!text(&read(&conn, "a", id, 0, 200).unwrap(), "content").contains("J1\t执行状态"));
        let mut fabricated = input.clone();
        fabricated["expected_revision"] = json!(2);
        fabricated["rows"][0]["run_id"] = json!("fake");
        assert!(prepare(&conn, "a", &turn.session, &fabricated).is_err());
    }
}
