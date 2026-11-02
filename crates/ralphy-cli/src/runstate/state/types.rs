//! The plain data the fold produces: the per-issue entry, its status and the run tallies.

use crate::runstate::{SkipKind, UsageLite};

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
    /// Distinct from the run-level `RunState::cur_model`, which never resets and
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
    /// Whether the idle watchdog stopped this issue's agent (docs/adr/0038).
    pub idle_reaped: bool,
}

/// A light `{number, title}` reference for the `run.started.queue` scope list and
/// the title source for the `run.finished.issues` rollup (ADR-0019 amendment #96).
/// Seeded from the enriched `queue.built` snapshot into a dedicated
/// `RunState::queue` field — kept OUT of `RunState::issues` so the Telegram card
/// fold (which iterates `issues`) never renders not-yet-started issues.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueueRef {
    pub number: u64,
    pub title: String,
}

/// The agent's hook-reported state (ADR-0059 §1, ADR-0085 D1): `working`,
/// `waiting`, `done` or `unknown`, with when it began and what a `waiting`
/// agent asks.
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
