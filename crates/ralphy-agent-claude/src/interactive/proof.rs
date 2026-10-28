//! The proof wait of an interactive session: the gate that holds the
//! charter of one child until its first hook event, and the keys that type
//! the charter (ADR-0072 D6).

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::EXEC_CHARTER;

/// The keys that submit the charter in the TUI: the text in one write, a
/// pause, then `\r` in a second write. M6 of
/// `docs/research/claude-hook-probes-2026-10-28.md` (Claude Code 2.1.312):
/// text + `\r` in one write submitted 0 of 3; a pause of 100 ms 3 of 3,
/// 30 ms 0 of 1, 300 ms 6 of 6.
pub(super) struct CharterKeys {
    pub(super) text: &'static [u8],
    pub(super) pause: Duration,
    pub(super) submit: &'static [u8],
}

pub(super) fn charter_keys() -> CharterKeys {
    CharterKeys {
        text: EXEC_CHARTER.as_bytes(),
        pause: Duration::from_millis(300),
        submit: b"\r",
    }
}

/// What the pump does with the charter on one turn.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum GateAction {
    Wait,
    Type,
    Refuse,
}

/// Where a gate stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GateState {
    Waiting,
    Typed,
    Refused,
}

/// The rule that holds the charter of one child until its first hook event.
/// Pure, so it unit-tests without a PTY.
pub(super) struct ProofGate {
    deadline: Instant,
    state: GateState,
}

impl ProofGate {
    pub(super) fn new(now: Instant, window: Duration) -> Self {
        Self {
            deadline: now + window,
            state: GateState::Waiting,
        }
    }

    pub(super) fn waiting(&self) -> bool {
        self.state == GateState::Waiting
    }

    /// `Type` once, on the first poll that saw the line before the deadline;
    /// `Refuse` from the deadline on when no line came.
    pub(super) fn poll(&mut self, line_seen: bool, now: Instant) -> GateAction {
        match self.state {
            GateState::Typed => GateAction::Wait,
            GateState::Refused => GateAction::Refuse,
            GateState::Waiting if line_seen => {
                self.state = GateState::Typed;
                GateAction::Type
            }
            GateState::Waiting if now >= self.deadline => {
                self.state = GateState::Refused;
                GateAction::Refuse
            }
            GateState::Waiting => GateAction::Wait,
        }
    }
}

/// The proof wait of one child: its gate, the status file it reads from
/// byte `from` on, and the second write of the charter still to come.
pub(super) struct ProofWait {
    pub(super) gate: ProofGate,
    pub(super) status_file: PathBuf,
    pub(super) from: u64,
    pub(super) settings: PathBuf,
    pub(super) project: PathBuf,
    pub(super) window: Duration,
    pub(super) submit_at: Option<Instant>,
}

impl ProofWait {
    pub(super) fn new(
        status_file: PathBuf,
        from: u64,
        settings: PathBuf,
        project: PathBuf,
        window: Duration,
    ) -> Self {
        Self {
            gate: ProofGate::new(Instant::now(), window),
            status_file,
            from,
            settings,
            project,
            window,
            submit_at: None,
        }
    }
}

/// The length of the status file now: a new child reads only what comes after.
pub(super) fn status_len(path: &Path) -> u64 {
    std::fs::metadata(path).map_or(0, |m| m.len())
}
