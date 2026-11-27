//! The repo-scoped presence lock (`.ralphy/run.lock`) as `triage` reads it.

use std::path::Path;

use crate::runlock::{self, LockState};

/// What the repo-scoped presence lock (`.ralphy/run.lock`) means for this
/// invocation of `triage`, given `--if-idle` and the lock's inspected state.
#[derive(Debug)]
pub(super) enum PresenceGate {
    /// Proceed, optionally warning first.
    Proceed { warn: Option<String> },
    /// Exit 0 with this message instead of proceeding.
    Defer(String),
}

/// Classify the repo-scoped presence lock into what `triage::run` should do.
/// Pure over an injected liveness predicate so it unit-tests without a real
/// second process (mirrors `runlock::inspect`'s own test pattern).
pub(super) fn presence_gate(
    path: &Path,
    if_idle: bool,
    is_alive: impl Fn(u32) -> bool,
) -> PresenceGate {
    match runlock::inspect(path, is_alive) {
        LockState::HeldAlive(info) if if_idle => PresenceGate::Defer(format!(
            "skipped: run in progress since {}, pid {}",
            runlock::since_text(&info.started_at),
            info.pid
        )),
        LockState::HeldAlive(info) => PresenceGate::Proceed {
            warn: Some(format!(
                "a run is already active in this repo — proceeding anyway (pid {})",
                info.pid
            )),
        },
        LockState::Stale(info) => PresenceGate::Proceed {
            warn: Some(format!(
                "ignoring stale run.lock (pid {} not running)",
                info.pid
            )),
        },
        LockState::Corrupt => PresenceGate::Proceed {
            warn: Some("ignoring unreadable run.lock".into()),
        },
        LockState::Free => PresenceGate::Proceed { warn: None },
    }
}

/// Print why `triage` defers, or the warning when it goes on; `false` when it
/// must stop with a success exit.
pub(super) fn pass_presence_gate(lock_path: &Path, if_idle: bool) -> bool {
    match presence_gate(lock_path, if_idle, runlock::pid_is_alive) {
        PresenceGate::Defer(msg) => {
            outln!("{msg}");
            false
        }
        PresenceGate::Proceed { warn } => {
            if let Some(w) = warn {
                eoutln!("{w}");
            }
            true
        }
    }
}
