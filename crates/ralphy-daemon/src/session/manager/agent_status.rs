//! The agent state a console shows: the hook tail read at each tick, and
//! the silence rule over it (docs/adr/0059 §5, A2).

use std::sync::atomic::Ordering;
use std::time::SystemTime;

use super::{ManagedSession, SessionManager};
use crate::agent_state::fold::Cause;
use crate::agent_state::Observed;
use crate::session::SessionId;

/// The turns a console finished, as the hooks showed them: how many, and the
/// hook time of the last one.
#[derive(Debug, Default)]
pub(super) struct TurnEnds {
    count: u64,
    since: String,
}

/// A console that finished at least one turn since it started
/// (docs/adr/0085 D11). `count` only grows, so a reader that kept the last
/// count it saw knows a new turn ended when the count is higher.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnEnd {
    pub id: SessionId,
    /// The window record the launch named.
    pub record: Option<String>,
    pub count: u64,
    /// The hook time of the last finished turn.
    pub since: String,
}

impl SessionManager {
    /// The consoles that finished a turn, in session order.
    pub fn turn_ends(&self) -> Vec<TurnEnd> {
        let sessions = self.sessions.lock().expect("sessions mutex");
        sessions
            .values()
            .filter_map(|s| {
                let ends = s.turn_ends.lock().expect("turn_ends mutex");
                (ends.count > 0).then(|| TurnEnd {
                    id: s.info.id,
                    record: s.info.record.clone(),
                    count: ends.count,
                    since: ends.since.clone(),
                })
            })
            .collect()
    }
}

/// How many of `transitions` end a turn: a `done` caused by the turn end
/// (never by a session start) right after a `working`. `was` is the state
/// shown before the first one; the hook time of the last such one is
/// returned with the count.
fn turn_ends_in(was: Option<&str>, transitions: &[Observed]) -> (u64, Option<String>) {
    let mut was = was;
    let (mut count, mut since) = (0, None);
    for t in transitions {
        if t.state == "done" && t.cause == Some(Cause::TurnEnd) && was == Some("working") {
            count += 1;
            since = Some(t.since.clone());
        }
        was = Some(t.state);
    }
    (count, since)
}

impl ManagedSession {
    /// The agent block the list shows at `now`: the unreported block while
    /// the state is not reported, else the observation rendered with §6
    /// staleness.
    pub(super) fn shown_at(&self, now: SystemTime) -> Option<crate::agent_state::AgentState> {
        if let Some((why, reason, since)) = self.silent.lock().expect("silent mutex").clone() {
            return Some(crate::agent_state::AgentState::unreported(
                why, reason, since,
            ));
        }
        self.agent_state
            .lock()
            .expect("agent_state mutex")
            .as_ref()
            .map(|o| crate::agent_state::render(o, now))
    }

    /// Apply the silence rule at `now` (docs/adr/0059 A2) and log one line
    /// the first time this console becomes silent. Only a console with a
    /// status tail calls this, and it has one only when its state comes from
    /// hooks.
    fn update_silence(&self, now: SystemTime, start_failure: Option<&str>) {
        let reason = crate::agent_state::silence(
            crate::agent_state::StateSource::Hooks,
            *self.first_input.lock().expect("first_input mutex"),
            *self.first_line.lock().expect("first_line mutex"),
            start_failure,
            now,
        );
        let mut silent = self.silent.lock().expect("silent mutex");
        match (reason, silent.is_some()) {
            (Some(reason), false) => {
                if !self.logged_silent.swap(true, Ordering::Relaxed) {
                    tracing::warn!(session = self.info.id, "a console became silent: {reason}");
                }
                *silent = Some((crate::agent_state::Unreported::Silent, reason, now));
            }
            (None, true) => *silent = None,
            (Some(_), true) | (None, false) => {}
        }
    }

    /// Add the turns `transitions` ended to this console's count.
    fn count_turn_ends(&self, was: Option<&str>, transitions: &[Observed]) {
        let (ended, at) = turn_ends_in(was, transitions);
        if let Some(at) = at {
            let mut ends = self.turn_ends.lock().expect("turn_ends mutex");
            ends.count += ended;
            ends.since = at;
        }
    }

    /// Read what the hooks appended since the last tick: keep the newest
    /// transition, and on ANY folded line refresh the observation's `seen` —
    /// a `working` that keeps producing `PreToolUse` lines is alive, and the
    /// staleness clock must not age it into `unknown` (§6). Cheap when
    /// nothing changed (an open, a seek, an empty read).
    ///
    /// Returns whether the state the list shows changed since the last tick:
    /// its word or its detail, including a `working` that ages into `unknown`
    /// (§6) with no new line, and the start or the end of a silence. A
    /// refreshed `seen` alone does not count.
    pub(super) fn poll_status(&self, now: SystemTime) -> bool {
        self.poll_status_with(now, crate::agent_state::start_check_failure().as_deref())
    }

    /// [`poll_status`](Self::poll_status) with the start check's failure
    /// given, so a test does not depend on the daemon-wide check. The
    /// failure counts only for a vendor whose hook runs the ralphy program.
    pub(super) fn poll_status_with(&self, now: SystemTime, start_failure: Option<&str>) -> bool {
        self.poll_status_between(now, start_failure, || {})
    }

    /// [`poll_status_with`](Self::poll_status_with), with `between` run
    /// after the tail is read and before the state is stored, so a test can
    /// write the operator's input there.
    ///
    /// LOCK ORDER: `tail`, then `agent_state`, as in `operator_input`. The
    /// tail stays locked until the new state is stored: an input folded
    /// between the read and the store would else be overwritten by the older
    /// state the read returned (ADR-0085 D7.5).
    pub(super) fn poll_status_between(
        &self,
        now: SystemTime,
        start_failure: Option<&str>,
        between: impl FnOnce(),
    ) -> bool {
        let Some(status) = &self.status else {
            return false;
        };
        let start_failure = start_failure.filter(|_| status.files.vendor.runs_the_ralphy_program());
        let mut tail = status.tail.lock().expect("tail mutex");
        let polled = tail.poll();
        between();
        if polled.activity {
            self.first_line
                .lock()
                .expect("first_line mutex")
                .get_or_insert(now);
        }
        let mut slot = self.agent_state.lock().expect("agent_state mutex");
        self.count_turn_ends(slot.as_ref().map(|o| o.state), &polled.transitions);
        if let Some(last) = polled.transitions.into_iter().last() {
            *slot = Some(last);
        } else if polled.activity {
            if let Some(obs) = slot.as_mut() {
                obs.seen = now;
            }
        }
        drop(slot);
        drop(tail);
        self.update_silence(now, start_failure);
        let shown_now = self.shown_at(now);
        let mut shown = self.shown.lock().expect("shown mutex");
        let changed = *shown != shown_now;
        *shown = shown_now;
        changed
    }
}
