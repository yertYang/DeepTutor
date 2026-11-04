//! Evidence-driven objective progression. Known answers never count as fresh
//! evidence or postpone a spaced review; prerequisites unlock after a full round.
use crate::{Error, Result, store::text};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
const THRESHOLD: usize = 3;
const INTERVALS: [f64; 6] = [1., 3., 7., 14., 30., 60.];
#[derive(Default)]
struct Objective {
    seen: HashSet<String>,
    verified: HashSet<String>,
    latest: HashMap<String, bool>,
    attempts: usize,
    mastered: bool,
    stage: usize,
    round: usize,
    lapses: usize,
    due: Option<f64>,
    last: f64,
    observed_at: Option<f64>,
    repair: bool,
}
impl Objective {
    fn observe(&mut self, row: &Value) -> Result<()> {
        let key = text(row, "question_fingerprint");
        if key.is_empty() {
            return Err(Error::conflict(
                "Practice evidence has no question fingerprint.",
            ));
        }
        let correct = row["correct"]
            .as_bool()
            .ok_or_else(|| Error::conflict("Invalid practice evidence."))?;
        let at = row["created_at"]
            .as_f64()
            .filter(|t| t.is_finite())
            .ok_or_else(|| Error::conflict("Invalid practice timestamp."))?;
        let at = self.observed_at.map_or(at, |previous| previous.max(at));
        self.observed_at = Some(at);
        self.attempts += 1;
        let previous = self.latest.insert(key.into(), correct);
        let fresh = self.seen.insert(key.into());
        // A repeated failure adds no new delay. Correcting an already revealed answer
        // is useful practice, but cannot complete a new assessment round.
        if !correct && (fresh || previous == Some(true)) {
            self.round = 0;
            self.lapses += 1;
            self.due = Some(self.due.unwrap_or(at + 600.).min(at + 600.));
            self.repair = self.mastered;
            if self.mastered {
                self.stage = self.stage.saturating_sub(1).max(1);
            }
        }
        if !fresh || !correct {
            return Ok(());
        }
        self.verified.insert(key.into());
        self.last = at;
        if !self.mastered || self.due.is_some_and(|due| at >= due) {
            self.round += 1;
            if self.round >= THRESHOLD {
                self.stage = if self.mastered {
                    (self.stage + 1).min(INTERVALS.len())
                } else {
                    1
                };
                self.mastered = true;
                self.repair = false;
                self.round = 0;
                self.due = Some(at + INTERVALS[self.stage - 1] * 86400.);
            }
        }
        Ok(())
    }
}
pub fn summarize(plans: &[Value], attempts: &[Value], now: f64) -> Result<Vec<Value>> {
    let mut evidence = HashMap::<(String, String), Objective>::new();
    for row in attempts {
        let plan = text(row, "plan_id");
        let objective = text(row, "objective_id");
        if plan.is_empty() || objective.is_empty() {
            continue;
        }
        evidence
            .entry((plan.into(), objective.into()))
            .or_default()
            .observe(row)?;
    }
    let mut out = Vec::new();
    for plan in plans {
        let units = plan["result"]["units"]
            .as_array()
            .ok_or_else(|| Error::conflict("Invalid saved learning path."))?;
        for unit in units {
            let key = (text(plan, "id").to_owned(), text(unit, "id").to_owned());
            let empty = Objective::default();
            let state = evidence.get(&key).unwrap_or(&empty);
            let mut blocked = Vec::new();
            let mut prerequisites = Vec::new();
            for prerequisite in unit["prerequisites"]
                .as_array()
                .ok_or_else(|| Error::conflict("Invalid path prerequisites."))?
            {
                let parent = prerequisite
                    .as_u64()
                    .and_then(|i| units.get(i as usize))
                    .ok_or_else(|| Error::conflict("Invalid path prerequisite index."))?;
                let id = text(parent, "id");
                prerequisites.push(id);
                if !evidence
                    .get(&(key.0.clone(), id.into()))
                    .is_some_and(|s| s.mastered)
                {
                    blocked.push(id);
                }
            }
            let status = if !blocked.is_empty() {
                "locked"
            } else if !state.mastered {
                if state.attempts == 0 {
                    "ready"
                } else {
                    "practicing"
                }
            } else if state.due.is_some_and(|due| due <= now) {
                "review_due"
            } else if state.repair {
                "review_scheduled"
            } else {
                "mastered"
            };
            out.push(json!({"plan_id":plan["id"],"plan_title":plan["title"],"objective_id":unit["id"],"title":unit["title"],"goals":unit["goals"],"prerequisites":prerequisites,"blocked_by":blocked,"status":status,"correct_questions":state.verified.len(),"practice_correct_questions":state.latest.values().filter(|c|**c).count(),"attempt_count":state.attempts,"round_correct":state.round,"round_required":THRESHOLD,"review_stage":state.stage,"review_due_at":state.due,"ever_mastered":state.mastered,"lapses":state.lapses,"last_evidence_at":if state.last>0.{Some(state.last)}else{None}}));
        }
    }
    Ok(out)
}
#[cfg(test)]
mod tests {
    use super::*;
    fn plans() -> Vec<Value> {
        vec![
            json!({"id":"p","title":"Path","result":{"units":[{"id":"O1","title":"Foundation","goals":["Learn"],"prerequisites":[]},{"id":"O2","title":"Next","goals":["Build"],"prerequisites":[0]}]}}),
        ]
    }
    fn row(key: &str, correct: bool, at: f64) -> Value {
        json!({"plan_id":"p","objective_id":"O1","question_fingerprint":key,"correct":correct,"created_at":at})
    }
    #[test]
    fn prerequisites_require_three_fresh_correct_questions_and_retries_cannot_delay_review() {
        let mut rows = vec![row("a", true, 1.), row("a", true, 2.), row("b", true, 3.)];
        let value = summarize(&plans(), &rows, 4.).unwrap();
        assert_eq!(value[0]["round_correct"], 2);
        assert_eq!(value[1]["status"], "locked");
        rows.push(row("c", true, 5.));
        let value = summarize(&plans(), &rows, 6.).unwrap();
        assert_eq!(value[0]["status"], "mastered");
        assert_eq!(value[1]["status"], "ready");
        let due = value[0]["review_due_at"].clone();
        rows.push(row("a", true, 100000.));
        let value = summarize(&plans(), &rows, 100001.).unwrap();
        assert_eq!(value[0]["review_due_at"], due);
        assert_eq!(value[0]["status"], "review_due");
    }
    #[test]
    fn early_practice_does_not_postpone_due_and_fresh_review_advances_interval() {
        let mut rows = vec![
            row("a", true, 1.),
            row("b", true, 2.),
            row("c", true, 3.),
            row("early", true, 100.),
        ];
        let first = summarize(&plans(), &rows, 100.).unwrap();
        assert_eq!(first[0]["review_due_at"], 86403.);
        for (i, key) in ["d", "e", "f"].iter().enumerate() {
            rows.push(row(key, true, 86404. + i as f64));
        }
        let next = summarize(&plans(), &rows, 86407.).unwrap();
        assert_eq!(next[0]["review_stage"], 2);
        assert_eq!(next[0]["review_due_at"], 86406. + 3. * 86400.);
        assert_eq!(next[0]["round_correct"], 0);
    }
    #[test]
    fn revealed_corrections_are_practice_and_repeated_wrong_answers_cannot_push_due() {
        let mut rows = vec![row("a", false, 1.), row("a", true, 2.), row("b", true, 3.)];
        let value = summarize(&plans(), &rows, 4.).unwrap();
        assert_eq!(value[0]["correct_questions"], 1);
        assert_eq!(value[0]["round_correct"], 1);
        assert_eq!(value[0]["practice_correct_questions"], 2);
        rows.extend([row("c", true, 5.), row("d", true, 6.), row("b", false, 10.)]);
        let value = summarize(&plans(), &rows, 11.).unwrap();
        assert_eq!(value[0]["status"], "review_scheduled");
        assert_eq!(value[0]["review_due_at"], 610.);
        rows.push(row("b", false, 1000.));
        rows.push(row("b", true, 1001.));
        let next = summarize(&plans(), &rows, 1002.).unwrap();
        assert_eq!(next[0]["review_due_at"], 610.);
        assert_eq!(next[0]["review_stage"], 1);
        assert_eq!(next[0]["status"], "review_due");
        assert_eq!(next[1]["status"], "ready");
    }
}
