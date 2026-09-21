//! The pure fold/state machine: [`RunState`] and its [`RunState::apply`] fold over
//! the semantic [`RunEvent`] stream (ADR-0007 D6).

use super::event::RunEvent;
use super::{SkipKind, UsageLite};

/// The per-issue status the card renders. Distinguishes ⏭️ skipped (a dependency
/// or `stop-before` skip) from 🤷 infeasible (an empty plan), 🧩 needs-split (a
/// bundle verdict awaiting a human split) and ⛔ blocked (a `Blocked` execution
/// outcome) from a generic non-green stop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IssueStatus {
    Planning,
    Executing,
    /// A plan-only pass superseded by the next `issue started` (a dry run): the
    /// issue got a plan and never executed, so it is terminal without a lifecycle
    /// outcome of its own.
    Planned,
    Done,
    Skipped,
    Blocked,
    Infeasible,
    NeedsSplit,
    NonGreen,
    /// Stalled on a human gate (`ready-for-human`/`HITL`) in its dependency path
    /// (ADR-0014). Distinct from a generic dependency skip so the operator can
    /// see which chains are waiting on a person, not on the queue.
    Hitl,
}

impl IssueStatus {
    /// Whether this is a terminal status (the issue will not change further).
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            IssueStatus::Planned
                | IssueStatus::Done
                | IssueStatus::Skipped
                | IssueStatus::Blocked
                | IssueStatus::Infeasible
                | IssueStatus::NeedsSplit
                | IssueStatus::NonGreen
                | IssueStatus::Hitl
        )
    }

    /// The wire name for the `run.finished.issues` rollup `status` field (#96):
    /// `Some(name)` for a terminal status (one of `planned|done|skipped|blocked|
    /// infeasible|needs_split|non_green|hitl`), `None` for the non-terminal
    /// `Planning`/`Executing` — the rollup includes only terminal entries.
    pub fn status_wire(&self) -> Option<&'static str> {
        match self {
            IssueStatus::Planned => Some("planned"),
            IssueStatus::Done => Some("done"),
            IssueStatus::Skipped => Some("skipped"),
            IssueStatus::Blocked => Some("blocked"),
            IssueStatus::Infeasible => Some("infeasible"),
            IssueStatus::NeedsSplit => Some("needs_split"),
            IssueStatus::NonGreen => Some("non_green"),
            IssueStatus::Hitl => Some("hitl"),
            IssueStatus::Planning | IssueStatus::Executing => None,
        }
    }
}

/// An active usage-limit sleep: the reset-time hint shown on the card and the
/// Unix-seconds wake anchor the live countdown is computed against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SleepState {
    pub reset: String,
    pub target_epoch: i64,
}

/// One issue in the run, in queue order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssueEntry {
    pub number: u64,
    pub title: String,
    pub status: IssueStatus,
    /// The skip reason, retained ONLY for a [`IssueStatus::Skipped`] entry so the
    /// `run.finished.issues` rollup can carry `kind` on a skip (#96); `None` for
    /// every non-skip entry.
    pub kind: Option<SkipKind>,
    /// The still-open issue(s) that gated a [`SkipKind::BlockedBy`] skip, retained so
    /// the Telegram card and the `run.finished.issues` rollup can name them
    /// (`blocked by #139`). Empty for every non-skip entry and for the other skip kinds.
    pub blocked_by: Vec<u64>,
    /// The current phase's display model for THIS issue, reset on `issue started`.
    /// Distinct from the run-level [`RunState::cur_model`], which never resets and
    /// degrades an empty exec model to `None` — the console's active line keeps the
    /// empty-string form, so the two cannot be merged.
    pub model: Option<String>,
    /// The current phase's reasoning effort for THIS issue, reset on `issue started`.
    pub effort: Option<String>,
    /// The execution budget ceiling in minutes, from `executing`.
    pub budget_min: Option<u64>,
    /// The planning phase's usage, stashed at `plan written` so the `done` line can
    /// show the issue total (plan + execute) and price each phase's model (ADR-0008 D8).
    pub plan_usage: Option<UsageLite>,
    /// The execution phase's usage, from `issue closed`.
    pub exec_usage: Option<UsageLite>,
    /// Vendor spawns this issue paid for, from `issue closed`. `None` for a
    /// not-yet-closed or older entry.
    pub invocations: Option<u64>,
}

/// A light `{number, title}` reference for the `run.started.queue` scope list and
/// the title source for the `run.finished.issues` rollup (ADR-0019 amendment #96).
/// Seeded from the enriched `queue.built` snapshot into a dedicated
/// [`RunState::queue`] field — kept OUT of [`RunState::issues`] so the Telegram card
/// fold (which iterates `issues`) never renders not-yet-started issues.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueueRef {
    pub number: u64,
    pub title: String,
}

/// The agent's hook-reported state (ADR-0059 §1): `working`, `waiting`,
/// `done` or `blocked`, with when it began and what a `waiting` agent asks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentState {
    pub state: String,
    pub since: String,
    pub detail: Option<String>,
}

/// A tally of issues by terminal/active status, for the card's counter line.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Counts {
    /// Plan-only passes superseded by the next issue (a dry run).
    pub planned: usize,
    pub done: usize,
    pub skipped: usize,
    pub blocked: usize,
    pub infeasible: usize,
    pub needs_split: usize,
    pub non_green: usize,
    pub planning: usize,
    pub executing: usize,
    /// Issues stalled on a human gate in their path (ADR-0014) — the
    /// "waiting on human" bucket, kept distinct from generic skips.
    pub hitl: usize,
}

/// The transport-agnostic state of a run, folded from its event stream.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunState {
    /// The card title (derived by the caller, not from events).
    pub title: String,
    /// The queue size from `queue built`.
    pub total: usize,
    /// The queue's issue numbers in working order, from `queue built` — the source
    /// of the console's derived queue bar (its pending list is `order` minus the
    /// entries already terminal).
    pub order: Vec<u64>,
    /// The first issue carrying `stop-before`: the run halts before it, so the
    /// derived queue bar marks the cut. `None` when no queue issue is tagged.
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
    /// The active issue's child is in a sustained API-degraded state (issue #149):
    /// set on `ApiDegraded`, cleared on `ApiRecovered`. Drives the live-region
    /// retry indicator and the Telegram matched-pair push edge.
    pub degraded: bool,
    /// The agent's own state as its hooks last reported it (ADR-0059): set on
    /// `AgentState`, cleared by every event that ends the child — a new issue,
    /// the issue closing green or not, a reap, a usage-limit sleep, the
    /// deadline, the operator's stop. A state belongs to the child that
    /// produced it, and after those that child is gone (§1: a run in
    /// `sleeping` has no agent at all).
    pub agent: Option<AgentState>,
    /// Whether the run has reached its terminal state. The worker flips this to
    /// `true` just before the final card render so the card grows its `🏁` footer
    /// (the consolidated single-component card — ADR-0007 D3); it stays `false`
    /// through the live run so the issue list is the last visible group.
    pub finished: bool,
    /// Live: the end-of-run knowledge consolidation is in progress over this many
    /// loose notes. Set by `KnowledgeConsolidating`, cleared on completion (and
    /// hidden once the run is `finished`, so a failed session leaves no stale line).
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
    pub fn new(title: impl Into<String>, total: usize) -> Self {
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
            });
            self.issues.last_mut().expect("just pushed")
        }
    }

    /// Fold one event into the state. Pure over `(self, event)`.
    pub fn apply(&mut self, event: RunEvent) {
        match event {
            RunEvent::QueueBuilt {
                count,
                order,
                stop_before,
                issues,
                ..
            } => {
                self.total = count as usize;
                self.order = order;
                self.stop_before = stop_before;
                // Seed the light queue scope from the enriched snapshot (tolerating
                // the legacy `Null` shape and missing titles) — NOT into `issues`,
                // so the Telegram card fold never renders not-yet-started issues.
                if let serde_json::Value::Array(arr) = &issues {
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
            RunEvent::IssueStarted { number, title } => {
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
            // Live-region only for the card (the planner's model/effort never
            // changes an issue's status), but it does set the current-phase agent
            // context the `data.agent` block reads (ADR-0019 amendment #96).
            RunEvent::Planning { model, effort } => {
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
            RunEvent::PlanWritten {
                number,
                open_steps,
                usage,
                ..
            } => {
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
            RunEvent::Executing {
                number,
                model,
                effort,
                budget_min,
            } => {
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
            RunEvent::IssueClosed {
                number,
                usage,
                invocations,
                ..
            } => {
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
            RunEvent::NonGreen { number, outcome } => {
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
            RunEvent::Skipped {
                number,
                kind,
                blockers,
                ..
            } => {
                let e = self.entry_mut(number);
                e.status = IssueStatus::Skipped;
                e.kind = Some(kind);
                e.blocked_by = blockers;
            }
            RunEvent::HumanBlocked { number, .. } => {
                // Its own status so the card and counts surface "waiting on human"
                // apart from a generic dependency skip (ADR-0014).
                self.entry_mut(number).status = IssueStatus::Hitl;
                self.agent = None;
            }
            RunEvent::NeedsSplit { number } => {
                let Some(n) = self.resolve(number) else {
                    return;
                };
                self.entry_mut(n).status = IssueStatus::NeedsSplit;
                self.agent = None;
            }
            RunEvent::Notice { .. } => {}
            RunEvent::DeadlinePassed { number } => {
                self.final_summary = Some(format!("deadline reached before #{number}"));
                self.agent = None;
            }
            // The operator's stop IS the card's terminal state. No issue changes
            // status: the one in flight keeps whatever the run last reported for
            // it, which is the truth — it was worked, it was not delivered.
            RunEvent::RunStopped { number } => {
                self.final_summary = Some(match number {
                    0 => "stopped by the operator".to_string(),
                    n => format!("stopped by the operator during #{n}"),
                });
                self.agent = None;
            }
            // The run declined to start (#222): the deferral sentence IS the card's
            // terminal state — no issue ever changed status.
            RunEvent::RunSkipped { reason } => {
                self.final_summary = Some(reason);
            }
            RunEvent::SleepStarted {
                reset,
                target_epoch,
            } => {
                self.sleep = Some(SleepState {
                    reset,
                    target_epoch,
                });
                // A run in `sleeping` has no agent at all (ADR-0059 §1).
                self.agent = None;
            }
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
            }
            // The agent's own state, verbatim (ADR-0059): the adapter emits on
            // a change only, so a fold is a transition.
            RunEvent::AgentState {
                state,
                since,
                detail,
            } => {
                self.agent = Some(AgentState {
                    state,
                    since,
                    detail,
                });
            }
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
        }
    }

    /// Tally the issues by status for the counter line.
    pub fn counts(&self) -> Counts {
        let mut c = Counts::default();
        for e in &self.issues {
            match e.status {
                IssueStatus::Planned => c.planned += 1,
                IssueStatus::Done => c.done += 1,
                IssueStatus::Skipped => c.skipped += 1,
                IssueStatus::Blocked => c.blocked += 1,
                IssueStatus::Infeasible => c.infeasible += 1,
                IssueStatus::NeedsSplit => c.needs_split += 1,
                IssueStatus::NonGreen => c.non_green += 1,
                IssueStatus::Planning => c.planning += 1,
                IssueStatus::Executing => c.executing += 1,
                IssueStatus::Hitl => c.hitl += 1,
            }
        }
        c
    }

    /// The active issue entry, if any.
    pub fn active_issue(&self) -> Option<&IssueEntry> {
        let n = self.active?;
        self.issues.iter().find(|e| e.number == n)
    }

    /// The most-recently-seen issue in a terminal status (for the collapsed card).
    pub fn most_recent_finished(&self) -> Option<&IssueEntry> {
        self.issues.iter().rev().find(|e| e.status.is_terminal())
    }

    /// The run's current phase — `starting|planning|executing|sleeping|
    /// consolidating`: a usage-limit sleep wins, then an in-progress
    /// consolidation, then the active issue's phase, else the initial
    /// `starting`. A sleep reports `sleeping` even with an executing issue so a
    /// long usage-limit pause is never mistaken for progress or for death.
    ///
    /// One vocabulary, read by the `run.heartbeat` envelope (ADR-0019) and by
    /// the run snapshot's phase block (ADR-0047 §5).
    pub fn run_phase(&self) -> &'static str {
        if self.sleep.is_some() {
            return "sleeping";
        }
        if self.consolidating.is_some() {
            return "consolidating";
        }
        match self.active_issue().map(|e| &e.status) {
            Some(IssueStatus::Executing) => "executing",
            Some(IssueStatus::Planning) => "planning",
            _ => "starting",
        }
    }
}

/// Fold a whole event stream into a [`RunState`], seeded with a title and size.
///
/// A convenience over repeated [`RunState::apply`], used by the fold tests and
/// available to the future ADR-0006 presenter; the live worker applies events one
/// at a time, so this is unused by the binary itself.
#[allow(dead_code)]
pub fn fold(
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
