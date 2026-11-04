//! Opt-in, bounded diagnostics. No prompts, output, credentials or request bodies.
use serde::Serialize;
use serde_json::{Value, json};
use std::{collections::VecDeque, sync::Mutex, time::Instant};

const LIMIT: usize = 256;

#[derive(Serialize)]
struct RequestSample {
    request_id: String,
    trace_id: String,
    http_status: Option<u16>,
}

#[derive(Serialize)]
struct TurnSample {
    turn_id: String,
    status: &'static str,
    #[serde(skip)]
    started: Instant,
    admission_wait_ms: f64,
    admitted_ms: f64,
    provider_started_ms: Option<f64>,
    requests: Vec<RequestSample>,
    first_content_consumed_ms: Option<f64>,
    first_content_committed_ms: Option<f64>,
    first_content_sent_ms: Option<f64>,
    finished_ms: Option<f64>,
    delta_queue_high_water: usize,
    batches: u64,
    events_committed: u64,
    batch_wall_ms: f64,
    writer_mutex_wait_ms: f64,
    sqlite_begin_ms: f64,
    write_commit_ms: f64,
}

#[derive(Default)]
struct Samples {
    turns: VecDeque<TurnSample>,
    evicted: u64,
}
pub struct RuntimeMetrics {
    enabled: bool,
    samples: Mutex<Samples>,
}
impl RuntimeMetrics {
    pub fn new(enabled: bool) -> Self {
        Self {
            enabled,
            samples: Mutex::new(Samples::default()),
        }
    }
    pub fn admitted(&self, turn: &str, started: Instant, wait: f64) {
        if !self.enabled {
            return;
        }
        if let Ok(mut samples) = self.samples.lock() {
            if samples.turns.len() == LIMIT {
                samples.turns.pop_front();
                samples.evicted += 1;
            }
            samples.turns.push_back(TurnSample {
                turn_id: turn.into(),
                status: "running",
                started,
                admission_wait_ms: wait,
                admitted_ms: elapsed(started),
                provider_started_ms: None,
                requests: Vec::new(),
                first_content_consumed_ms: None,
                first_content_committed_ms: None,
                first_content_sent_ms: None,
                finished_ms: None,
                delta_queue_high_water: 0,
                batches: 0,
                events_committed: 0,
                batch_wall_ms: 0.0,
                writer_mutex_wait_ms: 0.0,
                sqlite_begin_ms: 0.0,
                write_commit_ms: 0.0,
            });
        }
    }
    fn update(&self, turn: &str, f: impl FnOnce(&mut TurnSample)) {
        if !self.enabled {
            return;
        }
        if let Ok(mut samples) = self.samples.lock()
            && let Some(sample) = samples.turns.iter_mut().rev().find(|s| s.turn_id == turn)
        {
            f(sample);
        }
    }
    pub fn waiting(&self, turn: &str) {
        self.update(turn, |s| s.status = "waiting_input");
    }
    pub fn provider_started(&self, turn: &str) {
        self.update(turn, |s| s.status = "running");
        self.update(turn, |s| s.provider_started_ms = Some(elapsed(s.started)));
    }
    pub(crate) fn request(
        &self,
        turn: &str,
        request_id: &str,
        trace_id: &str,
        http_status: Option<u16>,
    ) {
        self.update(turn, |s| {
            if let Some(request) = s.requests.iter_mut().find(|r| r.request_id == request_id) {
                request.http_status = http_status;
            } else if s.requests.len() < 32 {
                s.requests.push(RequestSample {
                    request_id: request_id.into(),
                    trace_id: trace_id.into(),
                    http_status,
                });
            }
        });
    }
    pub fn delta(&self, turn: &str, content: bool, queued: usize) {
        self.update(turn, |s| {
            s.delta_queue_high_water = s.delta_queue_high_water.max(queued);
            if content && s.first_content_consumed_ms.is_none() {
                s.first_content_consumed_ms = Some(elapsed(s.started));
            }
        });
    }
    pub fn batch(
        &self,
        turn: &str,
        count: usize,
        content: bool,
        wall: f64,
        timing: crate::store::EventWriteTiming,
    ) {
        self.update(turn, |s| {
            s.batches += 1;
            s.events_committed += count as u64;
            s.batch_wall_ms += wall;
            s.writer_mutex_wait_ms += timing.writer_mutex_wait_ms;
            s.sqlite_begin_ms += timing.sqlite_begin_ms;
            s.write_commit_ms += timing.write_commit_ms;
            if content && s.first_content_committed_ms.is_none() {
                s.first_content_committed_ms = timing
                    .committed_at
                    .map(|at| at.duration_since(s.started).as_secs_f64() * 1000.0);
            }
        });
    }
    pub fn sent(&self, turn: &str) {
        self.update(turn, |s| {
            if s.first_content_sent_ms.is_none() {
                s.first_content_sent_ms = Some(elapsed(s.started));
            }
        });
    }
    pub fn finished(&self, turn: &str, status: &'static str) {
        self.update(turn, |s| {
            s.status = status;
            s.finished_ms = Some(elapsed(s.started));
        });
    }
    pub fn snapshot(&self) -> Value {
        match self.samples.lock() {
            Ok(samples) => {
                json!({"enabled":self.enabled,"capacity":LIMIT,"evicted":samples.evicted,"turns":samples.turns})
            }
            Err(_) => json!({"enabled":self.enabled,"unavailable":true}),
        }
    }
}
pub fn elapsed(start: Instant) -> f64 {
    start.elapsed().as_secs_f64() * 1000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_diagnostics_correlate_attempts_without_duplicating_or_exceeding_limits() {
        let metrics = RuntimeMetrics::new(true);
        metrics.admitted("turn", Instant::now(), 0.0);
        metrics.request("turn", "request-0", "trace", None);
        metrics.request("turn", "request-0", "trace", Some(429));
        for i in 1..40 {
            metrics.request("turn", &format!("request-{i}"), "trace", Some(200));
        }
        let snapshot = metrics.snapshot();
        let requests = snapshot["turns"][0]["requests"].as_array().unwrap();
        assert_eq!(requests.len(), 32);
        assert_eq!(requests[0]["http_status"], 429);
        assert_eq!(requests[1]["http_status"], 200);
        assert_eq!(requests[0].as_object().unwrap().len(), 3);
        let disabled = RuntimeMetrics::new(false);
        disabled.admitted("turn", Instant::now(), 0.0);
        disabled.request("turn", "request", "trace", Some(200));
        assert!(disabled.snapshot()["turns"].as_array().unwrap().is_empty());
    }
}
