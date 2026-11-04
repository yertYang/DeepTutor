//! Turn-local plan mutations. A mutation becomes visible through the durable
//! tool result acknowledgement, and is included in any later pause checkpoint.
use crate::{Error, Result, tools::ToolCall};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub const MAX_REPLANS: usize = 2;
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct SolveState {
    pub analysis: String,
    pub steps: Vec<Step>,
    pub replans: usize,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Step {
    id: String,
    goal: String,
    done: bool,
    summary: String,
}
pub fn definitions() -> Vec<Value> {
    let steps = json!({"type":"array","minItems":1,"maxItems":12,"items":{"type":"object","properties":{"goal":{"type":"string","maxLength":240}},"required":["goal"],"additionalProperties":false}});
    vec![
        json!({"name":"solve_plan","description":"Commit the problem analysis and ordered plan before working. Usually 2-6 steps. Steps receive server-generated IDs.","parameters":{"type":"object","properties":{"analysis":{"type":"string","maxLength":1200},"steps":steps},"required":["analysis","steps"],"additionalProperties":false}}),
        json!({"name":"solve_finish_step","description":"Record the current step's established result and advance to the next step. Work steps in order.","parameters":{"type":"object","properties":{"step_id":{"type":"string"},"summary":{"type":"string","maxLength":1600}},"required":["step_id","summary"],"additionalProperties":false}}),
        json!({"name":"solve_replan","description":"Replace a failed approach with a new plan and explain why. At most two replans per turn.","parameters":{"type":"object","properties":{"reason":{"type":"string","maxLength":1200},"steps":steps},"required":["reason","steps"],"additionalProperties":false}}),
    ]
}
fn bounded(input: &Value, key: &str, max: usize) -> Result<String> {
    let value = input[key]
        .as_str()
        .filter(|s| !s.trim().is_empty() && s.chars().count() <= max)
        .ok_or_else(|| {
            Error::invalid(format!(
                "A nonempty {key} within {max} characters is required."
            ))
        })?;
    Ok(value.trim().into())
}
fn steps(input: &Value) -> Result<Vec<Step>> {
    let source = input["steps"]
        .as_array()
        .filter(|s| !s.is_empty() && s.len() <= 12)
        .ok_or_else(|| Error::invalid("A solve plan requires 1-12 steps."))?;
    source
        .iter()
        .enumerate()
        .map(|(index, step)| {
            Ok(Step {
                id: format!("S{}", index + 1),
                goal: bounded(step, "goal", 240)?,
                done: false,
                summary: String::new(),
            })
        })
        .collect()
}
impl SolveState {
    pub fn all_done(&self) -> bool {
        !self.steps.is_empty() && self.steps.iter().all(|s| s.done)
    }
    fn next(&self) -> Option<&Step> {
        self.steps.iter().find(|s| !s.done)
    }
    pub fn execute(&mut self, call: &ToolCall) -> Value {
        match self.apply(call) {
            Ok((status, completed)) => {
                let mut payload = json!({"status":status,"analysis":self.analysis,"steps":self.steps,"next":self.next(),"completed":completed,"all_done":self.all_done(),"replans_used":self.replans,"replans_max":MAX_REPLANS});
                if call.name == "solve_replan" {
                    payload["reason"] = json!(self.analysis);
                }
                let content = if call.name == "solve_finish_step" {
                    json!({"status":status,"completed":completed,"summary":completed.as_ref().and_then(|id|self.steps.iter().find(|step|&step.id==id).map(|step|&step.summary)),"next":self.next(),"all_done":self.all_done()})
                } else {
                    payload.clone()
                };
                let mut result = json!({"success":true,"content":content});
                result[call.name.as_str()] = payload;
                result
            }
            Err(error) => json!({"success":false,"error":error.code,"content":error.message}),
        }
    }
    fn apply(&mut self, call: &ToolCall) -> Result<(&'static str, Option<String>)> {
        let input: Value = serde_json::from_str(&call.arguments)
            .map_err(|_| Error::invalid("Solve tools require valid JSON arguments."))?;
        match call.name.as_str() {
            "solve_plan" => {
                if !self.steps.is_empty() {
                    return Err(Error::conflict(
                        "A plan already exists. Continue it or use solve_replan.",
                    ));
                }
                let analysis = bounded(&input, "analysis", 1200)?;
                let steps = steps(&input)?;
                self.analysis = analysis;
                self.steps = steps;
                Ok(("planned", None))
            }
            "solve_replan" => {
                if self.steps.is_empty() {
                    return Err(Error::conflict("Create the initial plan with solve_plan."));
                }
                if self.replans >= MAX_REPLANS {
                    return Err(Error::conflict(
                        "Replan budget is exhausted. Continue the current plan.",
                    ));
                }
                let analysis = bounded(&input, "reason", 1200)?;
                let steps = steps(&input)?;
                self.analysis = analysis;
                self.steps = steps;
                self.replans += 1;
                Ok(("replanned", None))
            }
            "solve_finish_step" => {
                let id = bounded(&input, "step_id", 32)?;
                let summary = bounded(&input, "summary", 1600)?;
                let step = self
                    .steps
                    .iter()
                    .find(|s| s.id == id)
                    .ok_or_else(|| Error::invalid("Unknown solve step ID."))?;
                if step.done {
                    if step.summary != summary {
                        return Err(Error::conflict(
                            "A completed step's summary cannot be replaced.",
                        ));
                    }
                    return Ok(("already_completed", Some(id)));
                }
                if self.next().is_none_or(|s| s.id != id) {
                    return Err(Error::conflict(
                        "Finish the current solve step before advancing.",
                    ));
                }
                let step = self.steps.iter_mut().find(|s| s.id == id).unwrap();
                step.done = true;
                step.summary = summary;
                Ok(("step_done", Some(id)))
            }
            _ => Err(Error::unsupported("This solve tool is not available.")),
        }
    }
}
