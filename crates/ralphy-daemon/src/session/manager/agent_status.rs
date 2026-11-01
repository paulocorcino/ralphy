//! The agent state a console shows: the hook lines folded at each tick,
//! the operator's input, and the silence rule over them (docs/adr/0059 §5,
//! A2).

use std::time::SystemTime;

use super::SessionManager;
use crate::agent_state::fold::Cause;
use crate::agent_state::fold::Fold;
use crate::agent_state::{AgentState, HookVendor, Observed};
use crate::session::SessionId;

/// The turns a console finished, as the hooks showed them: how many, and the
/// hook time of the last one.
#[derive(Debug, Default)]
struct TurnEnds {
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
                let status = s.agent_status.lock().expect("agent_status mutex");
                status.turn_ends().map(|(count, since)| TurnEnd {
                    id: s.info.id,
                    record: s.info.record.clone(),
                    count,
                    since: since.to_owned(),
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

/// Why a console's state is not reported, the reason in words, and when it
/// began.
type Unreported = (crate::agent_state::Unreported, String, SystemTime);

/// The agent state of one console, and everything the silence rule and the
/// turn count read: one value, so one lock holds it.
///
/// The one rule of its lock: read the status file's lines first, with no
/// lock held, then lock this and fold them with [`on_tick`](Self::on_tick).
/// An input folds under the same lock, so a tick and an input never store
/// over each other (ADR-0085 D7.5). This type reads no file.
pub(super) struct AgentStatus {
    id: SessionId,
    /// The vendor of the hook lines and their fold; `None` for a console
    /// without hooks.
    hooks: Option<(HookVendor, Fold)>,
    /// The last observed state, `None` until the first hook fires.
    observed: Option<Observed>,
    /// The state word and detail the list showed at the last tick. A
    /// `working` that ages into `unknown` changes what is shown with no new
    /// observation, so the change is measured against this, not against the
    /// observation.
    shown: Option<AgentState>,
    /// The time of the operator's first input with at least one byte: the
    /// start of the silence clock (docs/adr/0059 A2). Later inputs do not
    /// move it.
    first_input: Option<SystemTime>,
    /// When a tick first folded a hook line; once set, the console is never
    /// silent again.
    first_line: Option<SystemTime>,
    /// Why the state is not reported, while it lasts.
    silent: Option<Unreported>,
    /// Whether this console already logged that it became silent.
    logged_silent: bool,
    /// The turns this console finished (docs/adr/0085 D11).
    turn_ends: TurnEnds,
}

impl AgentStatus {
    /// The state of console `id`: folded from the lines of `vendor` when it
    /// has hooks, and not reported from its start when `unreported` gives
    /// the reason and the time.
    pub(super) fn new(
        id: SessionId,
        vendor: Option<HookVendor>,
        unreported: Option<(String, SystemTime)>,
    ) -> Self {
        AgentStatus {
            id,
            hooks: vendor.map(|v| (v, Fold::new())),
            observed: None,
            shown: None,
            first_input: None,
            first_line: None,
            silent: unreported
                .map(|(reason, since)| (crate::agent_state::Unreported::NoSource, reason, since)),
            logged_silent: false,
            turn_ends: TurnEnds::default(),
        }
    }

    /// The operator wrote to the console at `now`: the silence clock starts,
    /// and the permission `waiting` of a vendor that sends no grant event
    /// ends (ADR-0085 D7.5). The next tick reports the change.
    pub(super) fn on_input(&mut self, now: SystemTime) {
        self.first_input.get_or_insert(now);
        if let Some(obs) = self.hooks.as_mut().and_then(|(v, f)| v.fold_input(f, now)) {
            self.observed = Some(obs);
        }
    }

    /// Fold the `lines` a tick read at `now`: keep the newest transition,
    /// and on ANY folded line refresh the observation's `seen` — a `working`
    /// that keeps producing `PreToolUse` lines is alive, and the staleness
    /// clock must not age it into `unknown` (§6). Then apply the silence
    /// rule; `start_failure` counts only for a vendor whose hook runs the
    /// ralphy program.
    ///
    /// Returns whether the state the list shows changed since the last tick:
    /// its word or its detail, including a `working` that ages into `unknown`
    /// (§6) with no new line, and the start or the end of a silence. A
    /// refreshed `seen` alone does not count. A console without hooks never
    /// changes.
    pub(super) fn on_tick(
        &mut self,
        lines: &[String],
        now: SystemTime,
        start_failure: Option<&str>,
    ) -> bool {
        let Some((vendor, fold)) = self.hooks.as_mut() else {
            return false;
        };
        let start_failure = start_failure.filter(|_| vendor.runs_the_ralphy_program());
        let polled = vendor.fold_lines(fold, lines, now);
        if polled.activity {
            self.first_line.get_or_insert(now);
        }
        self.count_turn_ends(&polled.transitions);
        if let Some(last) = polled.transitions.into_iter().last() {
            self.observed = Some(last);
        } else if polled.activity {
            if let Some(obs) = self.observed.as_mut() {
                obs.seen = now;
            }
        }
        self.update_silence(now, start_failure);
        let shown_now = self.shown_at(now);
        let changed = self.shown != shown_now;
        self.shown = shown_now;
        changed
    }

    /// The agent block the list shows at `now`: the unreported block while
    /// the state is not reported, else the observation rendered with §6
    /// staleness.
    pub(super) fn shown_at(&self, now: SystemTime) -> Option<AgentState> {
        if let Some((why, reason, since)) = self.silent.clone() {
            return Some(AgentState::unreported(why, reason, since));
        }
        self.observed
            .as_ref()
            .map(|o| crate::agent_state::render(o, now))
    }

    /// How many turns this console finished, and the hook time of the last
    /// one; `None` before the first.
    pub(super) fn turn_ends(&self) -> Option<(u64, &str)> {
        let ends = &self.turn_ends;
        (ends.count > 0).then_some((ends.count, ends.since.as_str()))
    }

    /// Apply the silence rule at `now` (docs/adr/0059 A2) and log one line
    /// the first time this console becomes silent. Only a console with hooks
    /// calls this.
    fn update_silence(&mut self, now: SystemTime, start_failure: Option<&str>) {
        let reason = crate::agent_state::silence(
            crate::agent_state::StateSource::Hooks,
            self.first_input,
            self.first_line,
            start_failure,
            now,
        );
        match (reason, self.silent.is_some()) {
            (Some(reason), false) => {
                if !std::mem::replace(&mut self.logged_silent, true) {
                    tracing::warn!(session = self.id, "a console became silent: {reason}");
                }
                self.silent = Some((crate::agent_state::Unreported::Silent, reason, now));
            }
            (None, true) => self.silent = None,
            (Some(_), true) | (None, false) => {}
        }
    }

    /// Add the turns `transitions` ended to this console's count.
    fn count_turn_ends(&mut self, transitions: &[Observed]) {
        let was = self.observed.as_ref().map(|o| o.state);
        let (ended, at) = turn_ends_in(was, transitions);
        if let Some(at) = at {
            self.turn_ends.count += ended;
            self.turn_ends.since = at;
        }
    }
}
