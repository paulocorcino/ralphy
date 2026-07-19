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
                let e = self.entry_mut(number);
                e.title = title;
                e.status = IssueStatus::Planning;
                // The render facts are per-pass, not cumulative: a restarted issue
                // must not inherit the previous pass's model/effort/budget.
                e.model = None;
                e.effort = None;
                e.budget_min = None;
                e.plan_usage = None;
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
            RunEvent::IssueClosed { number, usage, .. } => {
                let Some(n) = self.resolve(number) else {
                    return;
                };
                let e = self.entry_mut(n);
                e.exec_usage = Some(usage);
                e.status = IssueStatus::Done;
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
            }
            RunEvent::NeedsSplit { number } => {
                let Some(n) = self.resolve(number) else {
                    return;
                };
                self.entry_mut(n).status = IssueStatus::NeedsSplit;
            }
            RunEvent::Notice { .. } => {}
            RunEvent::DeadlinePassed { number } => {
                self.final_summary = Some(format!("deadline reached before #{number}"));
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
mod tests {
    use super::*;
    use crate::runstate::event::event_to_runevent;
    use crate::runstate::{EventFields, UsageLite};
    use tracing::Level;

    #[test]
    fn api_degraded_folds_the_degraded_flag() {
        let mut s = RunState::new("t", 1);
        assert!(!s.degraded);
        s.apply(RunEvent::ApiDegraded);
        assert!(s.degraded);
        s.apply(RunEvent::ApiRecovered);
        assert!(!s.degraded);
    }

    #[test]
    fn full_lifecycle_yields_expected_statuses_and_summary() {
        let events = vec![
            RunEvent::QueueBuilt {
                count: 2,
                order: vec![1, 2],
                stop_before: None,
                issues: serde_json::Value::Null,
                assignee_filter: None,
                scope: None,
            },
            RunEvent::IssueStarted {
                number: 1,
                title: "one".into(),
            },
            RunEvent::PlanWritten {
                number: 1,
                open_steps: 3,
                usage: UsageLite::default(),
                steps: vec![],
            },
            // The execution event carries no number; it must land on the active issue.
            RunEvent::Executing {
                number: 0,
                budget_min: 45,
                model: String::new(),
                effort: None,
            },
            RunEvent::IssueClosed {
                number: 1,
                tokens: 0,
                usage: UsageLite::default(),
            },
            RunEvent::IssueStarted {
                number: 2,
                title: "two".into(),
            },
            RunEvent::PlanWritten {
                number: 2,
                open_steps: 2,
                usage: UsageLite::default(),
                steps: vec![],
            },
            RunEvent::Executing {
                number: 0,
                budget_min: 45,
                model: String::new(),
                effort: None,
            },
            RunEvent::NonGreen {
                number: 2,
                outcome: "Stuck".into(),
            },
        ];
        let state = fold("title", 2, events);
        assert_eq!(state.total, 2);
        assert_eq!(state.issues.len(), 2);
        assert_eq!(state.issues[0].status, IssueStatus::Done);
        assert_eq!(state.issues[0].title, "one");
        assert_eq!(state.issues[1].status, IssueStatus::NonGreen);
        let summary = state.final_summary.as_deref().unwrap();
        assert!(summary.contains("#2"), "summary: {summary}");
        assert!(summary.contains("Stuck"), "summary: {summary}");
    }

    #[test]
    fn plan_written_with_zero_steps_is_infeasible() {
        let mut state = RunState::new("t", 1);
        state.apply(RunEvent::IssueStarted {
            number: 5,
            title: "x".into(),
        });
        state.apply(RunEvent::PlanWritten {
            number: 5,
            open_steps: 0,
            usage: UsageLite::default(),
            steps: vec![],
        });
        assert_eq!(state.issues[0].status, IssueStatus::Infeasible);
    }

    #[test]
    fn needs_split_upgrades_infeasible_and_decodes_from_stable_message() {
        // The runner emits "plan written" (0 steps) then "bundle plan — needs
        // split"; the fold must land on NeedsSplit, not stay Infeasible.
        let mut state = RunState::new("t", 1);
        state.apply(RunEvent::IssueStarted {
            number: 3,
            title: "W1 bundle".into(),
        });
        state.apply(RunEvent::PlanWritten {
            number: 3,
            open_steps: 0,
            usage: UsageLite::default(),
            steps: vec![],
        });
        assert_eq!(state.issues[0].status, IssueStatus::Infeasible);
        state.apply(RunEvent::NeedsSplit { number: 3 });
        assert_eq!(state.issues[0].status, IssueStatus::NeedsSplit);
        assert!(state.issues[0].status.is_terminal());
        assert_eq!(state.counts().needs_split, 1);
        assert_eq!(state.counts().infeasible, 0);

        // Decoder: the stable runner message maps to the typed event.
        assert_eq!(
            event_to_runevent(
                "ralphy_core::runner",
                "bundle plan — needs split",
                &EventFields {
                    message: "bundle plan — needs split".into(),
                    number: Some(3),
                    ..Default::default()
                }
            ),
            Some(RunEvent::NeedsSplit { number: 3 })
        );
    }

    #[test]
    fn skipped_event_sets_skipped_status() {
        let mut state = RunState::new("t", 1);
        state.apply(RunEvent::Skipped {
            number: 9,
            kind: SkipKind::BlockedBy,
            label: None,
            blockers: vec![7],
        });
        assert_eq!(state.issues[0].status, IssueStatus::Skipped);
        // The open-blocker list is retained on the entry for the card / rollup.
        assert_eq!(state.issues[0].blocked_by, vec![7]);
    }

    #[test]
    fn non_green_blocked_outcome_maps_to_blocked() {
        let mut state = RunState::new("t", 1);
        state.apply(RunEvent::IssueStarted {
            number: 1,
            title: "a".into(),
        });
        state.apply(RunEvent::NonGreen {
            number: 1,
            outcome: "Blocked".into(),
        });
        assert_eq!(state.issues[0].status, IssueStatus::Blocked);
    }

    #[test]
    fn deadline_event_sets_terminal_summary() {
        let mut state = RunState::new("t", 3);
        state.apply(RunEvent::DeadlinePassed { number: 7 });
        assert!(state.final_summary.as_deref().unwrap().contains("#7"));
    }

    #[test]
    fn zero_numbered_event_without_active_is_ignored() {
        // An `Executing` (number 0) whose `IssueStarted` was dropped under
        // back-pressure must not materialize a phantom issue `#0`.
        let mut state = RunState::new("t", 1);
        state.apply(RunEvent::Executing {
            number: 0,
            budget_min: 45,
            model: String::new(),
            effort: None,
        });
        assert!(state.issues.is_empty());
    }

    #[test]
    fn sleep_started_sets_state_and_sleep_ended_clears_it() {
        let mut state = RunState::new("t", 1);
        assert!(state.sleep.is_none());
        state.apply(RunEvent::SleepStarted {
            reset: "14:30".into(),
            target_epoch: 1_700_000_000,
        });
        let sleep = state.sleep.as_ref().expect("sleep set on start");
        assert_eq!(sleep.reset, "14:30");
        assert_eq!(sleep.target_epoch, 1_700_000_000);
        state.apply(RunEvent::SleepEnded);
        assert!(state.sleep.is_none(), "resume clears the sleep");
    }

    #[test]
    fn counts_tally_each_status() {
        let mut state = RunState::new("t", 4);
        state.apply(RunEvent::IssueStarted {
            number: 1,
            title: "a".into(),
        });
        state.apply(RunEvent::IssueClosed {
            number: 1,
            tokens: 0,
            usage: UsageLite::default(),
        });
        state.apply(RunEvent::Skipped {
            number: 2,
            kind: SkipKind::BlockedBy,
            label: None,
            blockers: vec![],
        });
        state.apply(RunEvent::IssueStarted {
            number: 3,
            title: "c".into(),
        });
        let c = state.counts();
        assert_eq!(c.done, 1);
        assert_eq!(c.skipped, 1);
        assert_eq!(c.planning, 1);
        assert_eq!(state.active_issue().map(|e| e.number), Some(3));
        assert_eq!(state.most_recent_finished().map(|e| e.number), Some(2));
    }

    #[test]
    fn human_blocked_is_its_own_status_and_bucket() {
        // A HumanBlocked event folds to the Hitl status (not generic Skipped) and
        // tallies its own bucket — so the card and counts surface "waiting on
        // human" distinctly (ADR-0014).
        let mut state = RunState::new("t", 2);
        state.apply(RunEvent::HumanBlocked {
            number: 5,
            on: vec![30],
        });
        let entry = state.issues.iter().find(|e| e.number == 5).unwrap();
        assert_eq!(entry.status, IssueStatus::Hitl);
        let c = state.counts();
        assert_eq!(c.hitl, 1);
        assert_eq!(c.skipped, 0, "a human gate is not a generic skip");
    }

    #[test]
    fn apply_knowledge_consolidation_sets_then_clears_live_and_records_count() {
        let mut state = RunState::new("t", 1);
        state.apply(RunEvent::KnowledgeConsolidating { notes: 4 });
        assert_eq!(state.consolidating, Some(4));
        assert_eq!(state.consolidated, None);
        // Completion clears the live flag and records the archived tally.
        state.apply(RunEvent::KnowledgeConsolidated { archived: 4 });
        assert_eq!(state.consolidating, None);
        assert_eq!(state.consolidated, Some(4));
    }

    #[test]
    fn apply_run_boundary_events_leave_the_card_fold_unchanged() {
        // `run.started` now seeds the agent-context fields (for the `data.agent`
        // block), but must not perturb the card fold — issues, counts, active, or
        // any status. `run.finished` stays a full no-op.
        let mut before = RunState::new("t", 1);
        before.apply(RunEvent::IssueStarted {
            number: 1,
            title: "a".into(),
        });
        let mut after = before.clone();
        after.apply(RunEvent::RunStarted {
            repo: "o/r".into(),
            queue_labels: vec![],
            agent: "claude".into(),
            plan_agent: "codex".into(),
            branch_mode: "new".into(),
            branch: "origin/main".into(),
            deadline_hours: None,
        });
        after.apply(RunEvent::RunFinished {
            outcome: "completed".into(),
            issues_done: 1,
            issues_skipped: 0,
            issues_total: 1,
            up: 0,
            cr: 0,
            cw: 0,
            out: 0,
            duration_s: 1,
        });
        // The card-visible fold is byte-unchanged.
        assert_eq!(before.issues, after.issues);
        assert_eq!(before.counts(), after.counts());
        assert_eq!(before.active, after.active);
        // But the agent identities are now seeded from `run.started`.
        assert_eq!(after.exec_agent, "claude");
        assert_eq!(after.plan_agent, "codex");
    }

    #[test]
    fn apply_notice_is_noop_on_runstate() {
        let mut before = RunState::new("t", 1);
        before.apply(RunEvent::IssueStarted {
            number: 1,
            title: "a".into(),
        });
        let mut after = before.clone();
        after.apply(RunEvent::Notice {
            level: Level::WARN,
            message: "some warning".into(),
        });
        assert_eq!(before, after);
    }

    #[test]
    fn apply_skipped_with_all_kinds_sets_skipped_status() {
        let mut state = RunState::new("t", 3);
        state.apply(RunEvent::Skipped {
            number: 1,
            kind: SkipKind::BlockedBy,
            label: None,
            blockers: vec![],
        });
        state.apply(RunEvent::Skipped {
            number: 2,
            kind: SkipKind::StopBefore,
            label: None,
            blockers: vec![],
        });
        state.apply(RunEvent::Skipped {
            number: 3,
            kind: SkipKind::HumanReturn,
            label: Some("wontfix".into()),
            blockers: vec![],
        });
        assert_eq!(state.issues[0].status, IssueStatus::Skipped);
        assert_eq!(state.issues[1].status, IssueStatus::Skipped);
        assert_eq!(state.issues[2].status, IssueStatus::Skipped);
    }

    #[test]
    fn queue_built_seeds_queue_ref_not_issues() {
        // The enriched snapshot seeds `state.queue` ({number,title}) but leaves
        // `state.issues` empty — so the Telegram card renders nothing until an issue
        // actually starts.
        let mut state = RunState::new("t", 2);
        state.apply(RunEvent::QueueBuilt {
            count: 2,
            order: vec![1, 2],
            stop_before: None,
            issues: serde_json::json!([
                {"number": 1, "title": "one"},
                {"number": 2, "title": "two"},
            ]),
            assignee_filter: None,
            scope: None,
        });
        assert_eq!(
            state.queue,
            vec![
                QueueRef {
                    number: 1,
                    title: "one".into()
                },
                QueueRef {
                    number: 2,
                    title: "two".into()
                },
            ]
        );
        assert!(state.issues.is_empty(), "queue.built must not touch issues");
        // A legacy `Null` snapshot leaves the queue empty rather than panicking.
        let mut legacy = RunState::new("t", 1);
        legacy.apply(RunEvent::QueueBuilt {
            count: 1,
            order: vec![1],
            stop_before: None,
            issues: serde_json::Value::Null,
            assignee_filter: None,
            scope: None,
        });
        assert!(legacy.queue.is_empty());
    }

    #[test]
    fn apply_threads_phase_agent_context() {
        // `run.started` seeds the plan/exec identities; a `Planning` fold flips the
        // current agent to the plan agent (with its model/effort), an `Executing`
        // fold to the exec agent — the source of the `data.agent` block (#96).
        let mut state = RunState::new("t", 1);
        assert_eq!(state.cur_agent, None);
        assert_eq!(state.cur_model, None);
        state.apply(RunEvent::RunStarted {
            repo: "o/r".into(),
            queue_labels: vec![],
            agent: "claude".into(),
            plan_agent: "codex".into(),
            branch_mode: "new".into(),
            branch: "origin/main".into(),
            deadline_hours: None,
        });
        state.apply(RunEvent::IssueStarted {
            number: 1,
            title: "a".into(),
        });
        state.apply(RunEvent::Planning {
            model: Some("opus".into()),
            effort: Some("high".into()),
        });
        assert_eq!(state.cur_agent.as_deref(), Some("codex"));
        assert_eq!(state.cur_model.as_deref(), Some("opus"));
        assert_eq!(state.cur_effort.as_deref(), Some("high"));
        state.apply(RunEvent::Executing {
            number: 0,
            budget_min: 45,
            model: "claude-sonnet-4".into(),
            effort: Some("medium".into()),
        });
        assert_eq!(state.cur_agent.as_deref(), Some("claude"));
        assert_eq!(state.cur_model.as_deref(), Some("claude-sonnet-4"));
        assert_eq!(state.cur_effort.as_deref(), Some("medium"));
        // An empty exec model degrades to `None` rather than an empty-string label.
        state.apply(RunEvent::Executing {
            number: 0,
            budget_min: 45,
            model: String::new(),
            effort: None,
        });
        assert_eq!(state.cur_model, None);
    }

    #[test]
    fn issue_started_supersedes_a_non_terminal_prior_issue() {
        // The dry-run bug: a plan-only pass left the prior issue perenially
        // "planning". The next `issue started` is the fold's supersede edge.
        let mut state = RunState::new("t", 2);
        state.apply(RunEvent::IssueStarted {
            number: 1,
            title: "one".into(),
        });
        state.apply(RunEvent::PlanWritten {
            number: 1,
            open_steps: 3,
            usage: UsageLite::default(),
            steps: vec![],
        });
        state.apply(RunEvent::IssueStarted {
            number: 2,
            title: "two".into(),
        });
        assert_eq!(state.issues[0].status, IssueStatus::Planned);
        let c = state.counts();
        assert_eq!(c.planned, 1);
        assert_eq!(c.planning, 1, "only the new issue is still planning");
    }

    #[test]
    fn per_issue_render_facts_fold_with_presenter_semantics() {
        // The presenter's per-field rules, now in the fold: `Planning` writes only
        // what is `Some`, `Executing` overwrites the model unconditionally (empty
        // string included), and `IssueStarted` resets the facts per issue.
        let mut state = RunState::new("t", 2);
        state.apply(RunEvent::IssueStarted {
            number: 1,
            title: "one".into(),
        });
        state.apply(RunEvent::Planning {
            model: Some("opus".into()),
            effort: None,
        });
        state.apply(RunEvent::Executing {
            number: 0,
            budget_min: 45,
            model: String::new(),
            effort: Some("medium".into()),
        });
        assert_eq!(state.issues[0].model, Some(String::new()));
        assert_eq!(state.issues[0].effort.as_deref(), Some("medium"));
        assert_eq!(state.issues[0].budget_min, Some(45));
        state.apply(RunEvent::IssueStarted {
            number: 2,
            title: "two".into(),
        });
        assert_eq!(state.issues[1].model, None);
        assert_eq!(state.issues[1].budget_min, None);
    }

    #[test]
    fn queue_built_seeds_the_working_order_and_stop_before_cut() {
        let mut state = RunState::new("t", 3);
        state.apply(RunEvent::QueueBuilt {
            count: 3,
            order: vec![1, 2, 3],
            stop_before: Some(3),
            issues: serde_json::Value::Null,
            assignee_filter: None,
            scope: None,
        });
        assert_eq!(state.order, vec![1, 2, 3]);
        assert_eq!(state.stop_before, Some(3));
    }

    #[test]
    fn planned_status_wire_is_additive() {
        assert_eq!(IssueStatus::Planned.status_wire(), Some("planned"));
        assert!(IssueStatus::Planned.is_terminal());
    }

    #[test]
    fn apply_executing_with_model_sets_executing_status() {
        let mut state = RunState::new("t", 1);
        state.apply(RunEvent::IssueStarted {
            number: 1,
            title: "a".into(),
        });
        state.apply(RunEvent::Executing {
            number: 1,
            budget_min: 45,
            model: "claude-opus-4".into(),
            effort: None,
        });
        assert_eq!(state.issues[0].status, IssueStatus::Executing);
    }
}
