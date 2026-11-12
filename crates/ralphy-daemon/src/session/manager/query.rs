//! The read-only questions to the session table: list, find and look up.

use std::time::SystemTime;

use super::{SessionId, SessionInfo, SessionManager};

impl SessionManager {
    /// The live sessions, ordered by id (the `BTreeMap` key order).
    pub fn list(&self) -> Vec<SessionInfo> {
        self.list_at(SystemTime::now())
    }

    /// [`list`](Self::list) with the staleness clock supplied (ADR-0059 §6):
    /// what `/api/sessions` would say if it were asked at `now`.
    ///
    /// # Panics
    ///
    /// When the sessions lock was poisoned by a panic in another thread.
    pub fn list_at(&self, now: SystemTime) -> Vec<SessionInfo> {
        self.sessions
            .lock()
            .expect("sessions mutex")
            .values()
            .map(|s| s.info_at(now))
            .collect()
    }

    /// Whether a live session of `repo` was spawned in the worktree `checkout`
    /// — the daemon's own `worktree.remove` gate (ADR-0063 §2): the CLI cannot
    /// see this table. A free console carries `checkout: None` and never matches.
    ///
    /// # Panics
    ///
    /// When the sessions lock was poisoned by a panic in another thread.
    pub fn console_in(&self, repo: &str, checkout: &str) -> bool {
        self.sessions
            .lock()
            .expect("sessions mutex")
            .values()
            .any(|s| s.info.repo == repo && s.info.checkout.as_deref() == Some(checkout))
    }

    /// A single session's identity, or `None` if it is not (or no longer) live.
    ///
    /// # Panics
    ///
    /// When the sessions lock was poisoned by a panic in another thread.
    pub fn get(&self, id: SessionId) -> Option<SessionInfo> {
        self.sessions
            .lock()
            .expect("sessions mutex")
            .get(&id)
            .map(|s| s.info_at(SystemTime::now()))
    }
}
