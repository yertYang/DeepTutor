//! Local calendar: RFC 5545 recurrence, scoped learning layers, global CAS and
//! idempotent mutations. Call in a transaction; reminders stay in the client.
use crate::{Error, Result, store::text};
use chrono::{DateTime, Datelike, Duration, NaiveDate, SecondsFormat, TimeZone, Utc};
use rrule::{RRuleSet, Tz};
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
const FIELDS: &[&str] = &[
    "calendar_id",
    "title",
    "notes",
    "location",
    "all_day",
    "start",
    "end",
    "tzid",
    "rrule",
    "reminders",
    "color",
    "task_id",
    "task_workspace_id",
];
fn invalid(message: &str) -> Error {
    Error::invalid(message)
}
fn object(v: &Value, fields: &[&str]) -> Result<()> {
    if v.as_object()
        .is_none_or(|o| o.keys().any(|k| !fields.contains(&k.as_str())))
    {
        return Err(invalid("Unexpected calendar field."));
    }
    Ok(())
}
fn new_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}
fn stamp() -> String {
    iso(Utc::now())
}
fn iso<T: TimeZone>(value: DateTime<T>) -> String {
    value
        .with_timezone(&Utc)
        .to_rfc3339_opts(SecondsFormat::Millis, true)
}
fn decode(raw: String) -> Result<Value> {
    serde_json::from_str(&raw).map_err(|_| Error::conflict("Invalid stored calendar data."))
}
pub(crate) fn optional(c: &Connection, kind: &str, id: &str) -> Result<Option<Value>> {
    c.query_row(
        "SELECT payload_json FROM native_calendar_objects WHERE kind=? AND id=?",
        params![kind, id],
        |r| r.get(0),
    )
    .optional()?
    .map(decode)
    .transpose()
}
fn default_calendar() -> Value {
    json!({"id":"default","name":"","color":"#2f6fde","visible":true,"position":0,"is_default":true,"created_at":"1970-01-01T00:00:00Z","updated_at":"1970-01-01T00:00:00Z"})
}
fn read(c: &Connection, kind: &str, id: &str) -> Result<Value> {
    optional(c, kind, id)?
        .or_else(|| (kind == "calendar" && id == "default").then(default_calendar))
        .ok_or_else(Error::missing)
}
pub(crate) fn put(c: &Connection, kind: &str, value: &Value) -> Result<()> {
    c.execute("INSERT INTO native_calendar_objects VALUES(?,?,?) ON CONFLICT(kind,id) DO UPDATE SET payload_json=excluded.payload_json",params![kind,text(value,"id"),value.to_string()])?;
    Ok(())
}
pub(crate) fn remove(c: &Connection, kind: &str, id: &str) -> Result<()> {
    c.execute(
        "DELETE FROM native_calendar_objects WHERE kind=? AND id=?",
        params![kind, id],
    )?;
    Ok(())
}
pub(crate) fn rows(c: &Connection, kind: &str) -> Result<Vec<Value>> {
    c.prepare(
        "SELECT payload_json FROM native_calendar_objects WHERE kind=? ORDER BY id LIMIT 5001",
    )?
    .query_map([kind], |r| r.get::<_, String>(0))?
    .collect::<rusqlite::Result<Vec<_>>>()?
    .into_iter()
    .map(decode)
    .collect()
}
pub fn revision(c: &Connection) -> Result<i64> {
    Ok(c.query_row(
        "SELECT revision FROM native_calendar_state WHERE id=1",
        [],
        |r| r.get(0),
    )?)
}
pub fn calendars(c: &Connection) -> Result<Value> {
    let mut all = rows(c, "calendar")?;
    if !all.iter().any(|v| v["id"] == "default") {
        all.push(default_calendar());
    }
    Ok(json!({"calendars":all,"revision":revision(c)?}))
}
fn color(value: &Value, nullable: bool) -> Result<()> {
    if nullable && value.is_null() {
        return Ok(());
    }
    let s = value
        .as_str()
        .ok_or_else(|| invalid("Invalid calendar color."))?;
    if s.len() != 7 || !s.starts_with('#') || !s[1..].bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(invalid("Use a six-digit hex color."));
    }
    Ok(())
}
fn string(v: &Value, key: &str, max: usize, required: bool) -> Result<String> {
    let value = crate::learning::string(v, key, max, required)?;
    if value
        .chars()
        .any(|c| c.is_control() && !['\n', '\r', '\t'].contains(&c))
    {
        return Err(invalid(
            "Calendar text contains invalid control characters.",
        ));
    }
    Ok(value)
}
fn date(v: &str) -> Result<NaiveDate> {
    NaiveDate::parse_from_str(v, "%Y-%m-%d").map_err(|_| invalid("Invalid calendar date."))
}
fn instant(v: &str) -> Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(v)
        .map(|d| d.with_timezone(&Utc))
        .map_err(|_| invalid("Use a complete date and time with a time zone."))
}
fn zone(v: &str) -> Result<Tz> {
    v.parse::<chrono_tz::Tz>()
        .map(Tz::Tz)
        .map_err(|_| invalid("Invalid IANA time zone."))
}
fn local(day: NaiveDate, tz: Tz) -> Result<DateTime<Tz>> {
    tz.from_local_datetime(&day.and_hms_opt(0, 0, 0).unwrap())
        .earliest()
        .ok_or_else(|| invalid("This local calendar date does not exist."))
}
fn bounds(v: &Value) -> Result<(DateTime<Tz>, DateTime<Tz>)> {
    let tz = zone(text(v, "tzid"))?;
    if v["all_day"] == true {
        Ok((
            local(date(text(v, "start"))?, tz)?,
            local(date(text(v, "end"))?, tz)?,
        ))
    } else {
        Ok((
            instant(text(v, "start"))?.with_timezone(&tz),
            instant(text(v, "end"))?.with_timezone(&tz),
        ))
    }
}
fn rule(v: &Value) -> Result<RRuleSet> {
    let (start, _) = bounds(v)?;
    let rule = text(v, "rrule");
    if rule.len() > 1000
        || rule.contains(['\r', '\n'])
        || !rule
            .split(';')
            .any(|p| ["FREQ=DAILY", "FREQ=WEEKLY", "FREQ=MONTHLY", "FREQ=YEARLY"].contains(&p))
    {
        return Err(invalid(
            "Use a daily, weekly, monthly or yearly recurrence rule.",
        ));
    }
    // rrule validates zoned UNTIL as UTC. Keep the authored date in storage
    // and exports, translating its inclusive local day only for expansion.
    let parts = rule
        .split(';')
        .map(|part| -> Result<String> {
            if v["all_day"] == true
                && let Some(until) = part.strip_prefix("UNTIL=").filter(|s| s.len() == 8)
            {
                let day = NaiveDate::parse_from_str(until, "%Y%m%d")
                    .map_err(|_| invalid("Invalid recurrence end date."))?;
                let end = local(day + Duration::days(1), start.timezone())? - Duration::seconds(1);
                return Ok(format!(
                    "UNTIL={}",
                    end.with_timezone(&Utc).format("%Y%m%dT%H%M%SZ")
                ));
            }
            Ok(part.into())
        })
        .collect::<Result<Vec<_>>>()?
        .join(";");
    let rule = parts
        .parse::<rrule::RRule<rrule::Unvalidated>>()
        .and_then(|r| r.validate(start))
        .map_err(|_| invalid("Invalid recurrence rule."))?;
    Ok(RRuleSet::new(start).rrule(rule))
}
fn event(c: &Connection, input: &Value, old: Option<&Value>) -> Result<Value> {
    object(input, FIELDS)?;
    let at = stamp();
    let mut v=old.cloned().unwrap_or_else(||json!({"id":new_id(),"calendar_id":"default","title":"","notes":"","location":"","all_day":false,"tzid":"UTC","rrule":null,"reminders":[],"color":null,"task_id":null,"task_workspace_id":null,"exdates":[],"series_id":null,"recurrence_id":null,"source":"user","created_at":at}));
    v.as_object_mut()
        .unwrap()
        .extend(input.as_object().unwrap().clone());
    if v["calendar_id"].is_null() {
        v["calendar_id"] = json!("default");
    }
    read(c, "calendar", text(&v, "calendar_id"))?;
    for (k, max) in [("title", 1000), ("notes", 16000), ("location", 2000)] {
        string(&v, k, max, false)?;
    }
    if !v["all_day"].is_boolean() {
        return Err(invalid("Specify all-day status."));
    }
    color(&v["color"], true)?;
    let (start, end) = bounds(&v)?;
    if !(1900..=2200).contains(&start.year()) || !(1900..=2201).contains(&end.year()) {
        return Err(invalid("Calendar dates must be between 1900 and 2200."));
    }
    if end <= start || end - start > Duration::days(366) {
        return Err(invalid(
            "Event end must follow its start by at most 366 days.",
        ));
    }
    if v["all_day"] != true {
        v["start"] = json!(iso(start));
        v["end"] = json!(iso(end));
    }
    let reminders = v["reminders"]
        .as_array()
        .filter(|a| a.len() <= 10)
        .ok_or_else(|| invalid("At most ten reminders are allowed."))?;
    if reminders
        .iter()
        .any(|n| n.as_i64().is_none_or(|n| !(-1440..=525600).contains(&n)))
    {
        return Err(invalid("Reminder minutes are out of range."));
    }
    for key in ["task_id", "task_workspace_id"] {
        if !v[key].is_null() {
            string(&v, key, 256, true)?;
        }
    }
    if let Some(id) = v["task_id"].as_str() {
        crate::task_board::exists(c, v["task_workspace_id"].as_str().unwrap_or(""), id)?;
    } else if !v["task_workspace_id"].is_null() {
        return Err(invalid("A task workspace requires a task ID."));
    }
    if !v["rrule"].is_null() {
        string(&v, "rrule", 1000, true)?;
        rule(&v)?;
    }
    v["updated_at"] = json!(at);
    Ok(v)
}
fn masters(c: &Connection) -> Result<Vec<Value>> {
    let rows = rows(c, "event")?;
    if rows.len() > 5000 {
        return Err(invalid("Calendar contains too many events."));
    }
    Ok(rows)
}
fn dates(v: &Value, from: DateTime<Utc>, to: DateTime<Utc>) -> Result<Vec<DateTime<Tz>>> {
    let (start, end) = bounds(v)?;
    if v["rrule"].is_null() {
        return Ok(vec![start]);
    }
    let length = (end - start).num_days() + 2;
    let result = rule(v)?
        .after((from - Duration::days(length)).with_timezone(&Tz::UTC))
        .before(to.with_timezone(&Tz::UTC))
        .all(10001);
    if result.limited || result.dates.len() > 10000 {
        return Err(invalid(
            "Too many calendar occurrences. Request a shorter range.",
        ));
    }
    Ok(result.dates)
}
fn occurrence(v: &Value, start: DateTime<Tz>, identity: &str, recurring: bool) -> Result<Value> {
    let (original_start, original_end) = bounds(v)?;
    let mut o = v.clone();
    let all = v["all_day"] == true;
    let end = if all {
        local(
            start.date_naive() + (original_end.date_naive() - original_start.date_naive()),
            start.timezone(),
        )?
    } else if v["rrule"].is_null() {
        original_end
    } else {
        let wall = original_end.naive_local() - original_start.naive_local();
        let elapsed = original_end - original_start;
        if wall <= Duration::zero() {
            start + elapsed
        } else {
            start
                .timezone()
                .from_local_datetime(&(start.naive_local() + wall))
                .earliest()
                .filter(|end| *end > start)
                .unwrap_or(start + elapsed)
        }
    };
    o["start"] = json!(if all {
        start.format("%Y-%m-%d").to_string()
    } else {
        iso(start)
    });
    o["end"] = json!(if all {
        end.format("%Y-%m-%d").to_string()
    } else {
        iso(end)
    });
    o["event_id"] = v["id"].clone();
    o["id"] = json!(format!("{}@{}", text(v, "id"), identity));
    o["occurrence"] = json!(identity);
    o["recurring"] = json!(recurring);
    o["reminder_times"] = json!(
        v["reminders"]
            .as_array()
            .unwrap()
            .iter()
            .map(|n| iso(start - Duration::minutes(n.as_i64().unwrap())))
            .collect::<Vec<_>>()
    );
    Ok(o)
}
fn identity(v: &Value, start: DateTime<Tz>) -> String {
    if v["all_day"] == true {
        start.format("%Y-%m-%d").to_string()
    } else {
        iso(start)
    }
}
pub fn occurrences(c: &Connection, from: &str, to: &str, tz: &str) -> Result<Value> {
    let from = instant(from)?;
    let to = instant(to)?;
    zone(tz)?;
    if to <= from || to - from > Duration::days(367) {
        return Err(invalid("Request a calendar range of at most 367 days."));
    }
    let all = masters(c)?;
    let overrides = all
        .iter()
        .filter(|v| !v["series_id"].is_null())
        .map(|v| {
            (
                (
                    text(v, "series_id").to_owned(),
                    text(v, "recurrence_id").to_owned(),
                ),
                v,
            )
        })
        .collect::<HashMap<_, _>>();
    let mut out = Vec::new();
    for v in all.iter().filter(|v| v["series_id"].is_null()) {
        for start in dates(v, from, to)? {
            let key = identity(v, start);
            if overrides.contains_key(&(text(v, "id").to_owned(), key.clone()))
                || v["exdates"]
                    .as_array()
                    .is_some_and(|a| a.iter().any(|x| x == &key))
                || v["native_until"]
                    .as_str()
                    .is_some_and(|cut| key.as_str() >= cut)
            {
                continue;
            }
            let o = occurrence(v, start, &key, !v["rrule"].is_null())?;
            let (a, b) = bounds(&o)?;
            if b.with_timezone(&Utc) > from && a.with_timezone(&Utc) < to {
                out.push(o);
            }
        }
    }
    for v in overrides.values() {
        let (start, _) = bounds(v)?;
        let o = occurrence(v, start, text(v, "recurrence_id"), true)?;
        let (a, b) = bounds(&o)?;
        if b.with_timezone(&Utc) > from && a.with_timezone(&Utc) < to {
            out.push(o);
        }
    }
    if out.len() > 10000 {
        return Err(invalid(
            "Calendar range contains more than 10000 occurrences.",
        ));
    }
    out.sort_by(|a, b| {
        text(a, "start")
            .cmp(text(b, "start"))
            .then_with(|| text(a, "id").cmp(text(b, "id")))
    });
    Ok(json!({"occurrences":out,"revision":revision(c)?}))
}
fn chosen(master: &Value, key: &str) -> Result<DateTime<Tz>> {
    let tz = zone(text(master, "tzid"))?;
    let dt = if master["all_day"] == true {
        local(date(key)?, tz)?
    } else {
        instant(key)?.with_timezone(&tz)
    };
    if !dates(
        master,
        dt.with_timezone(&Utc) - Duration::seconds(1),
        dt.with_timezone(&Utc) + Duration::seconds(1),
    )?
    .iter()
    .any(|d| identity(master, *d) == key)
    {
        return Err(invalid("This occurrence is not in the selected series."));
    }
    if master["native_until"]
        .as_str()
        .is_some_and(|cut| key >= cut)
    {
        return Err(Error::missing());
    }
    Ok(dt)
}
fn series(c: &Connection, id: &str) -> Result<(Value, Vec<Value>)> {
    let row = read(c, "event", id)?;
    let master = if row["series_id"].is_null() {
        row
    } else {
        read(c, "event", text(&row, "series_id"))?
    };
    let overrides = masters(c)?
        .into_iter()
        .filter(|v| v["series_id"] == master["id"])
        .collect();
    Ok((master, overrides))
}
fn patch_shift(master: &Value, changes: &Value, key: Option<&str>) -> Result<Value> {
    let Some(key) = key else {
        return Ok(changes.clone());
    };
    let selected = chosen(master, key)?;
    let old = occurrence(master, selected, key, true)?;
    let mut patch = changes.clone();
    let mut altered = old.clone();
    altered
        .as_object_mut()
        .unwrap()
        .extend(changes.as_object().unwrap().clone());
    let (a, b) = bounds(&altered)?;
    let (m, _) = bounds(master)?;
    if changes.get("start").is_some() || changes.get("end").is_some() {
        let moved = m.naive_local() + (a.naive_local() - selected.naive_local());
        let new_start = m
            .timezone()
            .from_local_datetime(&moved)
            .earliest()
            .ok_or_else(|| invalid("Invalid series start."))?;
        let new_end = m
            .timezone()
            .from_local_datetime(&(moved + (b.naive_local() - a.naive_local())))
            .earliest()
            .ok_or_else(|| invalid("Invalid series end."))?;
        let all = altered["all_day"] == true;
        patch["start"] = json!(if all {
            new_start.format("%Y-%m-%d").to_string()
        } else {
            iso(new_start)
        });
        patch["end"] = json!(if all {
            new_end.format("%Y-%m-%d").to_string()
        } else {
            iso(new_end)
        });
    }
    Ok(patch)
}
// Map exception identities by occurrence ordinal, rather than adding UTC seconds.
// This preserves weekday rules and local times over DST when a series is moved.
fn rebase_key(old: &Value, new: &Value, key: &str) -> Result<String> {
    if old["start"] == new["start"]
        && old["rrule"] == new["rrule"]
        && old["all_day"] == new["all_day"]
        && old["tzid"] == new["tzid"]
    {
        return Ok(key.into());
    }
    if old["rrule"].is_null() || new["rrule"].is_null() {
        return Err(invalid(
            "Remove series exceptions before removing recurrence.",
        ));
    }
    let tz = zone(text(old, "tzid"))?;
    let at = if old["all_day"] == true {
        local(date(key)?, tz)?
    } else {
        instant(key)?.with_timezone(&tz)
    };
    let before = rule(old)?
        .before((at - Duration::seconds(1)).with_timezone(&Tz::UTC))
        .all(32768);
    if before.limited || before.dates.len() >= 32767 {
        return Err(invalid("Series is too long to move safely."));
    }
    let index = before.dates.len();
    let generated = rule(new)?.all((index + 1) as u16);
    let at = generated
        .dates
        .get(index)
        .ok_or_else(|| invalid("The new recurrence would discard saved exceptions."))?;
    Ok(identity(new, *at))
}
fn rebase_exclusions(old: &Value, new: &mut Value) -> Result<()> {
    let keys = old["exdates"]
        .as_array()
        .unwrap()
        .iter()
        .map(|k| rebase_key(old, new, k.as_str().unwrap()).map(Value::String))
        .collect::<Result<Vec<_>>>()?;
    new["exdates"] = json!(keys);
    if let Some(key) = old["native_until"].as_str() {
        new["native_until"] = json!(rebase_key(old, new, key)?);
    }
    Ok(())
}
fn rebase_override(old: &Value, new: &Value, row: &Value) -> Result<Value> {
    let key = text(row, "recurrence_id");
    let next = rebase_key(old, new, key)?;
    let parse = |v: &Value, k: &str| -> Result<DateTime<Tz>> {
        let tz = zone(text(v, "tzid"))?;
        if v["all_day"] == true {
            local(date(k)?, tz)
        } else {
            Ok(instant(k)?.with_timezone(&tz))
        }
    };
    let a = parse(old, key)?;
    let b = parse(new, &next)?;
    let delta = b.naive_local() - a.naive_local();
    let mut row = row.clone();
    let (start, end) = bounds(&row)?;
    for (field, d) in [("start", start), ("end", end)] {
        let moved = d
            .timezone()
            .from_local_datetime(&(d.naive_local() + delta))
            .earliest()
            .ok_or_else(|| invalid("Moved exception falls in a missing local time."))?;
        row[field] = json!(if row["all_day"] == true {
            moved.format("%Y-%m-%d").to_string()
        } else {
            iso(moved)
        });
    }
    row["recurrence_id"] = json!(next);
    row["updated_at"] = json!(stamp());
    Ok(row)
}
fn edit(c: &Connection, id: &str, input: &Value) -> Result<Value> {
    object(input, &["changes", "scope", "occurrence"])?;
    object(&input["changes"], FIELDS)?;
    let scope = text(input, "scope");
    if !["all", "this", "following"].contains(&scope) {
        return Err(invalid("Invalid recurrence edit scope."));
    }
    let (master, overrides) = series(c, id)?;
    let key = input["occurrence"].as_str();
    if master["rrule"].is_null() || scope == "all" {
        if (!overrides.is_empty() || !master["exdates"].as_array().unwrap().is_empty())
            && ["all_day", "tzid", "rrule"]
                .iter()
                .any(|k| input["changes"].get(*k).is_some_and(|v| v != &master[*k]))
        {
            return Err(invalid(
                "This series has exceptions. Change its repeat pattern using this and following events.",
            ));
        }
        let patch = patch_shift(&master, &input["changes"], key)?;
        let mut value = event(c, &patch, Some(&master))?;
        rebase_exclusions(&master, &mut value)?;
        put(c, "event", &value)?;
        for old in overrides {
            let mut changed = rebase_override(&master, &value, &old)?;
            let mut patch = input["changes"].clone();
            for key in ["rrule", "start", "end", "all_day", "tzid"] {
                patch.as_object_mut().unwrap().remove(key);
            }
            changed = event(c, &patch, Some(&changed))?;
            put(c, "event", &changed)?;
        }
        return Ok(json!({"event":value}));
    }
    let key = key.ok_or_else(|| invalid("Choose a series occurrence."))?;
    let selected = chosen(&master, key)?;
    if scope == "this" {
        let existing = overrides.iter().find(|v| v["recurrence_id"] == key);
        let mut base = if let Some(v) = existing {
            v.clone()
        } else {
            let projection = occurrence(&master, selected, key, true)?;
            let mut v = master.clone();
            v["start"] = projection["start"].clone();
            v["end"] = projection["end"].clone();
            v["exdates"] = json!([]);
            v.as_object_mut().unwrap().remove("native_until");
            v["id"] = json!(new_id());
            v["series_id"] = master["id"].clone();
            v["recurrence_id"] = json!(key);
            v["rrule"] = Value::Null;
            v
        };
        // Occurrence projections have extra view fields; the event validator consumes only the patch.
        base["rrule"] = Value::Null;
        let mut value = event(c, &input["changes"], Some(&base))?;
        value["rrule"] = Value::Null;
        put(c, "event", &value)?;
        return Ok(json!({"event":value}));
    }
    let mut base = master.clone();
    base["id"] = json!(new_id());
    base["created_at"] = json!(stamp());
    base["exdates"] = json!(
        master["exdates"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|v| v.as_str().is_some_and(|v| v >= key))
            .cloned()
            .collect::<Vec<_>>()
    );
    let projection = occurrence(&master, selected, key, true)?;
    base["start"] = projection["start"].clone();
    base["end"] = projection["end"].clone();
    // COUNT belongs to the original series; split only its remaining instances.
    if !input["changes"].as_object().unwrap().contains_key("rrule")
        && let Some(rule_text) = master["rrule"].as_str()
        && let Some(count) = rule_text.split(';').find_map(|p| {
            p.strip_prefix("COUNT=")
                .and_then(|v| v.parse::<usize>().ok())
        })
    {
        let before = rule(&master)?
            .before((selected - Duration::seconds(1)).with_timezone(&Tz::UTC))
            .all(32768);
        if before.limited {
            return Err(invalid("Series is too long to split safely."));
        }
        let remaining = count.saturating_sub(before.dates.len());
        if remaining == 0 {
            return Err(Error::missing());
        }
        base["rrule"] = json!(
            rule_text
                .split(';')
                .map(|p| if p.starts_with("COUNT=") {
                    format!("COUNT={remaining}")
                } else {
                    p.into()
                })
                .collect::<Vec<_>>()
                .join(";")
        );
    }
    let mut value = event(c, &input["changes"], Some(&base))?;
    rebase_exclusions(&base, &mut value)?;
    let mut previous = master.clone();
    previous["native_until"] = json!(key);
    previous["updated_at"] = json!(stamp());
    put(c, "event", &previous)?;
    put(c, "event", &value)?;
    for old in overrides {
        if text(&old, "recurrence_id") >= key {
            let mut v = rebase_override(&base, &value, &old)?;
            v["series_id"] = value["id"].clone();
            put(c, "event", &v)?;
        }
    }
    Ok(json!({"event":value}))
}
fn erase(c: &Connection, id: &str, scope: &str, key: Option<&str>) -> Result<Value> {
    if !["all", "this", "following"].contains(&scope) {
        return Err(invalid("Invalid recurrence delete scope."));
    }
    let (master, overrides) = series(c, id)?;
    let mut before = Vec::new();
    let mut touched = Vec::new();
    before.push(master.clone());
    touched.push(text(&master, "id").to_owned());
    if master["rrule"].is_null() || scope == "all" {
        remove(c, "event", text(&master, "id"))?;
        for v in overrides {
            touched.push(text(&v, "id").to_owned());
            remove(c, "event", text(&v, "id"))?;
            before.push(v);
        }
    } else {
        let key = key.ok_or_else(|| invalid("Choose a series occurrence."))?;
        chosen(&master, key)?;
        let mut v = master.clone();
        if scope == "this" {
            if !v["exdates"].as_array().unwrap().iter().any(|x| x == key) {
                v["exdates"].as_array_mut().unwrap().push(json!(key));
            }
        } else {
            v["native_until"] = json!(key);
        }
        v["updated_at"] = json!(stamp());
        put(c, "event", &v)?;
        for old in overrides {
            if (scope == "this" && old["recurrence_id"] == key)
                || (scope == "following" && text(&old, "recurrence_id") >= key)
            {
                touched.push(text(&old, "id").to_owned());
                remove(c, "event", text(&old, "id"))?;
                before.push(old);
            }
        }
    }
    let after = touched
        .iter()
        .map(|id| optional(c, "event", id).map(|v| json!({"id":id,"value":v})))
        .collect::<Result<Vec<_>>>()?;
    let token = new_id();
    let after_series = series_state(c, text(&master, "id"))?;
    c.execute(
        "INSERT INTO native_calendar_undo VALUES(?,?)",
        params![
            token,
            json!({"before":before,"after":after,"series_id":master["id"],"after_series":after_series,"expires":Utc::now().timestamp()+7*86400})
                .to_string()
        ],
    )?;
    Ok(json!({"undo_token":token}))
}
fn series_state(c: &Connection, id: &str) -> Result<Vec<Value>> {
    Ok(masters(c)?
        .into_iter()
        .filter(|v| v["id"] == id || v["series_id"] == id)
        .collect())
}
fn undo(c: &Connection, token: &str) -> Result<Value> {
    let raw = c
        .query_row(
            "SELECT payload_json FROM native_calendar_undo WHERE id=?",
            [token],
            |r| r.get(0),
        )
        .optional()?
        .ok_or_else(Error::missing)?;
    let saved = decode(raw)?;
    if saved["expires"].as_i64().unwrap_or(0) < Utc::now().timestamp() {
        return Err(Error::conflict("This undo token expired."));
    }
    if json!(series_state(c, text(&saved, "series_id"))?) != saved["after_series"] {
        return Err(Error::conflict(
            "The series changed since deletion. Undo would overwrite newer changes.",
        ));
    }
    for row in saved["after"].as_array().unwrap() {
        if json!(optional(c, "event", text(row, "id"))?) != row["value"] {
            return Err(Error::conflict(
                "The series changed since deletion. Undo would overwrite newer changes.",
            ));
        }
    }
    for row in saved["before"].as_array().unwrap() {
        read(c, "calendar", text(row, "calendar_id"))?;
        put(c, "event", row)?;
    }
    c.execute("DELETE FROM native_calendar_undo WHERE id=?", [token])?;
    Ok(json!({"restored":true}))
}

/// Every write carries the last observed global revision and a stable command ID.
/// Retry receipts are checked first; a stale retry cannot replay destructive work.
pub fn mutate(
    c: &Connection,
    expected: i64,
    command: &str,
    operation: &str,
    id: &str,
    input: &Value,
) -> Result<Value> {
    if command.is_empty() || command.len() > 128 {
        return Err(invalid("A calendar command ID is required."));
    }
    let digest = format!(
        "{:x}",
        Sha256::digest(json!({"operation":operation,"id":id,"input":input}).to_string())
    );
    let prior: Option<(String, String)> = c
        .query_row(
            "SELECT request_digest,payload_json FROM native_calendar_receipts WHERE command_id=?",
            [command],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    if let Some((saved, raw)) = prior {
        if saved != digest {
            return Err(Error::conflict(
                "Calendar command ID was reused with different content.",
            ));
        }
        let mut v = if operation == "tasks" {
            crate::task_board::board(c, id)?
        } else {
            decode(raw)?
        };
        v["revision"] = json!(revision(c)?);
        return Ok(v);
    }
    let current = revision(c)?;
    if current != expected {
        return Err(Error::conflict(
            "Calendar changed elsewhere. Reload before saving; your edits have not been applied.",
        ));
    }
    let mut result = match operation {
        "calendar_create" | "calendar_update" => {
            object(input, &["name", "color", "visible", "position"])?;
            let mut v = if operation == "calendar_create" {
                if rows(c, "calendar")?.len() >= 100 {
                    return Err(invalid("At most 100 calendars are allowed."));
                }
                json!({"id":new_id(),"name":"","color":"#2f6fde","visible":true,"position":0,"is_default":false,"created_at":stamp()})
            } else {
                read(c, "calendar", id)?
            };
            v.as_object_mut()
                .unwrap()
                .extend(input.as_object().unwrap().clone());
            string(&v, "name", 400, operation == "calendar_create")?;
            color(&v["color"], false)?;
            if !v["visible"].is_boolean()
                || v["position"]
                    .as_i64()
                    .is_none_or(|n| !(-10000..=10000).contains(&n))
            {
                return Err(invalid("Invalid calendar visibility or order."));
            }
            v["updated_at"] = json!(stamp());
            put(c, "calendar", &v)?;
            json!({"calendar":v})
        }
        "calendar_delete" => {
            read(c, "calendar", id)?;
            if id == "default" {
                return Err(invalid("The default calendar cannot be deleted."));
            }
            let all = masters(c)?;
            let removed = all
                .iter()
                .filter(|v| v["calendar_id"] == id && v["series_id"].is_null())
                .map(|v| text(v, "id").to_owned())
                .collect::<std::collections::HashSet<_>>();
            for mut v in all {
                if v["calendar_id"] == id {
                    if let Some(parent) = v["series_id"].as_str().filter(|p| !removed.contains(*p))
                    {
                        let mut master = read(c, "event", parent)?;
                        if !master["exdates"]
                            .as_array()
                            .unwrap()
                            .contains(&v["recurrence_id"])
                        {
                            master["exdates"]
                                .as_array_mut()
                                .unwrap()
                                .push(v["recurrence_id"].clone());
                        }
                        master["updated_at"] = json!(stamp());
                        put(c, "event", &master)?;
                    }
                    remove(c, "event", text(&v, "id"))?;
                } else if v["series_id"].as_str().is_some_and(|p| removed.contains(p)) {
                    // An exception moved to another calendar survives as an independent event.
                    v["series_id"] = Value::Null;
                    v["recurrence_id"] = Value::Null;
                    put(c, "event", &v)?;
                }
            }
            remove(c, "calendar", id)?;
            json!({"deleted":true})
        }
        "event_create" => {
            if masters(c)?.len() >= 5000 {
                return Err(invalid("At most 5000 events are allowed."));
            }
            let v = event(c, input, None)?;
            put(c, "event", &v)?;
            json!({"event":v})
        }
        "event_update" => edit(c, id, input)?,
        "event_delete" => {
            object(input, &["scope", "occurrence"])?;
            erase(c, id, text(input, "scope"), input["occurrence"].as_str())?
        }
        "tasks" => crate::task_board::change(c, id, input)?,
        "undo" => undo(c, id)?,
        _ => return Err(invalid("Unknown calendar operation.")),
    };
    c.execute(
        "UPDATE native_calendar_state SET revision=revision+1 WHERE id=1",
        [],
    )?;
    result["revision"] = json!(current + 1);
    c.execute(
        "INSERT INTO native_calendar_receipts VALUES(?,?,?)",
        params![command, digest, result.to_string()],
    )?;
    Ok(result)
}
pub fn search(c: &Connection, query: &str) -> Result<Value> {
    if query.trim().is_empty() || query.len() > 500 {
        return Err(invalid("Enter a calendar search of at most 500 bytes."));
    }
    let query = query.to_lowercase();
    let mut result = Vec::new();
    let now = Utc::now();
    let upcoming = occurrences(c, &iso(now), &iso(now + Duration::days(366)), "UTC")?;
    for mut v in masters(c)? {
        if ["title", "notes", "location"]
            .iter()
            .any(|k| text(&v, k).to_lowercase().contains(&query))
        {
            v["next_start"] = upcoming["occurrences"]
                .as_array()
                .unwrap()
                .iter()
                .find(|o| o["event_id"] == v["id"])
                .map(|o| o["start"].clone())
                .unwrap_or(Value::Null);
            result.push(v);
            if result.len() == 50 {
                break;
            }
        }
    }
    Ok(json!({"events":result,"revision":revision(c)?}))
}
pub fn layers(c: &Connection, scope: &str, from: &str, to: &str, tz: &str) -> Result<Value> {
    let start = instant(from)?;
    let end = instant(to)?;
    let tz = zone(tz)?;
    if end <= start || end - start > Duration::days(367) {
        return Err(invalid("Invalid learning calendar range."));
    }
    let progress = crate::study::progress(c, scope)?;
    let mut days = std::collections::BTreeMap::<String, Value>::new();
    for row in progress["objectives"].as_array().unwrap() {
        let time = row["review_due_at"].as_f64().unwrap_or(0.0);
        let Some(at) = DateTime::from_timestamp(time as i64, 0) else {
            continue;
        };
        if at < start || at >= end {
            continue;
        }
        let key = at.with_timezone(&tz).format("%Y-%m-%d").to_string();
        let day = days
            .entry(key.clone())
            .or_insert_with(|| json!({"date":key,"mastery":0,"practice":0,"paths":[]}));
        day["mastery"] = json!(day["mastery"].as_i64().unwrap() + 1);
        day["paths"]
            .as_array_mut()
            .unwrap()
            .push(json!({"id":row["plan_id"],"title":row["objective_id"],"count":1}));
    }
    let sessions=c.prepare("SELECT id,title,created_at FROM sessions WHERE deleted_at IS NULL AND COALESCE(json_extract(preferences_json,'$.workspace_id'),'')=? AND created_at>=? AND created_at<? ORDER BY created_at LIMIT 10000")?.query_map(params![scope,start.timestamp() as f64,end.timestamp() as f64],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,f64>(2)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
    let mut footprints = std::collections::BTreeMap::<String, Value>::new();
    for (id, title, at) in sessions {
        if let Some(at) = DateTime::from_timestamp(at as i64, 0) {
            let key = at.with_timezone(&tz).format("%Y-%m-%d").to_string();
            let day=footprints.entry(key.clone()).or_insert_with(||json!({"date":key,"session_count":0,"sessions":[],"practice_reviews":0,"mastery_evidence":0}));
            day["session_count"] = json!(day["session_count"].as_i64().unwrap() + 1);
            day["sessions"]
                .as_array_mut()
                .unwrap()
                .push(json!({"id":id,"title":title}));
        }
    }
    Ok(
        json!({"workspace_id":scope,"reviews":days.into_values().collect::<Vec<_>>(),"automations":[],"footprints":footprints.into_values().collect::<Vec<_>>(),"revision":revision(c)?}),
    )
}
fn escape_ics(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace(';', "\\;")
        .replace(',', "\\,")
        .replace('\r', "")
        .replace('\n', "\\n")
}
fn fold(line: &str) -> String {
    let mut out = String::new();
    let mut length = 0;
    for ch in line.chars() {
        if length + ch.len_utf8() > 74 {
            out.push_str("\r\n ");
            length = 1;
        }
        out.push(ch);
        length += ch.len_utf8();
    }
    out.push_str("\r\n");
    out
}
fn ics_field(name: &str, v: &Value, d: DateTime<Tz>) -> String {
    if v["all_day"] == true {
        format!("{name};VALUE=DATE:{}", d.format("%Y%m%d"))
    } else if v["rrule"].is_null() && !["RECURRENCE-ID", "EXDATE"].contains(&name) {
        format!("{name}:{}", d.with_timezone(&Utc).format("%Y%m%dT%H%M%SZ"))
    } else {
        format!(
            "{name};TZID={}:{}",
            text(v, "tzid"),
            d.format("%Y%m%dT%H%M%S")
        )
    }
}
pub fn export(c: &Connection, calendar: Option<&str>) -> Result<String> {
    if let Some(id) = calendar {
        read(c, "calendar", id)?;
    }
    let mut lines = vec![
        "BEGIN:VCALENDAR".to_owned(),
        "VERSION:2.0".into(),
        "PRODID:-//Lorevia//Native Calendar//EN".into(),
        "CALSCALE:GREGORIAN".into(),
    ];
    let all_events = masters(c)?;
    for v in &all_events {
        if calendar.is_some_and(|id| v["calendar_id"] != id) {
            continue;
        }
        let (start, end) = bounds(v)?;
        let all = v["all_day"] == true;
        let field = |name: &str, d: DateTime<Tz>| ics_field(name, v, d);
        let uid = if v["series_id"].is_null() {
            text(v, "id")
        } else {
            text(v, "series_id")
        };
        lines.extend([
            "BEGIN:VEVENT".into(),
            format!("UID:{uid}@lorevia"),
            format!("DTSTAMP:{}", Utc::now().format("%Y%m%dT%H%M%SZ")),
            field("DTSTART", start),
            field("DTEND", end),
            format!("SUMMARY:{}", escape_ics(text(v, "title"))),
            format!("DESCRIPTION:{}", escape_ics(text(v, "notes"))),
            format!("LOCATION:{}", escape_ics(text(v, "location"))),
        ]);
        if !v["rrule"].is_null() {
            let mut rule_text = text(v, "rrule").to_owned();
            if let Some(cut) = v["native_until"].as_str() {
                let at = if all {
                    local(date(cut)?, zone(text(v, "tzid"))?)?.with_timezone(&Utc)
                } else {
                    instant(cut)?
                } - Duration::seconds(1);
                let until = if all {
                    (date(cut)? - Duration::days(1))
                        .format("%Y%m%d")
                        .to_string()
                } else {
                    at.format("%Y%m%dT%H%M%SZ").to_string()
                };
                rule_text = rule_text
                    .split(';')
                    .filter(|p| !p.starts_with("COUNT=") && !p.starts_with("UNTIL="))
                    .collect::<Vec<_>>()
                    .join(";")
                    + ";UNTIL="
                    + &until;
            }
            lines.push(format!("RRULE:{rule_text}"));
        }
        if let Some(key) = v["recurrence_id"].as_str() {
            let parent = all_events
                .iter()
                .find(|p| p["id"] == v["series_id"])
                .ok_or_else(|| Error::conflict("Calendar exception has no parent series."))?;
            let tz = zone(text(parent, "tzid"))?;
            let d = if parent["all_day"] == true {
                local(date(key)?, tz)?
            } else {
                instant(key)?.with_timezone(&tz)
            };
            lines.push(ics_field("RECURRENCE-ID", parent, d));
        }
        for key in v["exdates"].as_array().unwrap() {
            let key = key.as_str().unwrap();
            let d = if all {
                local(date(key)?, zone(text(v, "tzid"))?)?
            } else {
                instant(key)?.with_timezone(&zone(text(v, "tzid"))?)
            };
            lines.push(field("EXDATE", d));
        }
        for minutes in v["reminders"].as_array().unwrap() {
            let minutes = minutes.as_i64().unwrap();
            lines.extend([
                "BEGIN:VALARM".into(),
                "ACTION:DISPLAY".into(),
                format!("DESCRIPTION:{}", escape_ics(text(v, "title"))),
                format!(
                    "TRIGGER:{}PT{}M",
                    if minutes >= 0 { "-" } else { "" },
                    minutes.abs()
                ),
                "END:VALARM".into(),
            ]);
        }
        lines.push("END:VEVENT".into());
    }
    lines.push("END:VCALENDAR".into());
    Ok(lines.iter().map(|l| fold(l)).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn db() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(include_str!("schema.sql")).unwrap();
        c
    }
    fn command(c: &Connection, op: &str, id: &str, input: Value) -> Value {
        let tx = c.unchecked_transaction().unwrap();
        let result = mutate(&tx, revision(&tx).unwrap(), &new_id(), op, id, &input).unwrap();
        tx.commit().unwrap();
        result
    }
    fn seed(c: &Connection, extra: Value) -> Value {
        let mut input = json!({"title":"Study","start":"2026-10-25T13:00:00Z","end":"2026-10-25T14:00:00Z","tzid":"America/New_York","rrule":"FREQ=WEEKLY;COUNT=5","reminders":[15]});
        input
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        command(c, "event_create", "", input)["event"].clone()
    }
    fn range(c: &Connection, from: &str, to: &str) -> Vec<Value> {
        occurrences(c, from, to, "UTC").unwrap()["occurrences"]
            .as_array()
            .unwrap()
            .clone()
    }
    fn month(c: &Connection) -> Vec<Value> {
        range(c, "2026-10-01T00:00:00Z", "2026-12-01T00:00:00Z")
    }
    #[test]
    fn weekly_dst_keeps_local_clock_and_reminders() {
        let c = db();
        seed(&c, json!({}));
        let values = month(&c);
        assert_eq!(values.len(), 5);
        assert_eq!(values[0]["start"], "2026-10-25T13:00:00.000Z");
        assert_eq!(values[1]["start"], "2026-11-01T14:00:00.000Z");
        assert_eq!(values[1]["reminder_times"][0], "2026-11-01T13:45:00.000Z");
    }
    #[test]
    fn all_day_has_exclusive_end_over_dst() {
        let c = db();
        seed(
            &c,
            json!({"all_day":true,"start":"2026-10-31","end":"2026-11-02","rrule":null}),
        );
        assert_eq!(
            range(&c, "2026-11-02T05:00:00Z", "2026-11-03T05:00:00Z").len(),
            0
        );
        assert_eq!(
            range(&c, "2026-11-01T05:00:00Z", "2026-11-02T05:00:00Z")[0]["end"],
            "2026-11-02"
        );
    }
    #[test]
    fn last_weekday_monthly_count() {
        let c = db();
        seed(
            &c,
            json!({"start":"2026-10-30T13:00:00Z","end":"2026-10-30T14:00:00Z","rrule":"FREQ=MONTHLY;BYDAY=-1FR;COUNT=3"}),
        );
        let values = range(&c, "2026-10-01T00:00:00Z", "2027-02-01T00:00:00Z");
        assert_eq!(values.len(), 3);
        assert_eq!(values[1]["start"], "2026-11-27T14:00:00.000Z");
    }
    #[test]
    fn moved_override_is_found_outside_original_range() {
        let c = db();
        let v = seed(&c, json!({}));
        command(
            &c,
            "event_update",
            text(&v, "id"),
            json!({"scope":"this","occurrence":"2026-11-01T14:00:00.000Z","changes":{"start":"2027-01-02T14:00:00Z","end":"2027-01-02T15:00:00Z"}}),
        );
        assert_eq!(month(&c).len(), 4);
        let rows = range(&c, "2027-01-01T00:00:00Z", "2027-01-04T00:00:00Z");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["occurrence"], "2026-11-01T14:00:00.000Z");
    }
    #[test]
    fn following_split_preserves_count_and_deleted_instances() {
        let c = db();
        let v = seed(&c, json!({}));
        command(
            &c,
            "event_delete",
            text(&v, "id"),
            json!({"scope":"this","occurrence":"2026-11-15T14:00:00.000Z"}),
        );
        command(
            &c,
            "event_update",
            text(&v, "id"),
            json!({"scope":"following","occurrence":"2026-11-01T14:00:00.000Z","changes":{"start":"2026-11-01T15:00:00Z","end":"2026-11-01T16:00:00Z"}}),
        );
        let values = month(&c);
        assert_eq!(values.len(), 4);
        assert_eq!(values[1]["start"], "2026-11-01T15:00:00.000Z");
        assert!(
            !values
                .iter()
                .any(|v| text(v, "start").starts_with("2026-11-15"))
        );
    }
    #[test]
    fn shifting_all_rebases_override_identity_and_exclusions() {
        let c = db();
        let v = seed(&c, json!({}));
        let id = text(&v, "id");
        command(
            &c,
            "event_update",
            id,
            json!({"scope":"this","occurrence":"2026-11-01T14:00:00.000Z","changes":{"title":"Exception","start":"2026-11-02T14:00:00Z","end":"2026-11-02T16:00:00Z"}}),
        );
        command(
            &c,
            "event_delete",
            id,
            json!({"scope":"this","occurrence":"2026-11-08T14:00:00.000Z"}),
        );
        command(
            &c,
            "event_update",
            id,
            json!({"scope":"all","changes":{"start":"2026-10-25T14:00:00Z","end":"2026-10-25T15:00:00Z"}}),
        );
        let values = month(&c);
        assert_eq!(values.len(), 4);
        let changed = values.iter().find(|v| v["title"] == "Exception").unwrap();
        assert_eq!(changed["occurrence"], "2026-11-01T15:00:00.000Z");
        assert_eq!(changed["start"], "2026-11-02T15:00:00.000Z");
        assert_eq!(changed["end"], "2026-11-02T17:00:00.000Z");
    }
    #[test]
    fn delete_and_undo_preserves_exact_data_and_rejects_newer_edits() {
        let c = db();
        let v = seed(&c, json!({}));
        let id = text(&v, "id");
        let deleted = command(
            &c,
            "event_delete",
            id,
            json!({"scope":"this","occurrence":"2026-11-01T14:00:00.000Z"}),
        );
        assert_eq!(month(&c).len(), 4);
        command(&c, "undo", text(&deleted, "undo_token"), json!({}));
        assert_eq!(read(&c, "event", id).unwrap(), v);
        let deleted = command(
            &c,
            "event_delete",
            id,
            json!({"scope":"this","occurrence":"2026-11-01T14:00:00.000Z"}),
        );
        command(
            &c,
            "event_update",
            id,
            json!({"scope":"all","changes":{"notes":"newer"}}),
        );
        assert!(undo(&c, text(&deleted, "undo_token")).is_err());
        assert_eq!(read(&c, "event", id).unwrap()["notes"], "newer");
    }
    #[test]
    fn receipts_are_idempotent_and_conflicts_roll_back() {
        let c = db();
        let input = json!({"start":"2026-11-01T10:00:00Z","end":"2026-11-01T11:00:00Z"});
        let first = mutate(&c, 0, "same", "event_create", "", &input).unwrap();
        let retry = mutate(&c, 0, "same", "event_create", "", &input).unwrap();
        assert_eq!(first, retry);
        assert!(mutate(&c, 0, "other", "event_create", "", &input).is_err());
        assert!(mutate(&c, 1, "same", "event_create", "", &json!({})).is_err());
        assert_eq!(masters(&c).unwrap().len(), 1);
        let tx = c.unchecked_transaction().unwrap();
        assert!(
            mutate(
                &tx,
                1,
                "bad",
                "event_update",
                text(&first["event"], "id"),
                &json!({"scope":"all","changes":{"end":"invalid"}})
            )
            .is_err()
        );
        drop(tx);
        assert_eq!(revision(&c).unwrap(), 1);
        assert_eq!(
            read(&c, "event", text(&first["event"], "id")).unwrap(),
            first["event"]
        );
    }
    #[test]
    fn deleting_calendar_preserves_other_calendar_exceptions() {
        let c = db();
        let cal = command(&c, "calendar_create", "", json!({"name":"Other"}))["calendar"].clone();
        let v = seed(&c, json!({"calendar_id":cal["id"]}));
        command(
            &c,
            "event_update",
            text(&v, "id"),
            json!({"scope":"this","occurrence":"2026-11-01T14:00:00.000Z","changes":{"calendar_id":"default"}}),
        );
        command(&c, "calendar_delete", text(&cal, "id"), json!({}));
        let rows = month(&c);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["recurring"], false);
        assert!(rows[0]["series_id"].is_null());
    }
    #[test]
    fn deleting_override_calendar_does_not_resurrect_original() {
        let c = db();
        let cal = command(&c, "calendar_create", "", json!({"name":"Other"}))["calendar"].clone();
        let v = seed(&c, json!({}));
        command(
            &c,
            "event_update",
            text(&v, "id"),
            json!({"scope":"this","occurrence":"2026-11-01T14:00:00.000Z","changes":{"calendar_id":cal["id"]}}),
        );
        command(&c, "calendar_delete", text(&cal, "id"), json!({}));
        assert_eq!(month(&c).len(), 4);
    }
    #[test]
    fn ics_preserves_local_recurrence_parent_identity_and_unicode() {
        let c = db();
        let v = seed(
            &c,
            json!({"title":"学习".repeat(80),"notes":"hello\nBEGIN:VEVENT;injection"}),
        );
        command(
            &c,
            "event_update",
            text(&v, "id"),
            json!({"scope":"this","occurrence":"2026-11-01T14:00:00.000Z","changes":{"all_day":true,"start":"2026-11-01","end":"2026-11-02"}}),
        );
        let ics = export(&c, None).unwrap();
        assert!(ics.contains("DTSTART;TZID=America/New_York:20261025T090000"));
        assert!(ics.contains("RECURRENCE-ID;TZID=America/New_York:20261101T090000"));
        assert!(ics.contains("DTSTART;VALUE=DATE:20261101"));
        assert!(ics.contains("DESCRIPTION:hello\\nBEGIN:VEVENT\\;injection"));
        for line in ics.split("\r\n") {
            assert!(line.len() <= 75);
        }
        assert_eq!(ics.matches("BEGIN:VEVENT\r\n").count(), 2);
    }
    #[test]
    fn rejects_invalid_rules_controls_zones_and_large_ranges() {
        let c = db();
        for extra in [
            json!({"title":"bad\u{0000}"}),
            json!({"rrule":"FREQ=SECONDLY"}),
            json!({"tzid":"invalid"}),
            json!({"start":"1800-01-01T00:00:00Z"}),
        ] {
            let mut input = json!({"start":"2026-11-01T10:00:00Z","end":"2026-11-01T11:00:00Z"});
            input
                .as_object_mut()
                .unwrap()
                .extend(extra.as_object().unwrap().clone());
            assert!(event(&c, &input, None).is_err());
        }
        assert!(occurrences(&c, "2026-01-01T00:00:00Z", "2028-01-01T00:00:00Z", "UTC").is_err());
    }
}
