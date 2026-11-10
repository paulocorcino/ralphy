//! The pure fold/state machine: [`RunState`] and its [`RunState::apply`] fold over
//! the semantic [`RunEvent`] stream (ADR-0007 D6).

use super::event::RunEvent;
use super::{SkipKind, UsageLite};

mod queries;
mod types;
pub(crate) use types::{AgentState, Counts, IssueEntry, IssueStatus, QueueRef, SleepState};

/// The transport-agnostic state of a run, folded from its event stream.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct RunState {
    /// The card title (set by the caller, not from events).
    pub title: String,
    /// The queue size from `queue built`.
    pub total: usize,
    /// The queue's issue numbers in working order, from `queue built`: the source
    /// of the console's queue bar.
    pub order: Vec<u64>,
    /// The first issue carrying `stop-before`: the run halts before it.
    pub stop_before: Option<u64>,
    /// The issues that have entered the lifecycle, in the order first seen.
    pub issues: Vec<IssueEntry>,
    /// The current/active issue number (the "phase" pointer): its [`IssueStatus`]
    /// is the run's current phase.
    pub active: Option<u64>,
    /// The terminal summary, set when the run stops non-green or on the deadline.
    pub final_summary: Option<String>,
    /// The active usage-limit sleep, if the run is currently waiting for a reset.
    pub sleep: Option<SleepState>,
    /// The active child is in a sustained API-degraded state (#149): set on
    /// `ApiDegraded`, cleared on `ApiRecovered`.
    pub degraded: bool,
    /// The agent's state as its hooks last reported it (ADR-0059): set on
    /// `AgentState`, cleared by every event that ends the child (a state belongs
    /// to the child that produced it).
    pub agent: Option<AgentState>,
    /// Whether the run has reached its terminal state; the worker sets it just
    /// before the final card render so the card grows its `🏁` footer (ADR-0007 D3).
    pub finished: bool,
    /// Live: the end-of-run knowledge consolidation covers this many loose notes;
    /// cleared on completion and hidden once the run is `finished`.
    pub consolidating: Option<u64>,
    /// Terminal: notes folded into `KNOWLEDGE.md` by the end-of-run consolidation,
    /// surfaced as a `📚` segment in the card footer.
    pub consolidated: Option<u64>,
    /// The run's planning agent name (from `run.started`), the default identity for
    /// the `data.agent` block on planning-phase events (ADR-0019 amendment #96).
    pub plan_agent: String,
    /// The run's executing agent name (from `run.started`), the default identity for
    /// the `data.agent` block on executing-phase and pre-phase events.
    pub exec_agent: String,
    /// The current phase's agent name — set to [`plan_agent`](Self::plan_agent) on a
    /// `Planning` fold and [`exec_agent`](Self::exec_agent) on an `Executing` fold;
    /// `None` before any phase begins (the block then falls back to `exec_agent`).
    pub cur_agent: Option<String>,
    /// The current phase's model, `None` before a phase begins.
    pub cur_model: Option<String>,
    /// The current phase's reasoning effort, `None` before a phase begins.
    pub cur_effort: Option<String>,
    /// The light queue scope (`[{number, title}]`) seeded from the enriched
    /// `queue.built` snapshot — the source for `run.started.queue` and the title
    /// fallback for the `run.finished.issues` rollup. Kept separate from
    /// [`issues`](Self::issues) so the Telegram card fold is undisturbed (#96).
    pub queue: Vec<QueueRef>,
}

impl RunState {
    /// A fresh state with a known title and queue size (the worker seeds these
    /// since the card is sent before the first folded event).
    pub(crate) fn new(title: impl Into<String>, total: usize) -> Self {
        RunState {
            title: title.into(),
            total,
            ..Default::default()
        }
    }

    /// Resolve a possibly-zero issue number (the adapter's execution events carry
    /// no number) to the active issue. `None` when it is zero and there is no
    /// active issue — e.g. an `IssueStarted` was dropped under back-pressure — so
    /// callers skip rather than materialize a phantom issue `#0`.
    fn resolve(&self, number: u64) -> Option<u64> {
        if number == 0 {
            self.active
        } else {
            Some(number)
        }
    }

    /// Get the entry for `number`, inserting a planning placeholder if unseen.
    fn entry_mut(&mut self, number: u64) -> &mut IssueEntry {
        if let Some(pos) = self.issues.iter().position(|e| e.number == number) {
            &mut self.issues[pos]
        } else {
            self.issues.push(IssueEntry {
                number,
                title: String::new(),
                status: IssueStatus::Planning,
                kind: None,
                blocked_by: Vec::new(),
                model: None,
                effort: None,
                budget_min: None,
                plan_usage: None,
                exec_usage: None,
                invocations: None,
                idle_reaped: false,
            });
            self.issues.last_mut().expect("just pushed")
        }
    }

    /// The idle watchdog stopped the agent of the active issue: the issue
    /// keeps that fact, so a finished run still shows it (docs/adr/0085 D11).
    fn mark_idle_reaped(&mut self) {
        if let Some(n) = self.active {
            self.entry_mut(n).idle_reaped = true;
        }
    }

    /// Fold one event into the state. Pure over `(self, event)`.
    pub(crate) fn apply(&mut self, event: RunEvent) {
        match event {
            RunEvent::QueueBuilt {
                count,
                order,
                stop_before,
                issues,
                ..
            } => self.apply_queue_built(count, order, stop_before, &issues),
            RunEvent::IssueStarted { number, title } => self.apply_issue_started(number, title),
            // Live-region only for the card (the planner's model/effort never
            // changes an issue's status), but it does set the current-phase agent
            // context the `data.agent` block reads (ADR-0019 amendment #96).
            RunEvent::Planning { model, effort } => self.apply_planning(model, effort),
            RunEvent::PlanWritten {
                number,
                open_steps,
                usage,
                ..
            } => self.apply_plan_written(number, open_steps, usage),
            RunEvent::Executing {
                number,
                model,
                effort,
                budget_min,
            } => self.apply_executing(number, model, effort, budget_min),
            RunEvent::IssueClosed {
                number,
                usage,
                invocations,
                ..
            } => self.apply_issue_closed(number, usage, invocations),
            RunEvent::NonGreen { number, outcome } => self.apply_non_green(number, &outcome),
            RunEvent::Skipped {
                number,
                kind,
                blockers,
                ..
            } => self.apply_skipped(number, kind, blockers),
            RunEvent::HumanBlocked { number, .. } => {
                // Its own status so the card and counts surface "waiting on human"
                // apart from a generic dependency skip (ADR-0014).
                self.entry_mut(number).status = IssueStatus::Hitl;
                self.agent = None;
            }
            RunEvent::NeedsSplit { number } => self.apply_needs_split(number),
            RunEvent::Notice { .. } => {}
            RunEvent::DeadlinePassed { number } => {
                self.final_summary = Some(format!("deadline reached before #{number}"));
                self.agent = None;
            }
            // The operator's stop IS the card's terminal state. No issue changes
            // status: the one in flight keeps whatever the run last reported for
            // it, which is the truth — it was worked, it was not delivered.
            RunEvent::RunStopped { number } => self.apply_run_stopped(number),
            // The run declined to start (#222): the deferral sentence IS the card's
            // terminal state — no issue ever changed status.
            RunEvent::RunSkipped { reason } => {
                self.final_summary = Some(reason);
            }
            RunEvent::SleepStarted {
                reset,
                target_epoch,
            } => self.apply_sleep_started(reset, target_epoch),
            RunEvent::SleepEnded => {
                self.sleep = None;
            }
            RunEvent::ApiDegraded => {
                self.degraded = true;
            }
            RunEvent::ApiRecovered => {
                self.degraded = false;
            }
            // The child is gone, so any live retry indicator is now a lie: clear
            // it. A reap can legitimately follow an `ApiDegraded` that never got
            // its matching `ApiRecovered` (the child never recovered), and that is
            // the one case where the #149 pair is closed by something else.
            RunEvent::IdleReaped { .. } => {
                self.degraded = false;
                self.agent = None;
                self.mark_idle_reaped();
            }
            // The agent's own state, verbatim (ADR-0059): the adapter emits on
            // a change only, so a fold is a transition.
            RunEvent::AgentState {
                state,
                since,
                detail,
            } => self.apply_agent_state(state, since, detail),
            RunEvent::KnowledgeConsolidating { notes } => {
                self.consolidating = Some(notes);
            }
            RunEvent::KnowledgeConsolidated { archived } => {
                self.consolidating = None;
                self.consolidated = Some(archived);
            }
            // `run.started` seeds the plan/exec agent identities the `data.agent`
            // block defaults to (ADR-0019 amendment #96); it still carries no
            // per-issue status, so the card fold (issues/counts) is unchanged.
            RunEvent::RunStarted {
                agent, plan_agent, ..
            } => {
                self.exec_agent = agent;
                self.plan_agent = plan_agent;
            }
            // The run-boundary end carries no per-issue status; the fold infers the
            // boundary from the Layer lifecycle, so it is a no-op here.
            // …except the empty-queue border (#222), whose outcome IS the run's whole
            // story: without a summary the card falls back to "stopped before any
            // issue was processed", which reads as an unexplained abort.
            RunEvent::RunFinished { outcome, .. } if outcome == "no_work" => {
                self.final_summary = Some("no open issues to process".to_string());
            }
            RunEvent::RunFinished { .. } => {}
            // The raw plan snapshots carry no per-issue status change (the sink
            // resets its plan-step poll snapshot on `PlanWritten`, not these).
            RunEvent::PlanOpened { .. } | RunEvent::PlanClosed { .. } => {}
            // A warning line, not a state: the session goes on as before.
            RunEvent::HooksUnprovenAllowed { .. } => {}
        }
    }

    fn apply_queue_built(
        &mut self,
        count: u64,
        order: Vec<u64>,
        stop_before: Option<u64>,
        issues: &serde_json::Value,
    ) {
        self.total = count as usize;
        self.order = order;
        self.stop_before = stop_before;
        // Seed the light queue scope from the enriched snapshot (tolerating
        // the legacy `Null` shape and missing titles) — NOT into `issues`,
        // so the Telegram card fold never renders not-yet-started issues.
        if let serde_json::Value::Array(arr) = issues {
            self.queue = arr
                .iter()
                .filter_map(|e| {
                    let number = e.get("number")?.as_u64()?;
                    let title = e
                        .get("title")
                        .and_then(|t| t.as_str())
                        .unwrap_or_default()
                        .to_string();
                    Some(QueueRef { number, title })
                })
                .collect();
        }
    }

    fn apply_issue_started(&mut self, number: u64, title: String) {
        // A new active issue supersedes a still-non-terminal prior one: it
        // emitted no lifecycle outcome (a plan-only dry run, or an execution
        // whose outcome never arrived), so it is done as far as the run goes.
        if let Some(prev) = self.active.filter(|p| *p != number) {
            let e = self.entry_mut(prev);
            if !e.status.is_terminal() {
                e.status = IssueStatus::Planned;
            }
        }
        self.active = Some(number);
        self.agent = None;
        let e = self.entry_mut(number);
        e.title = title;
        e.status = IssueStatus::Planning;
        // The render facts are per-pass, not cumulative: a restarted issue
        // must not inherit the previous pass's model/effort/budget.
        e.model = None;
        e.effort = None;
        e.budget_min = None;
        e.plan_usage = None;
        e.exec_usage = None;
    }

    fn apply_planning(&mut self, model: Option<String>, effort: Option<String>) {
        self.cur_agent = Some(self.plan_agent.clone());
        self.cur_model = model.clone();
        self.cur_effort = effort.clone();
        // The planner's labels land on the active issue only when present:
        // an absent field must not blank what a previous event set.
        if let Some(n) = self.active {
            let e = self.entry_mut(n);
            if model.is_some() {
                e.model = model;
            }
            if effort.is_some() {
                e.effort = effort;
            }
        }
    }

    fn apply_plan_written(&mut self, number: u64, open_steps: u64, usage: UsageLite) {
        let Some(n) = self.resolve(number) else {
            return;
        };
        let e = self.entry_mut(n);
        e.plan_usage = Some(usage);
        e.status = if open_steps == 0 {
            IssueStatus::Infeasible
        } else {
            IssueStatus::Planning
        };
    }

    fn apply_executing(
        &mut self,
        number: u64,
        model: String,
        effort: Option<String>,
        budget_min: u64,
    ) {
        self.cur_agent = Some(self.exec_agent.clone());
        self.cur_model = (!model.is_empty()).then(|| model.clone());
        self.cur_effort = effort.clone();
        let Some(n) = self.resolve(number) else {
            return;
        };
        let e = self.entry_mut(n);
        e.status = IssueStatus::Executing;
        // The per-issue model is set unconditionally, empty string included:
        // the console's active line renders the bare separator that produces,
        // and `cur_model`'s `None` degradation would change it.
        e.model = Some(model);
        if effort.is_some() {
            e.effort = effort;
        }
        e.budget_min = Some(budget_min);
    }

    fn apply_issue_closed(&mut self, number: u64, usage: UsageLite, invocations: u64) {
        let Some(n) = self.resolve(number) else {
            return;
        };
        let e = self.entry_mut(n);
        e.exec_usage = Some(usage);
        // `0` (a manual construction or pre-#270 producer) means "unknown",
        // not "zero spawns", so store `None` and let the done line omit the
        // estimate rather than render a nonsensical `0×`.
        e.invocations = (invocations > 0).then_some(invocations);
        e.status = IssueStatus::Done;
        // The child that reported the agent's state is gone with the
        // issue (ADR-0059 §1): no agent between issues.
        self.agent = None;
    }

    fn apply_skipped(&mut self, number: u64, kind: SkipKind, blockers: Vec<u64>) {
        let e = self.entry_mut(number);
        e.status = IssueStatus::Skipped;
        e.kind = Some(kind);
        e.blocked_by = blockers;
    }

    fn apply_needs_split(&mut self, number: u64) {
        let Some(n) = self.resolve(number) else {
            return;
        };
        self.entry_mut(n).status = IssueStatus::NeedsSplit;
        self.agent = None;
    }

    fn apply_run_stopped(&mut self, number: u64) {
        self.final_summary = Some(match number {
            0 => "stopped by the operator".to_owned(),
            n => format!("stopped by the operator during #{n}"),
        });
        self.agent = None;
    }

    fn apply_sleep_started(&mut self, reset: String, target_epoch: i64) {
        self.sleep = Some(SleepState {
            reset,
            target_epoch,
        });
        // A run in `sleeping` has no agent at all (ADR-0059 §1).
        self.agent = None;
    }

    fn apply_agent_state(&mut self, state: String, since: String, detail: Option<String>) {
        self.agent = Some(AgentState {
            state,
            since,
            detail,
        });
    }

    fn apply_non_green(&mut self, number: u64, outcome: &str) {
        let Some(n) = self.resolve(number) else {
            return;
        };
        // A `Blocked` execution outcome is its own status; everything else
        // non-green collapses to NonGreen.
        let status = if outcome.starts_with("Blocked") {
            IssueStatus::Blocked
        } else {
            IssueStatus::NonGreen
        };
        self.entry_mut(n).status = status;
        self.final_summary = Some(format!("stopped on #{n}: {outcome}"));
        self.agent = None;
    }
}

/// Fold a whole event stream into a [`RunState`], seeded with a title and size.
///
/// A convenience over repeated [`RunState::apply`], used by the fold tests and
/// available to the future ADR-0006 presenter; the live worker applies events one
/// at a time, so this is unused by the binary itself.
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "the binary applies events one at a time; tests use the fold"
    )
)]
pub(crate) fn fold(
    title: impl Into<String>,
    total: usize,
    events: impl IntoIterator<Item = RunEvent>,
) -> RunState {
    let mut state = RunState::new(title, total);
    for event in events {
        state.apply(event);
    }
    state
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod agent_state_tests;

#[cfg(test)]
mod summary_tests;
