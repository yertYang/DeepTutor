//! Authored flashcards with durable spaced-review scheduling and retry receipts.
use crate::{
    Error, Result, documents,
    store::{now, text},
};
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
fn fields(v: &Value, allowed: &[&str]) -> Result<()> {
    if v.as_object()
        .is_none_or(|o| o.keys().any(|k| !allowed.contains(&k.as_str())))
    {
        return Err(Error::invalid("Unexpected flashcard field."));
    }
    Ok(())
}
fn label(v: &Value, k: &str, max: usize) -> Result<String> {
    crate::learning::string(v, k, max, true)
}
pub fn deck(c: &Connection, s: &str, page: &str) -> Result<Value> {
    let note = documents::read(c, s, page)?;
    let raw: Option<String> = c
        .query_row(
            "SELECT payload_json FROM native_flashcard_decks WHERE workspace_id=? AND page_id=?",
            params![s, page],
            |r| r.get(0),
        )
        .optional()?;
    raw.map(|v| serde_json::from_str(&v).map_err(|_| Error::conflict("Invalid stored flashcards.")))
        .unwrap_or_else(|| legacy_deck(page, &note))
}
fn legacy_deck(page: &str, note: &Value) -> Result<Value> {
    fn visit(blocks: &Value, depth: usize, cards: &mut Vec<Value>) -> Result<()> {
        if depth > 16 {
            return Err(Error::invalid("Flashcard nesting exceeds 16 levels."));
        }
        for block in blocks.as_array().into_iter().flatten() {
            if block["type"] == "learning" && block["props"]["kind"] == "flashcards" {
                let raw = text(&block["props"], "data");
                if !raw.is_empty() {
                    let data: Value = serde_json::from_str(raw).map_err(|_| {
                        Error::conflict(
                            "Legacy flashcard data is invalid; the source note is preserved.",
                        )
                    })?;
                    for (index, card) in data["cards"].as_array().into_iter().flatten().enumerate()
                    {
                        let front = label(card, "front", 8000)?;
                        let back = label(card, "back", 16000)?;
                        let id = format!(
                            "legacy_{:x}",
                            Sha256::digest(json!([block["id"], index, front, back]).to_string())
                        );
                        cards.push(json!({"id":id,"front":front,"back":back,"due_at":0,"interval_days":0,"ease":2.5,"reviews":0,"lapses":0}));
                        if cards.len() > 500 {
                            return Err(Error::invalid(
                                "Legacy page has more than 500 flashcards. Split it before reviewing.",
                            ));
                        }
                    }
                }
            }
            visit(&block["children"], depth + 1, cards)?;
        }
        Ok(())
    }
    let mut cards = Vec::new();
    visit(&note["blocks"], 0, &mut cards)?;
    Ok(json!({"page_id":page,"revision":0,"cards":cards}))
}
fn save(c: &Connection, s: &str, page: &str, v: &mut Value) -> Result<()> {
    v["revision"] = json!(v["revision"].as_u64().unwrap() + 1);
    c.execute("INSERT INTO native_flashcard_decks VALUES(?,?,?) ON CONFLICT(workspace_id,page_id) DO UPDATE SET payload_json=excluded.payload_json",params![s,page,v.to_string()])?;
    Ok(())
}
pub fn mutate(c: &Connection, s: &str, page: &str, review: bool, input: &Value) -> Result<Value> {
    fields(
        input,
        if review {
            &["command_id", "expected_revision", "card_id", "remembered"]
        } else {
            &["command_id", "expected_revision", "cards"]
        },
    )?;
    let command = label(input, "command_id", 128)?;
    let digest = format!(
        "{:x}",
        Sha256::digest(json!({"page":page,"review":review,"input":input}).to_string())
    );
    let prior:Option<(String,String)>=c.query_row("SELECT digest,payload_json FROM native_flashcard_receipts WHERE workspace_id=? AND command_id=?",params![s,command],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
    if let Some((old, _raw)) = prior {
        if digest != old {
            return Err(Error::conflict(
                "Flashcard command was reused with different content.",
            ));
        }
        return deck(c, s, page);
    }
    let mut v = deck(c, s, page)?;
    if input["expected_revision"].as_u64() != v["revision"].as_u64() {
        return Err(Error::conflict(
            "Flashcards changed elsewhere. Reload before saving.",
        ));
    }
    if review {
        let id = label(input, "card_id", 128)?;
        let remembered = input["remembered"]
            .as_bool()
            .ok_or_else(|| Error::invalid("Specify whether the card was remembered."))?;
        let card = v["cards"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|c| c["id"] == id)
            .ok_or_else(Error::missing)?;
        if card["due_at"].as_f64().unwrap_or(0.) > now() + 1. {
            return Err(Error::conflict("This card is not due for review yet."));
        }
        let previous = card["interval_days"].as_f64().unwrap_or(0.);
        let ease = card["ease"].as_f64().unwrap_or(2.5);
        let days = if !remembered {
            0.
        } else if previous < 1. {
            1.
        } else if previous < 3. {
            3.
        } else {
            (previous * ease).round().min(365.)
        };
        card["interval_days"] = json!(days);
        card["ease"] = json!(if remembered {
            (ease + 0.05).min(3.)
        } else {
            (ease - 0.2).max(1.3)
        });
        card["due_at"] = json!(now() + if remembered { days * 86400. } else { 600. });
        card["last_reviewed_at"] = json!(now());
        card["reviews"] = json!(card["reviews"].as_u64().unwrap_or(0) + 1);
        if !remembered {
            card["lapses"] = json!(card["lapses"].as_u64().unwrap_or(0) + 1);
        }
    } else {
        let cards = input["cards"]
            .as_array()
            .filter(|a| a.len() <= 500)
            .ok_or_else(|| Error::invalid("A page supports at most 500 flashcards."))?;
        let mut ids = std::collections::HashSet::new();
        let mut out = Vec::new();
        for card in cards {
            fields(card, &["id", "front", "back"])?;
            let id = label(card, "id", 128)?;
            if !ids.insert(id.clone()) {
                return Err(Error::invalid("Flashcard IDs must be unique."));
            }
            let front = label(card, "front", 8000)?;
            let back = label(card, "back", 16000)?;
            let old = v["cards"]
                .as_array()
                .unwrap()
                .iter()
                .find(|c| c["id"] == id && c["front"] == front && c["back"] == back);
            out.push(old.cloned().unwrap_or_else(||json!({"id":id,"front":front,"back":back,"due_at":now(),"interval_days":0,"ease":2.5,"reviews":0,"lapses":0})));
        }
        v["cards"] = json!(out);
    }
    save(c, s, page, &mut v)?;
    c.execute(
        "INSERT INTO native_flashcard_receipts VALUES(?,?,?,?)",
        params![s, command, digest, v.to_string()],
    )?;
    Ok(v)
}
pub fn due(c: &Connection, s: &str) -> Result<Value> {
    let mut statement=c.prepare("SELECT n.id,COALESCE(d.payload_json,''),n.payload_json FROM native_documents n LEFT JOIN native_flashcard_decks d ON n.workspace_id=d.workspace_id AND n.id=d.page_id WHERE n.workspace_id=? AND (d.page_id IS NOT NULL OR instr(n.payload_json,'flashcards')>0) ORDER BY n.id")?;
    let rows = statement.query_map([s], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
        ))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (page, raw, note) = row?;
        let note: Value =
            serde_json::from_str(&note).map_err(|_| Error::conflict("Invalid source page."))?;
        let deck: Value = if raw.is_empty() {
            legacy_deck(&page, &note)?
        } else {
            serde_json::from_str(&raw).map_err(|_| Error::conflict("Invalid stored flashcards."))?
        };
        for card in deck["cards"]
            .as_array()
            .ok_or_else(|| Error::conflict("Invalid flashcard deck."))?
        {
            if card["due_at"].as_f64().unwrap_or(0.) <= now() {
                out.push(json!({"page_id":page,"page_title":note["title"],"block_id":"","card_id":card["id"],"front":card["front"],"back":card["back"],"due_at":card["due_at"],"revision":deck["revision"]}));
            }
        }
    }
    out.sort_by(|a, b| {
        a["due_at"]
            .as_f64()
            .partial_cmp(&b["due_at"].as_f64())
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    out.truncate(1000);
    Ok(json!({"cards":out}))
}
pub fn export(c: &Connection, s: &str, page: Option<&str>) -> Result<String> {
    let pages = if let Some(page) = page {
        documents::read(c, s, page)?;
        vec![page.to_owned()]
    } else {
        c.prepare(
            "SELECT n.id FROM native_documents n LEFT JOIN native_flashcard_decks d ON n.workspace_id=d.workspace_id AND n.id=d.page_id WHERE n.workspace_id=? AND (d.page_id IS NOT NULL OR instr(n.payload_json,'flashcards')>0) ORDER BY n.id",
        )?
        .query_map([s], |r| r.get(0))?
        .collect::<rusqlite::Result<Vec<String>>>()?
    };
    fn escape(s: &str) -> String {
        s.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace(['\r', '\n'], "<br>")
            .replace('\t', " ")
    }
    let mut out = String::from("#separator:Tab\n#html:true\n#columns:Front\tBack\n");
    for page in pages {
        let d = deck(c, s, &page)?;
        for card in d["cards"].as_array().unwrap() {
            out.push_str(&format!(
                "{}\t{}\n",
                escape(text(card, "front")),
                escape(text(card, "back"))
            ));
            if out.len() > 16 * 1024 * 1024 {
                return Err(Error::invalid(
                    "Flashcard export exceeds 16 MiB. Export individual pages instead.",
                ));
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn db() -> (Connection, String) {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
        c.execute_batch(include_str!("schema.sql")).unwrap();
        let page =
            documents::create(&c, "a", &json!({"title":"Notes","content":"Study text"})).unwrap();
        let id = text(&page, "id").into();
        (c, id)
    }
    fn seed(c: &Connection, p: &str) -> Value {
        mutate(c,"a",p,false,&json!({"command_id":"create","expected_revision":0,"cards":[{"id":"one","front":"2 + 2?","back":"4"},{"id":"two","front":"<script>\nQuestion\t","back":"& answer"}]})).unwrap()
    }
    #[test]
    fn spaced_reviews_are_persistent_scoped_and_idempotent() {
        let (c, p) = db();
        seed(&c, &p);
        assert_eq!(due(&c, "a").unwrap()["cards"].as_array().unwrap().len(), 2);
        assert!(deck(&c, "b", &p).is_err());
        assert_eq!(due(&c, "b").unwrap()["cards"], json!([]));
        let body =
            json!({"command_id":"grade","expected_revision":1,"card_id":"one","remembered":true});
        let first = mutate(&c, "a", &p, true, &body).unwrap();
        assert_eq!(first["cards"][0]["interval_days"], 1.);
        assert_eq!(first["cards"][0]["reviews"], 1);
        assert_eq!(mutate(&c, "a", &p, true, &body).unwrap(), first);
        assert_eq!(due(&c, "a").unwrap()["cards"].as_array().unwrap().len(), 1);
        assert!(
            mutate(
                &c,
                "a",
                &p,
                true,
                &json!({"command_id":"new","expected_revision":2,"card_id":"one","remembered":true})
            )
            .is_err()
        );
        assert!(mutate(&c,"a",&p,true,&json!({"command_id":"grade","expected_revision":1,"card_id":"one","remembered":false})).is_err());
        let again = mutate(
            &c,
            "a",
            &p,
            true,
            &json!({"command_id":"again","expected_revision":2,"card_id":"two","remembered":false}),
        )
        .unwrap();
        assert_eq!(again["cards"][1]["lapses"], 1);
        assert!(again["cards"][1]["due_at"].as_f64().unwrap() > now() + 590.);
        assert_eq!(due(&c, "a").unwrap()["cards"], json!([]));
    }
    #[test]
    fn edited_cards_reset_schedule_but_unchanged_cards_retain_it() {
        let (c, p) = db();
        seed(&c, &p);
        mutate(
            &c,
            "a",
            &p,
            true,
            &json!({"command_id":"grade","expected_revision":1,"card_id":"one","remembered":true}),
        )
        .unwrap();
        let deck = deck(&c, "a", &p).unwrap();
        let old = deck["cards"][0].clone();
        let value=mutate(&c,"a",&p,false,&json!({"command_id":"edit","expected_revision":2,"cards":[{"id":"one","front":"2 + 2?","back":"4"},{"id":"two","front":"Changed","back":"Answer"}]})).unwrap();
        assert_eq!(value["cards"][0], old);
        assert_eq!(value["cards"][1]["reviews"], 0);
        assert!(
            mutate(
                &c,
                "a",
                &p,
                false,
                &json!({"command_id":"stale","expected_revision":1,"cards":[]})
            )
            .is_err()
        );
        assert_eq!(super::deck(&c, "a", &p).unwrap(), value);
    }
    #[test]
    fn export_escapes_markup_tabs_and_newlines_and_delete_cascades() {
        let (c, p) = db();
        seed(&c, &p);
        let export = export(&c, "a", None).unwrap();
        assert!(export.contains("&lt;script&gt;<br>Question \t&amp; answer\n"));
        assert_eq!(export.lines().filter(|l| !l.starts_with('#')).count(), 2);
        assert!(super::export(&c, "b", Some(&p)).is_err());
        documents::delete(&c, "a", &p).unwrap();
        assert_eq!(
            c.query_row("SELECT COUNT(*) FROM native_flashcard_decks", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
        assert_eq!(due(&c, "a").unwrap()["cards"], json!([]));
    }
    #[test]
    fn invalid_deck_writes_roll_back_without_publishing_receipts() {
        let (c, p) = db();
        let tx = c.unchecked_transaction().unwrap();
        let body = json!({"command_id":"invalid","expected_revision":0,"cards":[{"id":"x","front":"Q","back":"A"},{"id":"x","front":"Duplicate","back":"A"}]});
        assert!(mutate(&tx, "a", &p, false, &body).is_err());
        drop(tx);
        assert_eq!(deck(&c, "a", &p).unwrap()["revision"], 0);
        assert_eq!(
            c.query_row("SELECT COUNT(*) FROM native_flashcard_receipts", [], |r| {
                r.get::<_, i64>(0)
            })
            .unwrap(),
            0
        );
    }
}
