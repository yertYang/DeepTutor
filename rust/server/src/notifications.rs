//! Per-turn wakeups; durable SQLite events remain the source of truth.
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, Weak},
};
use tokio::sync::Notify;

#[derive(Default)]
pub(crate) struct TurnChanges(Mutex<HashMap<String, Weak<Notify>>>);

impl TurnChanges {
    pub fn subscribe(&self, turn: &str) -> Arc<Notify> {
        let mut turns = self.0.lock().unwrap_or_else(|e| e.into_inner());
        // Keep only live subscribers; completed turns must not accumulate here.
        turns.retain(|_, signal| signal.strong_count() > 0);
        if let Some(signal) = turns.get(turn).and_then(Weak::upgrade) {
            return signal;
        }
        let signal = Arc::new(Notify::new());
        turns.insert(turn.to_owned(), Arc::downgrade(&signal));
        signal
    }

    pub fn notify(&self, turn: &str) {
        let turns = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(signal) = turns.get(turn).and_then(Weak::upgrade) {
            signal.notify_waiters();
        }
    }
}
