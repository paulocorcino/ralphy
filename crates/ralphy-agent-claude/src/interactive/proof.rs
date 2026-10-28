//! The proof wait of an interactive session: the gate that holds the
//! charter of one child until its first hook event, and the keys that type
//! the charter (ADR-0072 D6).

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};

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

    /// The waiver let a refused session go on: the charter is typed now and
    /// the gate asks for nothing more.
    pub(super) fn go_on(&mut self) {
        self.state = GateState::Typed;
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
    /// `claude.run_without_guard_proof_i_understand_the_risk`: at the end of
    /// the window with no event, type the charter instead of refusing.
    pub(super) waiver: bool,
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
            waiver: false,
        }
    }

    /// Type the charter once the child's first hook event is in the status
    /// file, the submit key on a later turn; when the window ends with no
    /// event, refuse the session, or type the charter with the waiver.
    pub(super) fn advance(
        &mut self,
        now: Instant,
        keys: &mut dyn FnMut(&[u8]) -> Result<()>,
    ) -> Result<()> {
        let charter = charter_keys();
        if self.submit_at.is_some_and(|at| now >= at) {
            self.submit_at = None;
            keys(charter.submit).context("typing the charter's submit key")?;
        }
        let seen =
            self.gate.waiting() && crate::status::session_started(&self.status_file, self.from);
        match self.gate.poll(seen, now) {
            GateAction::Wait => return Ok(()),
            GateAction::Type => {}
            GateAction::Refuse => {
                let cause =
                    crate::status::no_session_start(&self.settings, &self.project, self.window);
                crate::status::refuse_or_go_on(cause, self.waiver)?;
                self.gate.go_on();
            }
        }
        keys(charter.text).context("typing the charter")?;
        self.submit_at = Some(now + charter.pause);
        Ok(())
    }
}

/// The length of the status file now: a new child reads only what comes after.
pub(super) fn status_len(path: &Path) -> u64 {
    std::fs::metadata(path).map_or(0, |m| m.len())
}
