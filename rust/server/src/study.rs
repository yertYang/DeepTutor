//! Durable study artifacts. Tool writes are committed with their tool_result.
use crate::{
    Error, Result,
    store::{now, text},
};
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Value, json};

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Target {
    pub plan_id: String,
    pub objective_id: String,
}
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct ProposalContext {
    pub target: Value,
    pub source: String,
}
pub fn proposal(
    conn: &Connection,
    scope: &str,
    document: &Value,
    memory: &Value,
    capability: crate::capability::Capability,
) -> Result<Option<ProposalContext>> {
    use crate::capability::Capability;
    if (capability == Capability::NoteAi && !memory.is_null())
        || (capability == Capability::MemoryConsolidate && !document.is_null())
    {
        return Err(Error::invalid(
            "Select exactly one source for the edit proposal.",
        ));
    }
    let (target, source) = match capability {
        Capability::NoteAi => {
            if document.as_object().is_none_or(|o| {
                o.keys()
                    .any(|k| !matches!(k.as_str(), "document_id" | "expected_revision"))
            }) {
                return Err(Error::invalid("Select a saved note and its revision."));
            }
            let id = field(document, "document_id", 64)?;
            let page = crate::documents::read(conn, scope, &id)?;
            if !page["blocks"].is_null() {
                crate::documents::markdown(&page["blocks"]).map_err(|_|Error::unsupported("AI editing is unavailable for this page's legacy blocks. Export or copy the original before converting it to native blocks."))?;
            }
            if page["revision"] != document["expected_revision"] {
                return Err(Error::conflict(
                    "The selected note changed. Reload it before requesting AI edits.",
                ));
            }
            (
                json!({"document_id":id,"revision":page["revision"],"title":page["title"]}),
                text(&page, "content").to_owned(),
            )
        }
        Capability::MemoryConsolidate => {
            if memory.as_object().is_none_or(|o| {
                o.keys()
                    .any(|k| !matches!(k.as_str(), "layer" | "key" | "expected_revision"))
            }) {
                return Err(Error::invalid("Select a memory document and its revision."));
            }
            let layer = field(memory, "layer", 2)?;
            let key = field(memory, "key", 64)?;
            let page = crate::learning::read_memory(conn, scope, &layer, &key)?;
            if page["revision"] != memory["expected_revision"] {
                return Err(Error::conflict(
                    "The memory document changed. Reload before consolidating.",
                ));
            }
            (
                json!({"layer":layer,"key":key,"revision":page["revision"]}),
                text(&page, "content").to_owned(),
            )
        }
        _ => {
            if !document.is_null() || !memory.is_null() {
                return Err(Error::invalid(
                    "This study mode does not accept a note or memory target.",
                ));
            }
            return Ok(None);
        }
    };
    if (source.is_empty() && capability != Capability::NoteAi) || source.len() > 64 * 1024 {
        return Err(Error::invalid(
            "Select a nonempty source document with at most 64 KiB of text.",
        ));
    }
    Ok(Some(ProposalContext { target, source }))
}
pub fn target(
    conn: &Connection,
    scope: &str,
    value: &Value,
    capability: crate::capability::Capability,
) -> Result<Option<Target>> {
    if value.is_null() {
        return Ok(None);
    }
    if capability != crate::capability::Capability::DeepQuestion {
        return Err(Error::invalid(
            "A learning objective can only be attached to a question-generation turn.",
        ));
    }
    let target: Target = serde_json::from_value(value.clone())
        .map_err(|_| Error::invalid("Invalid learning objective selection."))?;
    let plan = raw(conn, scope, &target.plan_id)?;
    if plan["kind"] != "mastery_path"
        || plan["phase"] != "published"
        || !plan["result"]["units"]
            .as_array()
            .is_some_and(|a| a.iter().any(|u| u["id"] == target.objective_id))
    {
        return Err(Error::missing());
    }
    require_objective_ready(conn, scope, &target.plan_id, &target.objective_id)?;
    Ok(Some(target))
}

#[derive(Clone, Debug)]
pub struct Mutation {
    pub turn: String,
    pub kind: String,
    pub operation: String,
    pub input: Value,
    pub knowledge: Vec<String>,
    pub web: bool,
    pub target: Option<Target>,
    pub proposal: Option<ProposalContext>,
}
fn field(input: &Value, key: &str, max: usize) -> Result<String> {
    crate::learning::string(input, key, max, true)
}
fn strings(input: &Value, key: &str, max: usize) -> Result<Vec<String>> {
    input[key]
        .as_array()
        .filter(|a| !a.is_empty() && a.len() <= max)
        .ok_or_else(|| Error::invalid(format!("{key} must contain 1-{max} items.")))?
        .iter()
        .map(|v| {
            v.as_str()
                .filter(|s| !s.trim().is_empty() && s.len() <= 2000)
                .map(str::to_owned)
                .ok_or_else(|| Error::invalid("Invalid study item."))
        })
        .collect()
}
pub fn definitions(research: bool) -> Vec<Value> {
    let mut tools = vec![
        json!({"name":"study_plan","description":"Commit the title and ordered study objectives before research, question generation or learning-path creation.","parameters":{"type":"object","properties":{"title":{"type":"string"},"objectives":{"type":"array","items":{"type":"string"},"minItems":1,"maxItems":12}},"required":["title","objectives"],"additionalProperties":false}}),
    ];
    if research {
        tools.push(json!({"name":"study_evidence","description":"Record verified evidence from an attached knowledge source (kb_name,path,hash,start_line,end_line) or a URL actually returned by web_search in this turn. Never invent a source. Returns a server-generated evidence ID for the report.","parameters":{"type":"object","properties":{"kb_name":{"type":"string"},"path":{"type":"string"},"hash":{"type":"string"},"start_line":{"type":"integer","minimum":1},"end_line":{"type":"integer","minimum":1},"url":{"type":"string"}},"additionalProperties":false}}));
    }
    tools.push(json!({"name":"study_publish","description":"Persist the final structured artifact. Research: report (Markdown) and citations (recorded evidence IDs). Questions: questions array with prompt, choices (2-6 strings), answer_index (zero-based), explanation, optional plan_id and objective_id. Learning path: units array with title, goals (strings), prerequisites (zero-based indexes of earlier units). Publish only after completing the committed objectives. The final chat answer must summarize this saved artifact.","parameters":{"type":"object","properties":{"report":{"type":"string"},"citations":{"type":"array","items":{"type":"string"}},"questions":{"type":"array","items":{"type":"object","properties":{"prompt":{"type":"string"},"choices":{"type":"array","items":{"type":"string"}},"answer_index":{"type":"integer"},"explanation":{"type":"string"},"plan_id":{"type":"string"},"objective_id":{"type":"string"}},"required":["prompt","choices","answer_index","explanation"],"additionalProperties":false}},"units":{"type":"array","items":{"type":"object","properties":{"title":{"type":"string"},"goals":{"type":"array","items":{"type":"string"}},"prerequisites":{"type":"array","items":{"type":"integer"}}},"required":["title","goals","prerequisites"],"additionalProperties":false}}},"additionalProperties":false}}));
    let text = json!({"type":"object","properties":{"type":{"type":"string","enum":["text"]},"text":{"type":"string"},"styles":{"type":"object","properties":{"bold":{"type":"boolean"},"italic":{"type":"boolean"},"code":{"type":"boolean"},"strike":{"type":"boolean"}},"additionalProperties":false}},"required":["type","text","styles"],"additionalProperties":false});
    let inline = json!({"oneOf":[text.clone(),{"type":"object","properties":{"type":{"type":"string","enum":["link"]},"href":{"type":"string"},"content":{"type":"array","items":text}},"required":["type","href","content"],"additionalProperties":false}]});
    let block = json!({"type":"object","properties":{"id":{"type":"string"},"type":{"type":"string","enum":["paragraph","heading","bulletListItem","numberedListItem","checkListItem","quote","codeBlock","divider"]},"props":{"type":"object","properties":{"level":{"type":"integer","minimum":1,"maximum":3},"language":{"type":"string"},"checked":{"type":"boolean"}},"additionalProperties":false},"content":{"type":"array","items":inline},"children":{"type":"array","maxItems":0,"items":{"type":"object","properties":{},"additionalProperties":false}}},"required":["id","type","props","content","children"],"additionalProperties":false});
    let publish = tools.last_mut().expect("publish tool");
    publish["parameters"]["properties"]["blocks"] =
        json!({"type":"array","minItems":1,"items":block});
    publish["parameters"]["properties"]["content"] = json!({"type":"string"});
    publish["description"] = json!(format!(
        "{} Note AI publishes blocks using the advertised BlockNote schema; memory consolidation publishes content. These are proposals and do not edit the source until the user explicitly reviews and applies them.",
        crate::store::text(publish, "description")
    ));
    tools
}
fn raw(conn: &Connection, scope: &str, id: &str) -> Result<Value> {
    let raw: Option<String> = conn
        .query_row(
            "SELECT payload_json FROM native_study_artifacts WHERE workspace_id=? AND id=?",
            params![scope, id],
            |r| r.get(0),
        )
        .optional()?;
    serde_json::from_str(&raw.ok_or_else(Error::missing)?)
        .map_err(|_| Error::conflict("Invalid stored study artifact."))
}
fn save(conn: &Connection, scope: &str, record: &mut Value) -> Result<()> {
    record["updated_at"] = json!(now());
    record["revision"] = json!(
        record["revision"]
            .as_u64()
            .unwrap_or(0)
            .checked_add(1)
            .ok_or_else(|| Error::conflict("Study revision overflow."))?
    );
    conn.execute("INSERT INTO native_study_artifacts VALUES (?,?,?) ON CONFLICT(workspace_id,id) DO UPDATE SET payload_json=excluded.payload_json",params![scope,text(record,"id"),record.to_string()])?;
    Ok(())
}
fn public(mut record: Value) -> Value {
    if let Some(questions) = record
        .get_mut("result")
        .and_then(|v| v.get_mut("questions"))
        .and_then(Value::as_array_mut)
    {
        for q in questions {
            if let Some(o) = q.as_object_mut() {
                o.remove("answer_index");
                o.remove("explanation");
            }
        }
    }
    record
}
pub fn read(conn: &Connection, scope: &str, id: &str) -> Result<Value> {
    Ok(public(raw(conn, scope, id)?))
}
pub fn list(conn: &Connection, scope: &str) -> Result<Value> {
    let rows=conn.prepare("SELECT payload_json FROM native_study_artifacts WHERE workspace_id=? ORDER BY json_extract(payload_json,'$.updated_at') DESC LIMIT 100")?.query_map([scope],|r|r.get::<_,String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
    let records = rows
        .into_iter()
        .map(|s| {
            serde_json::from_str(&s)
                .map(public)
                .map_err(|_| Error::conflict("Invalid study artifact."))
        })
        .collect::<Result<Vec<_>>>()?;
    let turns=conn.prepare("SELECT t.id,t.session_id,t.status,t.error,t.capability FROM turns t JOIN sessions s ON s.id=t.session_id WHERE COALESCE(json_extract(s.preferences_json,'$.workspace_id'),'')=? AND s.deleted_at IS NULL AND t.capability IN ('deep_research','deep_question','mastery_path','note_ai','memory_consolidate') ORDER BY t.created_at DESC LIMIT 100")?.query_map([scope],|r|Ok(json!({"id":r.get::<_,String>(0)?,"session_id":r.get::<_,String>(1)?,"status":r.get::<_,String>(2)?,"error":r.get::<_,String>(3)?,"capability":r.get::<_,String>(4)?})))?.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(json!({"artifacts":records,"turns":turns}))
}
pub fn dashboard(conn: &Connection, scope: &str) -> Result<Value> {
    let records=conn.prepare("SELECT id,json_extract(payload_json,'$.kind'),json_extract(payload_json,'$.title'),json_extract(payload_json,'$.created_at'),json_extract(payload_json,'$.updated_at') FROM native_study_artifacts WHERE workspace_id=? ORDER BY json_extract(payload_json,'$.updated_at') DESC LIMIT 100")?
        .query_map([scope],|row|Ok(json!({"id":row.get::<_,String>(0)?,"kind":row.get::<_,String>(1)?,"title":row.get::<_,String>(2)?,"created_at":row.get::<_,f64>(3)?,"updated_at":row.get::<_,f64>(4)?,"content_workspace_id":scope})))?.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(
        json!({"sources":{"books":[],"mastery":[],"reading":[],"watching":[],"practice":[],"native":records,"native_reading":crate::reading::dashboard(conn,scope)?},"failed":[]}),
    )
}
fn source_url(kb: &str, file: &str, hash: &str, scope: &str, line: i64) -> String {
    let mut url = reqwest::Url::parse("http://localhost").unwrap();
    url.path_segments_mut()
        .unwrap()
        .extend(["api", "knowledge-bases", kb, "source"]);
    url.query_pairs_mut()
        .append_pair("path", file)
        .append_pair("hash", hash)
        .append_pair("dt_workspace", scope);
    format!("{}#L{line}", &url.as_str()["http://localhost".len()..])
}
fn evidence(conn: &Connection, scope: &str, m: &Mutation) -> Result<Value> {
    if !text(&m.input, "url").is_empty() {
        if !m.web {
            return Err(Error::invalid("Web search was not enabled for this turn."));
        }
        let url = field(&m.input, "url", 2048)?;
        let results=conn.prepare("SELECT result_json FROM native_tool_invocations WHERE turn_id=? AND name='web_search' AND status='completed'")?.query_map([&m.turn],|r|r.get::<_,String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
        for result in results {
            let result: Value = serde_json::from_str(&result)
                .map_err(|_| Error::conflict("Invalid search receipt."))?;
            for source in result["sources"].as_array().into_iter().flatten() {
                if source["url"] == url {
                    return Ok(
                        json!({"url":url,"quote":source["snippet"],"title":source["title"],"kind":"web_search_snippet"}),
                    );
                }
            }
        }
        return Err(Error::invalid(
            "Research URLs must come from a successful web_search in this turn.",
        ));
    }
    let kb = field(&m.input, "kb_name", 200)?;
    if !m.knowledge.contains(&kb) {
        return Err(Error::missing());
    }
    let file = field(&m.input, "path", 512)?;
    let hash = field(&m.input, "hash", 128)?;
    let first = m.input["start_line"]
        .as_i64()
        .filter(|n| *n > 0)
        .ok_or_else(|| Error::invalid("Invalid evidence range."))?;
    let last = m.input["end_line"]
        .as_i64()
        .filter(|n| *n >= first && *n - first < 100)
        .ok_or_else(|| Error::invalid("Evidence is limited to 100 lines."))?;
    let content = crate::knowledge::source(conn, scope, &kb, &file, &hash)?;
    let lines = content.split('\n').collect::<Vec<_>>();
    if last > lines.len() as i64 {
        return Err(Error::invalid("Evidence exceeds the source."));
    }
    let quote = lines[(first - 1) as usize..last as usize].join("\n");
    if quote.len() > 8000 {
        return Err(Error::invalid("Evidence exceeds 8 KiB."));
    }
    Ok(
        json!({"kind":"knowledge","title":file,"kb_name":kb,"path":file,"hash":hash,"start_line":first,"end_line":last,"quote":quote,"url":source_url(&kb,&file,&hash,scope,first)}),
    )
}
fn question_fingerprint(question: &Value) -> Result<String> {
    use sha2::{Digest, Sha256};
    let normalize = |value: &str| {
        value
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase()
    };
    let choices = question["choices"]
        .as_array()
        .ok_or_else(|| Error::conflict("Invalid saved question choices."))?;
    let answer = question["answer_index"]
        .as_u64()
        .and_then(|i| choices.get(i as usize))
        .and_then(Value::as_str)
        .ok_or_else(|| Error::conflict("Invalid saved correct answer."))?;
    let mut choices = choices
        .iter()
        .map(|v| {
            v.as_str()
                .map(normalize)
                .ok_or_else(|| Error::conflict("Invalid saved question choice."))
        })
        .collect::<Result<Vec<_>>>()?;
    choices.sort();
    Ok(format!("{:x}",Sha256::digest(json!({"prompt":normalize(text(question,"prompt")),"choices":choices,"answer":normalize(answer)}).to_string())))
}
fn require_objective_ready(c: &Connection, s: &str, plan: &str, objective: &str) -> Result<()> {
    let progress = progress(c, s)?;
    let row = progress["objectives"]
        .as_array()
        .and_then(|a| {
            a.iter()
                .find(|v| v["plan_id"] == plan && v["objective_id"] == objective)
        })
        .ok_or_else(Error::missing)?;
    if row["status"] == "locked" {
        return Err(Error::invalid(
            "Complete the prerequisite objectives before practicing this objective.",
        ));
    }
    Ok(())
}
fn questions(
    conn: &Connection,
    scope: &str,
    input: &Value,
    target: Option<&Target>,
) -> Result<Value> {
    let items = input["questions"]
        .as_array()
        .filter(|q| !q.is_empty() && q.len() <= 20)
        .ok_or_else(|| Error::invalid("Generate 1-20 questions."))?;
    let mut result = Vec::new();
    let mut unique = std::collections::HashSet::new();
    let eligibility = progress(conn, scope)?;
    for (i, q) in items.iter().enumerate() {
        let prompt = field(q, "prompt", 4000)?;
        let choices = strings(q, "choices", 6)?;
        if choices.len() < 2 {
            return Err(Error::invalid("Each question needs at least two choices."));
        }
        if choices
            .iter()
            .map(|s| s.trim().to_lowercase())
            .collect::<std::collections::HashSet<_>>()
            .len()
            != choices.len()
        {
            return Err(Error::invalid("Question choices must be distinct."));
        }
        let answer = q["answer_index"]
            .as_u64()
            .filter(|n| *n < choices.len() as u64)
            .ok_or_else(|| Error::invalid("Invalid correct answer index."))?;
        let explanation = field(q, "explanation", 4000)?;
        if let Some(target) = target
            && ((!text(q, "plan_id").is_empty() && q["plan_id"] != target.plan_id)
                || (!text(q, "objective_id").is_empty()
                    && q["objective_id"] != target.objective_id))
        {
            return Err(Error::invalid(
                "Generated questions must match the selected learning objective.",
            ));
        }
        let plan = target
            .map(|t| t.plan_id.as_str())
            .unwrap_or_else(|| text(q, "plan_id"));
        let objective = target
            .map(|t| t.objective_id.as_str())
            .unwrap_or_else(|| text(q, "objective_id"));
        if plan.is_empty() != objective.is_empty() {
            return Err(Error::invalid(
                "Specify both the learning path and objective identity.",
            ));
        }
        if !plan.is_empty() {
            let plan = raw(conn, scope, plan)?;
            if plan["kind"] != "mastery_path"
                || plan["phase"] != "published"
                || !plan["result"]["units"]
                    .as_array()
                    .is_some_and(|a| a.iter().any(|u| u["id"] == objective))
            {
                return Err(Error::invalid(
                    "Question refers to an unavailable learning objective.",
                ));
            }
        }
        if !plan.is_empty() {
            let ready = eligibility["objectives"]
                .as_array()
                .and_then(|a| {
                    a.iter()
                        .find(|r| r["plan_id"] == plan && r["objective_id"] == objective)
                })
                .ok_or_else(Error::missing)?;
            if ready["status"] == "locked" {
                return Err(Error::invalid(
                    "Complete prerequisite objectives before generating questions for this objective.",
                ));
            }
        }
        if !unique.insert(question_fingerprint(q)?) {
            return Err(Error::invalid(
                "Practice questions must have distinct prompts and choices.",
            ));
        }
        result.push(json!({"id":format!("Q{}",i+1),"prompt":prompt,"choices":choices,"answer_index":answer,"explanation":explanation,"plan_id":plan,"objective_id":objective}));
    }
    Ok(json!({"questions":result}))
}
fn path(input: &Value) -> Result<Value> {
    let units = input["units"]
        .as_array()
        .filter(|a| !a.is_empty() && a.len() <= 20)
        .ok_or_else(|| Error::invalid("A learning path needs 1-20 units."))?;
    let mut out = Vec::new();
    for (i, unit) in units.iter().enumerate() {
        let title = field(unit, "title", 240)?;
        let goals = strings(unit, "goals", 8)?;
        let dependencies = unit["prerequisites"]
            .as_array()
            .ok_or_else(|| Error::invalid("Prerequisites must be an array."))?;
        if dependencies
            .iter()
            .any(|n| n.as_u64().is_none_or(|n| n >= i as u64))
        {
            return Err(Error::invalid(
                "Prerequisites must refer to earlier units. Cycles are not permitted.",
            ));
        }
        out.push(json!({"id":format!("O{}",i+1),"title":title,"goals":goals,"prerequisites":dependencies}));
    }
    Ok(json!({"units":out}))
}
pub fn apply(conn: &Connection, scope: &str, mutation: Mutation) -> Result<Value> {
    let m = &mutation;
    let mut record = if m.operation == "study_plan" {
        if conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM native_study_artifacts WHERE workspace_id=? AND id=?)",
            params![scope, m.turn],
            |r| r.get::<_, bool>(0),
        )? {
            return Err(Error::conflict(
                "This turn already has a committed study plan.",
            ));
        }
        json!({"id":m.turn,"kind":m.kind,"title":field(&m.input,"title",240)?,"objectives":strings(&m.input,"objectives",12)?,"phase":"planned","evidence":[],"revision":0,"created_at":now()})
    } else {
        raw(conn, scope, &m.turn)?
    };
    if record["kind"] != m.kind {
        return Err(Error::conflict(
            "Study capability does not match its committed plan.",
        ));
    }
    if let Some(proposal) = &m.proposal {
        record["proposal_target"] = proposal.target.clone();
    }
    if record["phase"] == "published" {
        return Err(Error::conflict("This study artifact is already published."));
    }
    match m.operation.as_str() {
        "study_plan" => {}
        "study_evidence" if m.kind == "deep_research" => {
            let mut item = evidence(conn, scope, m)?;
            let items = record["evidence"]
                .as_array_mut()
                .ok_or_else(|| Error::conflict("Invalid evidence ledger."))?;
            if items.len() >= 12 {
                return Err(Error::invalid("Research evidence limit reached."));
            }
            item["id"] = json!(format!("E{}", items.len() + 1));
            items.push(item);
        }
        "study_publish" => {
            record["result"] = match m.kind.as_str() {
                "deep_question" => questions(conn, scope, &m.input, m.target.as_ref())?,
                "mastery_path" => path(&m.input)?,
                "note_ai" => {
                    let blocks = m.input.get("blocks").ok_or_else(|| {
                        Error::invalid("AI note proposals require editable blocks.")
                    })?;
                    if blocks.as_array().is_none_or(Vec::is_empty) {
                        return Err(Error::invalid(
                            "AI note proposals must contain at least one block.",
                        ));
                    }
                    let content = crate::documents::markdown(blocks)?;
                    json!({"blocks":blocks,"content":content})
                }
                "memory_consolidate" => json!({"content":field(&m.input,"content",32768)?}),
                "deep_research" => {
                    let report = field(&m.input, "report", 32768)?;
                    let citations = strings(&m.input, "citations", 12)?;
                    let evidence = record["evidence"]
                        .as_array()
                        .ok_or_else(|| Error::conflict("Missing evidence ledger."))?;
                    if citations
                        .iter()
                        .any(|id| !evidence.iter().any(|e| e["id"] == id.as_str()))
                    {
                        return Err(Error::invalid(
                            "The report contains a citation that was not verified.",
                        ));
                    }
                    json!({"report":report,"citations":citations})
                }
                _ => return Err(Error::invalid("Unsupported study capability.")),
            };
            record["phase"] = json!("published");
        }
        _ => {
            return Err(Error::invalid(
                "This study tool is unavailable in the current capability.",
            ));
        }
    }
    save(conn, scope, &mut record)?;
    Ok(json!({"success":true,"content":record,"study_artifact":record}))
}
pub fn attempt(conn: &Connection, scope: &str, id: &str, input: &Value) -> Result<Value> {
    let command = field(input, "command_id", 200)?;
    let question = field(input, "question_id", 32)?;
    let prior: Option<String> = conn
        .query_row(
            "SELECT payload_json FROM native_study_attempts WHERE workspace_id=? AND command_id=?",
            params![scope, command],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(prior) = prior {
        let prior: Value = serde_json::from_str(&prior)
            .map_err(|_| Error::conflict("Invalid attempt receipt."))?;
        if prior["artifact_id"] != id
            || prior["question_id"] != question
            || prior["answer_index"] != input["answer_index"]
        {
            return Err(Error::conflict(
                "Attempt identity was reused with a different answer.",
            ));
        }
        return Ok(prior);
    }
    let record = raw(conn, scope, id)?;
    if record["phase"] != "published" || record["revision"] != input["expected_revision"] {
        return Err(Error::conflict(
            "Question set changed. Reload before answering.",
        ));
    }
    let question = record["result"]["questions"]
        .as_array()
        .and_then(|a| a.iter().find(|q| q["id"] == question))
        .ok_or_else(Error::missing)?;
    let answer = input["answer_index"]
        .as_u64()
        .filter(|n| *n < question["choices"].as_array().map_or(0, Vec::len) as u64)
        .ok_or_else(|| Error::invalid("Choose a valid answer."))?;
    let correct = question["answer_index"] == answer;
    let result = json!({"artifact_id":id,"question_id":question["id"],"answer_index":answer,"correct":correct,"correct_answer_index":question["answer_index"],"explanation":question["explanation"],"created_at":now(),"plan_id":question["plan_id"],"objective_id":question["objective_id"],"question_fingerprint":question_fingerprint(question)?});
    conn.execute(
        "INSERT INTO native_study_attempts VALUES (?,?,?)",
        params![scope, command, result.to_string()],
    )?;
    Ok(result)
}
pub fn progress(conn: &Connection, scope: &str) -> Result<Value> {
    let rows = conn
        .prepare(
            "SELECT payload_json FROM native_study_attempts WHERE workspace_id=? ORDER BY rowid",
        )?
        .query_map([scope], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut rows = rows
        .into_iter()
        .map(|raw| {
            serde_json::from_str::<Value>(&raw)
                .map_err(|_| Error::conflict("Invalid learning progress."))
        })
        .collect::<Result<Vec<_>>>()?;
    let mut artifacts = std::collections::HashMap::<String, Value>::new();
    for row in &mut rows {
        if row["question_fingerprint"].is_string() {
            continue;
        }
        let id = text(row, "artifact_id");
        if !artifacts.contains_key(id) {
            artifacts.insert(id.into(), raw(conn, scope, id)?);
        }
        let question = artifacts[id]["result"]["questions"]
            .as_array()
            .and_then(|a| a.iter().find(|q| q["id"] == row["question_id"]))
            .ok_or_else(|| Error::conflict("Practice evidence refers to a missing question."))?;
        row["question_fingerprint"] = json!(question_fingerprint(question)?);
    }
    let plans=conn.prepare("SELECT payload_json FROM native_study_artifacts WHERE workspace_id=? AND json_extract(payload_json,'$.kind')='mastery_path' AND json_extract(payload_json,'$.phase')='published' ORDER BY json_extract(payload_json,'$.created_at'),id")?.query_map([scope],|r|r.get::<_,String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?.into_iter().map(|v|serde_json::from_str::<Value>(&v).map_err(|_|Error::conflict("Invalid saved learning path."))).collect::<Result<Vec<_>>>()?;
    let correct = rows.iter().filter(|r| r["correct"] == true).count();
    let objectives = crate::mastery::summarize(&plans, &rows, now())?;
    Ok(json!({"attempts":rows,"correct":correct,"total":rows.len(),"objectives":objectives}))
}
pub fn reviews(conn: &Connection, scope: &str) -> Result<Value> {
    let progress = progress(conn, scope)?;
    let at = now();
    let mut due = Vec::new();
    let mut upcoming = Vec::new();
    for row in progress["objectives"].as_array().unwrap() {
        if row["status"] == "locked" {
            continue;
        }
        if let Some(time) = row["review_due_at"].as_f64() {
            if time <= at {
                due.push(row.clone());
            } else {
                upcoming.push(row.clone());
            }
        }
    }
    let sort = |a: &Value, b: &Value| {
        a["review_due_at"]
            .as_f64()
            .partial_cmp(&b["review_due_at"].as_f64())
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| text(a, "plan_id").cmp(text(b, "plan_id")))
            .then_with(|| text(a, "objective_id").cmp(text(b, "objective_id")))
    };
    due.sort_by(sort);
    upcoming.sort_by(sort);
    Ok(json!({"due":due,"upcoming":upcoming,"generated_at":at}))
}

pub fn note(conn: &Connection, scope: &str, id: &str, input: &Value) -> Result<Value> {
    let mut record = raw(conn, scope, id)?;
    if let Some(note) = record["note_id"].as_str()
        && let Ok(page) = crate::documents::read(conn, scope, note)
    {
        return Ok(page);
    }
    if record["phase"] != "published" || record["revision"] != input["expected_revision"] {
        return Err(Error::conflict(
            "Study artifact changed. Reload before saving.",
        ));
    }
    let mut content = format!("# {}\n\n", text(&record, "title"));
    if record["kind"] == "deep_research" {
        content.push_str(text(&record["result"], "report"));
        for evidence in record["evidence"].as_array().into_iter().flatten() {
            content.push_str(&format!(
                "\n\n{}: {}\n{}\n",
                text(evidence, "id"),
                text(evidence, "url"),
                text(evidence, "quote")
            ));
        }
    } else if let Some(proposal) = record["result"]["content"].as_str() {
        content.push_str(proposal);
    } else if let Some(questions) = record["result"]["questions"].as_array() {
        for question in questions {
            content.push_str(&format!(
                "## {}\n\n{}\n\n",
                text(question, "id"),
                text(question, "prompt")
            ));
            for choice in question["choices"].as_array().into_iter().flatten() {
                content.push_str(&format!("- {}\n", choice.as_str().unwrap_or("")));
            }
            content.push('\n');
        }
    } else if let Some(units) = record["result"]["units"].as_array() {
        for unit in units {
            content.push_str(&format!("## {}\n\n", text(unit, "title")));
            for goal in unit["goals"].as_array().into_iter().flatten() {
                content.push_str(&format!("- {}\n", goal.as_str().unwrap_or("")));
            }
            content.push('\n');
        }
    }
    let page = crate::documents::create(
        conn,
        scope,
        &json!({"title":record["title"],"content":content}),
    )?;
    record["note_id"] = page["id"].clone();
    save(conn, scope, &mut record)?;
    Ok(page)
}
pub fn apply_proposal(conn: &Connection, scope: &str, id: &str, input: &Value) -> Result<Value> {
    let mut record = raw(conn, scope, id)?;
    let target = record["proposal_target"].clone();
    let existing = if record["kind"] == "note_ai" {
        crate::documents::read(conn, scope, text(&target, "document_id"))?
    } else if record["kind"] == "memory_consolidate" {
        crate::learning::read_memory(conn, scope, text(&target, "layer"), text(&target, "key"))?
    } else {
        return Err(Error::invalid(
            "This study artifact is not an edit proposal.",
        ));
    };
    if record["applied"] == true {
        return Ok(existing);
    }
    if record["phase"] != "published"
        || record["revision"] != input["expected_revision"]
        || target["revision"] != input["expected_target_revision"]
    {
        return Err(Error::conflict(
            "Edit proposal changed. Reload before applying.",
        ));
    }
    let result = if record["kind"] == "note_ai" {
        crate::documents::update(
            conn,
            scope,
            text(&target, "document_id"),
            &json!({"blocks":record["result"]["blocks"],"expected_revision":target["revision"]}),
        )?
    } else {
        crate::learning::write_memory(
            conn,
            scope,
            text(&target, "layer"),
            text(&target, "key"),
            &json!({"content":record["result"]["content"],"expected_revision":target["revision"]}),
        )?
    };
    record["applied"] = json!(true);
    record["applied_revision"] = result["revision"].clone();
    save(conn, scope, &mut record)?;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn db() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(include_str!("schema.sql")).unwrap();
        c
    }
    fn mutation(id: &str, kind: &str, op: &str, input: Value) -> Mutation {
        Mutation {
            turn: id.into(),
            kind: kind.into(),
            operation: op.into(),
            input,
            knowledge: vec!["source".into()],
            web: false,
            target: None,
            proposal: None,
        }
    }
    fn plan(c: &Connection, id: &str, kind: &str) {
        apply(
            c,
            "w",
            mutation(
                id,
                kind,
                "study_plan",
                json!({"title":"学习任务","objectives":["理解与应用"]}),
            ),
        )
        .unwrap();
    }
    #[test]
    fn research_requires_real_scoped_sources_and_a_verified_citation_ledger() {
        let c = db();
        plan(&c, "r", "deep_research");
        assert!(
            apply(
                &c,
                "w",
                mutation(
                    "r",
                    "deep_research",
                    "study_publish",
                    json!({"report":"fabricated","citations":["E1"]})
                )
            )
            .is_err()
        );
        crate::knowledge::ingest(
            &c,
            "w",
            "source",
            &[crate::knowledge::Document {
                path: "paper.md".into(),
                content: "真实资料\n实验结果".into(),
                original: b"original".to_vec(),
            }],
            true,
        )
        .unwrap();
        let hash = crate::knowledge::files(&c, "w", "source").unwrap()[0]["sha256"]
            .as_str()
            .unwrap()
            .to_owned();
        let evidence =
            json!({"kb_name":"source","path":"paper.md","hash":hash,"start_line":1,"end_line":2});
        assert!(
            apply(
                &c,
                "other",
                mutation("r", "deep_research", "study_evidence", evidence.clone())
            )
            .is_err()
        );
        apply(
            &c,
            "w",
            mutation("r", "deep_research", "study_evidence", evidence),
        )
        .unwrap();
        apply(
            &c,
            "w",
            mutation(
                "r",
                "deep_research",
                "study_publish",
                json!({"report":"结论 [E1]","citations":["E1"]}),
            ),
        )
        .unwrap();
        let value = read(&c, "w", "r").unwrap();
        assert_eq!(value["evidence"][0]["quote"], "真实资料\n实验结果");
        assert_eq!(value["phase"], "published");
        let page = note(
            &c,
            "w",
            "r",
            &json!({"expected_revision":value["revision"]}),
        )
        .unwrap();
        assert!(text(&page, "content").contains("真实资料"));
        assert_eq!(
            note(&c, "w", "r", &json!({"expected_revision":0})).unwrap()["id"],
            page["id"]
        );
    }
    #[test]
    fn practice_hides_solutions_until_answering_and_deduplicates_attempts() {
        let c = db();
        plan(&c, "q", "deep_question");
        apply(&c,"w",mutation("q","deep_question","study_publish",json!({"questions":[{"prompt":"1+1=?","choices":["2","3"],"answer_index":0,"explanation":"两个一合并为二"}]}))).unwrap();
        let public = read(&c, "w", "q").unwrap();
        assert!(
            public["result"]["questions"][0]
                .get("answer_index")
                .is_none()
        );
        assert!(
            public["result"]["questions"][0]
                .get("explanation")
                .is_none()
        );
        let input = json!({"command_id":"attempt-1","question_id":"Q1","answer_index":0,"expected_revision":public["revision"]});
        let result = attempt(&c, "w", "q", &input).unwrap();
        assert_eq!(result["correct"], true);
        assert_eq!(attempt(&c, "w", "q", &input).unwrap(), result);
        let mut changed = input.clone();
        changed["answer_index"] = json!(1);
        assert!(attempt(&c, "w", "q", &changed).is_err());
        changed["command_id"] = json!("attempt-2");
        changed["expected_revision"] = json!(0);
        assert!(attempt(&c, "w", "q", &changed).is_err());
        assert!(attempt(&c, "other", "q", &input).is_err());
        assert_eq!(progress(&c, "w").unwrap()["total"], 1);
        let index = dashboard(&c, "w").unwrap();
        assert_eq!(index["sources"]["native"][0]["id"], "q");
        assert!(!index.to_string().contains("answer_index"));
        assert_eq!(
            dashboard(&c, "other").unwrap()["sources"]["native"],
            json!([])
        );
    }
    #[test]
    fn learning_paths_reject_cycles_and_progress_counts_distinct_question_evidence() {
        let c = db();
        plan(&c, "p", "mastery_path");
        assert!(
            apply(
                &c,
                "w",
                mutation(
                    "p",
                    "mastery_path",
                    "study_publish",
                    json!({"units":[{"title":"循环","goals":["goal"],"prerequisites":[0]}]})
                )
            )
            .is_err()
        );
        apply(
            &c,
            "w",
            mutation(
                "p",
                "mastery_path",
                "study_publish",
                json!({"units":[{"title":"加法","goals":["计算"],"prerequisites":[]}]}),
            ),
        )
        .unwrap();
        plan(&c, "q", "deep_question");
        apply(&c,"w",mutation("q","deep_question","study_publish",json!({"questions":[{"prompt":"1+1=?","choices":["2","3"],"answer_index":0,"explanation":"加法","plan_id":"p","objective_id":"O1"}]}))).unwrap();
        for id in ["a", "b"] {
            attempt(
                &c,
                "w",
                "q",
                &json!({"command_id":id,"question_id":"Q1","answer_index":0,"expected_revision":2}),
            )
            .unwrap();
        }
        assert_eq!(
            progress(&c, "w").unwrap()["objectives"][0]["correct_questions"],
            1
        );
    }
    #[test]
    fn study_artifact_and_tool_receipt_commit_once_in_the_same_transaction() {
        let dir = tempfile::tempdir().unwrap();
        let store = crate::store::Store::open(&dir.path().join("test.db")).unwrap();
        let admitted=store.admit(&json!({"content":"练习","workspace_id":"w","capability":"deep_question","client_submission_id":"study-test"}),"owner").unwrap();
        let call = crate::tools::ToolCall {
            id: "plan-call".into(),
            name: "study_plan".into(),
            arguments: "{}".into(),
        };
        store.start_tool(&admitted.turn, "invoke", &call).unwrap();
        let outcome = || crate::tools::ToolOutcome {
            result: Value::Null,
            mutation: Some(crate::learning::Mutation::Study(mutation(
                &admitted.turn,
                "deep_question",
                "study_plan",
                json!({"title":"计划","objectives":["理解"]}),
            ))),
        };
        let first = store
            .commit_tool(&admitted.turn, "invoke", outcome())
            .unwrap();
        assert_eq!(first["success"], true);
        assert_eq!(
            store
                .commit_tool(&admitted.turn, "invoke", outcome())
                .unwrap(),
            first
        );
        assert_eq!(
            raw(&store.connect().unwrap(), "w", &admitted.turn).unwrap()["revision"],
            1
        );
        let receipts: i64 = store
            .connect()
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM turn_events WHERE turn_id=? AND type='tool_result'",
                [&admitted.turn],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(receipts, 1);
    }
    #[test]
    fn reviewed_note_proposals_preserve_source_until_apply_and_reject_stale_revisions() {
        use crate::{capability::Capability, documents};
        let c = db();
        let page = documents::create(&c, "w", &json!({"title":"Notes","content":"original fact"}))
            .unwrap();
        let id = text(&page, "id");
        let target = json!({"document_id":id,"expected_revision":page["revision"]});
        assert!(proposal(&c, "other", &target, &Value::Null, Capability::NoteAi).is_err());
        let mut legacy = page.clone();
        legacy["blocks"] = json!([{"id":"legacy-media","type":"image","props":{"url":"/old-image.png"},"content":[],"children":[]}]);
        c.execute(
            "UPDATE native_documents SET payload_json=? WHERE workspace_id='w' AND id=?",
            params![legacy.to_string(), id],
        )
        .unwrap();
        assert!(proposal(&c, "w", &target, &Value::Null, Capability::NoteAi).is_err());
        c.execute(
            "UPDATE native_documents SET payload_json=? WHERE workspace_id='w' AND id=?",
            params![page.to_string(), id],
        )
        .unwrap();
        let context = proposal(&c, "w", &target, &Value::Null, Capability::NoteAi)
            .unwrap()
            .unwrap();
        assert!(context.source.contains("original fact"));
        let blocks = json!([{"id":"proposal-block","type":"paragraph","props":{},"content":[{"type":"text","text":"clear original fact","styles":{}}],"children":[]}]);
        for (op, input) in [
            (
                "study_plan",
                json!({"title":"Edit note","objectives":["Clarify"]}),
            ),
            ("study_publish", json!({"blocks":blocks})),
        ] {
            let mut m = mutation("edit", "note_ai", op, input);
            m.proposal = Some(context.clone());
            apply(&c, "w", m).unwrap();
        }
        assert_eq!(documents::read(&c, "w", id).unwrap(), page);
        let record = read(&c, "w", "edit").unwrap();
        let input = json!({"expected_revision":record["revision"],"expected_target_revision":page["revision"]});
        assert!(apply_proposal(&c, "other", "edit", &input).is_err());
        assert!(
            apply_proposal(
                &c,
                "w",
                "edit",
                &json!({"expected_revision":0,"expected_target_revision":page["revision"]})
            )
            .is_err()
        );
        let changed = documents::update(
            &c,
            "w",
            id,
            &json!({"content":"concurrent human edit","expected_revision":page["revision"]}),
        )
        .unwrap();
        assert!(apply_proposal(&c, "w", "edit", &input).is_err());
        assert_eq!(documents::read(&c, "w", id).unwrap(), changed);
        assert_ne!(read(&c, "w", "edit").unwrap()["applied"], true);
        // A second proposal uses the new source revision, and repeat apply never overwrites later edits.
        let context = proposal(
            &c,
            "w",
            &json!({"document_id":id,"expected_revision":changed["revision"]}),
            &Value::Null,
            Capability::NoteAi,
        )
        .unwrap()
        .unwrap();
        for (op, input) in [
            (
                "study_plan",
                json!({"title":"Second edit","objectives":["Clarify"]}),
            ),
            ("study_publish", json!({"blocks":blocks})),
        ] {
            let mut m = mutation("edit-2", "note_ai", op, input);
            m.proposal = Some(context.clone());
            apply(&c, "w", m).unwrap();
        }
        let applied = apply_proposal(
            &c,
            "w",
            "edit-2",
            &json!({"expected_revision":2,"expected_target_revision":changed["revision"]}),
        )
        .unwrap();
        assert!(text(&applied, "content").contains("clear original fact"));
        let later=documents::update(&c,"w",id,&json!({"blocks":[{"id":"later-block","type":"paragraph","props":{},"content":[{"type":"text","text":"later human edit","styles":{}}],"children":[]}],"expected_revision":applied["revision"]})).unwrap();
        assert_eq!(
            apply_proposal(
                &c,
                "w",
                "edit-2",
                &json!({"expected_revision":2,"expected_target_revision":changed["revision"]})
            )
            .unwrap(),
            later
        );
        assert_eq!(documents::read(&c, "w", id).unwrap(), later);
    }
    #[test]
    fn memory_consolidation_is_reviewed_scoped_and_versioned() {
        use crate::{capability::Capability, learning};
        let c = db();
        let old = learning::read_memory(&c, "w", "L3", "preferences").unwrap();
        let page=learning::write_memory(&c,"w","L3","preferences",&json!({"content":"prefers examples\nprefers examples","expected_revision":old["revision"]})).unwrap();
        let target = json!({"layer":"L3","key":"preferences","expected_revision":page["revision"]});
        let context = proposal(
            &c,
            "w",
            &Value::Null,
            &target,
            Capability::MemoryConsolidate,
        )
        .unwrap()
        .unwrap();
        for (op, input) in [
            (
                "study_plan",
                json!({"title":"Consolidate","objectives":["Deduplicate"]}),
            ),
            ("study_publish", json!({"content":"prefers examples"})),
        ] {
            let mut m = mutation("memory", "memory_consolidate", op, input);
            m.proposal = Some(context.clone());
            apply(&c, "w", m).unwrap();
        }
        assert_eq!(
            learning::read_memory(&c, "w", "L3", "preferences").unwrap(),
            page
        );
        let updated = apply_proposal(
            &c,
            "w",
            "memory",
            &json!({"expected_revision":2,"expected_target_revision":page["revision"]}),
        )
        .unwrap();
        assert_eq!(updated["content"], "prefers examples");
        assert_ne!(
            learning::read_memory(&c, "other", "L3", "preferences").unwrap()["content"],
            updated["content"]
        );
        assert_eq!(c.query_row("SELECT content FROM native_memory_versions WHERE workspace_id='w' AND layer='L3' AND key='preferences' ORDER BY id DESC LIMIT 1",[],|r|r.get::<_,String>(0)).unwrap(),"prefers examples\nprefers examples");
    }
}

#[cfg(test)]
mod mastery_workflow_tests {
    use super::*;
    fn db() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(include_str!("schema.sql")).unwrap();
        c
    }
    fn mutation(id: &str, kind: &str, operation: &str, input: Value) -> Mutation {
        Mutation {
            turn: id.into(),
            kind: kind.into(),
            operation: operation.into(),
            input,
            knowledge: vec![],
            web: false,
            target: None,
            proposal: None,
        }
    }
    fn plan(c: &Connection, id: &str, kind: &str) {
        apply(
            c,
            "w",
            mutation(
                id,
                kind,
                "study_plan",
                json!({"title":id,"objectives":["Learn"]}),
            ),
        )
        .unwrap();
    }
    fn path(c: &Connection) {
        plan(c, "path", "mastery_path");
        apply(c,"w",mutation("path","mastery_path","study_publish",json!({"units":[{"title":"Foundation","goals":["Basics"],"prerequisites":[]},{"title":"Application","goals":["Apply"],"prerequisites":[0]}]}))).unwrap();
    }
    fn questions(c: &Connection, id: &str, prompts: &[&str]) {
        plan(c, id, "deep_question");
        let questions=prompts.iter().map(|prompt|json!({"prompt":prompt,"choices":["Correct","Wrong"],"answer_index":0,"explanation":"Reason","plan_id":"path","objective_id":"O1"})).collect::<Vec<_>>();
        apply(
            c,
            "w",
            mutation(
                id,
                "deep_question",
                "study_publish",
                json!({"questions":questions}),
            ),
        )
        .unwrap();
    }
    fn answer(c: &Connection, id: &str, q: &str, choice: u64) {
        attempt(c,"w",id,&json!({"command_id":uuid::Uuid::new_v4().to_string(),"question_id":q,"answer_index":choice,"expected_revision":2})).unwrap();
    }
    #[test]
    fn first_exposure_evidence_unlocks_prerequisites_and_exact_content_duplicates_do_not_inflate_it()
     {
        let c = db();
        path(&c);
        assert!(
            target(
                &c,
                "w",
                &json!({"plan_id":"path","objective_id":"O2"}),
                crate::capability::Capability::DeepQuestion
            )
            .is_err()
        );
        questions(&c, "q", &["First?", "Second?", "Third?"]);
        answer(&c, "q", "Q1", 1);
        answer(&c, "q", "Q1", 0);
        answer(&c, "q", "Q2", 0);
        answer(&c, "q", "Q3", 0);
        let before = progress(&c, "w").unwrap();
        assert_eq!(before["objectives"][0]["correct_questions"], 2);
        assert_eq!(before["objectives"][1]["status"], "locked");
        questions(&c, "duplicate", &["  SECOND?  "]);
        answer(&c, "duplicate", "Q1", 0);
        assert_eq!(
            progress(&c, "w").unwrap()["objectives"][0]["correct_questions"],
            2
        );
        questions(&c, "fresh", &["Fourth?"]);
        answer(&c, "fresh", "Q1", 0);
        let next = progress(&c, "w").unwrap();
        assert_eq!(next["objectives"][0]["status"], "mastered");
        assert_eq!(next["objectives"][1]["status"], "ready");
        assert!(
            target(
                &c,
                "w",
                &json!({"plan_id":"path","objective_id":"O2"}),
                crate::capability::Capability::DeepQuestion
            )
            .unwrap()
            .is_some()
        );
        assert_eq!(next["objectives"][0]["round_correct"], 0);
        let before_due = next["objectives"][0]["review_due_at"].clone();
        answer(&c, "fresh", "Q1", 0);
        assert_eq!(
            progress(&c, "w").unwrap()["objectives"][0]["review_due_at"],
            before_due
        );
    }
    #[test]
    fn review_queue_restores_from_evidence_and_stays_workspace_scoped() {
        let c = db();
        path(&c);
        questions(&c, "q", &["One?", "Two?", "Three?"]);
        for q in ["Q1", "Q2", "Q3"] {
            answer(&c, "q", q, 0);
        }
        c.execute("UPDATE native_study_attempts SET payload_json=json_set(payload_json,'$.created_at',? + rowid * 0.01)",[now()-2.*86400.]).unwrap();
        let queue = reviews(&c, "w").unwrap();
        assert_eq!(queue["due"].as_array().unwrap().len(), 1);
        assert_eq!(queue["due"][0]["objective_id"], "O1");
        assert_eq!(queue["due"][0]["title"], "Foundation");
        assert_eq!(reviews(&c, "other").unwrap()["due"], json!([]));
        let layers = crate::calendar::layers(
            &c,
            "w",
            &(chrono::Utc::now() - chrono::Duration::days(2)).to_rfc3339(),
            &(chrono::Utc::now() + chrono::Duration::days(2)).to_rfc3339(),
            "UTC",
        )
        .unwrap();
        assert_eq!(layers["reviews"][0]["paths"][0]["title"], "Foundation");
    }
    #[test]
    fn reordered_duplicate_choices_and_locked_objective_references_are_rejected() {
        let c = db();
        path(&c);
        plan(&c, "dup", "deep_question");
        let before = read(&c, "w", "dup").unwrap();
        let q = json!({"questions":[{"prompt":"Same?","choices":["A","B"],"answer_index":0,"explanation":"A"},{"prompt":"same?","choices":["B","A"],"answer_index":1,"explanation":"A"}]});
        assert!(
            apply(
                &c,
                "w",
                mutation("dup", "deep_question", "study_publish", q)
            )
            .is_err()
        );
        assert_eq!(read(&c, "w", "dup").unwrap(), before);
        let q = json!({"questions":[{"prompt":"Locked?","choices":["A","B"],"answer_index":0,"explanation":"A","plan_id":"path","objective_id":"O2"}]});
        assert!(
            apply(
                &c,
                "w",
                mutation("dup", "deep_question", "study_publish", q)
            )
            .is_err()
        );
        assert_eq!(read(&c, "w", "dup").unwrap(), before);
    }
    #[test]
    fn clock_rollback_cannot_turn_a_revealed_correction_into_first_exposure_evidence() {
        let c = db();
        path(&c);
        questions(&c, "q", &["One?"]);
        answer(&c, "q", "Q1", 1);
        c.execute(
            "UPDATE native_study_attempts SET payload_json=json_set(payload_json,'$.created_at',?)",
            [now() + 3600.],
        )
        .unwrap();
        answer(&c, "q", "Q1", 0);
        let value = progress(&c, "w").unwrap();
        assert_eq!(value["objectives"][0]["correct_questions"], 0);
        assert_eq!(value["objectives"][0]["practice_correct_questions"], 1);
        assert_eq!(value["objectives"][0]["round_correct"], 0);
    }
}
