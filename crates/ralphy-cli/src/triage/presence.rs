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

/// Format an RFC 3339 timestamp as `%Y-%m-%d %H:%M:%S`, falling back to the raw
/// string on parse failure (mirrors `run_cmd`'s formatting, run.rs L76-78).
fn format_since(started_at: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(started_at).map_or_else(
        |_| started_at.to_owned(),
        |t| t.format("%Y-%m-%d %H:%M:%S").to_string(),
    )
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
            format_since(&info.started_at),
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
            println!("{msg}");
            false
        }
        PresenceGate::Proceed { warn } => {
            if let Some(w) = warn {
                eprintln!("{w}");
            }
            true
        }
    }
}
