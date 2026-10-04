//! A launch's claim on its window record (ADR-0050 amendment 2026-10-04): one
//! launch at a time per record, so two pages that load one desk start each
//! console once.

use std::sync::Arc;

use super::SessionManager;
use crate::session::SessionId;

/// A launch's hold on one window record, from the check for a live session to
/// the insert of the new one: a second launch for the same record waits here,
/// then finds the session the first one started. Async because the agent
/// launch awaits between the check and the spawn.
pub struct RecordClaim {
    manager: Arc<SessionManager>,
    record: String,
    gate: Arc<tokio::sync::Mutex<()>>,
    guard: Option<tokio::sync::OwnedMutexGuard<()>>,
}

impl RecordClaim {
    pub fn record(&self) -> &str {
        &self.record
    }

    /// The live session that already serves this record, if any.
    pub fn live(&self) -> Option<SessionId> {
        self.manager
            .sessions
            .lock()
            .expect("sessions mutex")
            .values()
            .find(|s| s.info.record.as_deref() == Some(self.record.as_str()))
            .map(|s| s.info.id)
    }
}

impl Drop for RecordClaim {
    fn drop(&mut self) {
        self.guard.take();
        // Under the map lock, so no launch can clone the gate between the
        // count and the removal: two references are the map's and this one.
        let mut records = self.manager.records.lock().expect("records mutex");
        if Arc::strong_count(&self.gate) == 2 {
            records.remove(&self.record);
        }
    }
}

impl SessionManager {
    /// Hold `record` until the returned claim is dropped. A launch that names
    /// its record takes this BEFORE it looks for a live session, and keeps it
    /// until the session it spawns is in the list.
    pub async fn claim_record(self: &Arc<Self>, record: &str) -> RecordClaim {
        let gate = self
            .records
            .lock()
            .expect("records mutex")
            .entry(record.to_string())
            .or_default()
            .clone();
        let guard = gate.clone().lock_owned().await;
        RecordClaim {
            manager: self.clone(),
            record: record.to_string(),
            gate,
            guard: Some(guard),
        }
    }
}
