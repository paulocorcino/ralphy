//! `ralphy-core` — the execution-mode-agnostic heart of Ralphy.
//!
//! It owns the *method*: the queue, the run lifecycle, branch policy, and the
//! [`Agent`] contract. It never names an agent vendor, an execution mode, a PTY,
//! or a model. Everything vendor-specific lives behind [`Agent`], inside an
//! adapter. See docs/adr/0002 for the boundary this enforces.

use std::time::Duration;

/// Default per-issue wall-clock budget in minutes: resolved via
/// `--max-minutes-per-issue` (flag) → persisted `claude.max_minutes_per_issue`
/// (settings) → this constant, in that precedence order.
///
/// This is an **opt-in productivity cap** — "this issue does not deserve more
/// than N minutes" — and it defaults to `0`, meaning *no per-issue cap*: an
/// issue is then bounded only by the run-level deadline (`--deadline-hours`),
/// if one is set. A wall clock cannot tell a healthy long issue from a wedged
/// child, so defaulting it to a finite value cuts productive work to catch a
/// hang it was never able to recognize.
///
/// **Liveness is deliberately NOT this constant's job** (docs/adr/0038). A child
/// that stops making progress is caught by the idle watchdog
/// ([`DEFAULT_IDLE_MINUTES`] / [`DEFAULT_INTERACTIVE_IDLE_MINUTES`]), which
/// measures *progress* rather than *duration*. Do not restore a finite default
/// here to paper over a liveness gap — that trade was made once (#150) and cost
/// case A to buy case B.
///
/// The single source of truth for every adapter's default — keep adapter
/// `Default` impls pointing here rather than re-spelling a literal.
pub const DEFAULT_MAX_MINUTES_PER_ISSUE: u64 = 0;

/// Default time budget in minutes for the runner-enforced verify gate
/// (ADR-0011), used when `verify.timeout_minutes` is unset.
///
/// The gate owns its own clock: it must always be finite (a verify command needs
/// a well-defined timeout), which is exactly why it can no longer be derived
/// from the per-issue cap — that one is `0`/unbounded by default and would
/// collapse the gate to a 0s timeout. Large enough that a real verify command
/// has room to run.
pub const VERIFY_GATE_FALLBACK_MINUTES: u64 = 90;

/// Default idle watchdog window in minutes for the **headless** child path,
/// where the progress signal is fine-grained: any byte on stdout/stderr counts
/// as liveness. `0` disables the watchdog.
///
/// This is the backstop the per-issue cap used to improvise (docs/adr/0038): it
/// fires on *silence*, not on elapsed time, so a healthy long-running issue is
/// never cut. It is what catches a provider quota block that the child retries
/// silently — the failure mode no stderr matcher can see.
pub const DEFAULT_IDLE_MINUTES: u64 = 20;

/// Default idle watchdog window in minutes for the **interactive (PTY)** child
/// path. Deliberately larger than [`DEFAULT_IDLE_MINUTES`]: the only honest
/// progress signal there is the agent transcript growing, and a legitimate long
/// tool call (say, a 30-minute build) advances nothing in the meantime. PTY
/// bytes are *not* usable as progress — the TUI redraws its spinner forever, so
/// bytes keep flowing while the child is wedged.
///
/// Two different values is the point, not an inconsistency: a coarser progress
/// signal has to buy more slack before it is allowed to kill.
pub const DEFAULT_INTERACTIVE_IDLE_MINUTES: u64 = 45;

/// The horizon an adapter substitutes for the per-issue deadline when the budget
/// is disabled (`max_minutes_per_issue == 0`): far enough out to never fire in a
/// real run, but finite so `Instant` arithmetic and subprocess timeouts stay
/// well-defined. The issue is then bounded only by the run-level deadline
/// (`--deadline-hours`), if one is set.
pub const UNBOUNDED_ISSUE_HORIZON: Duration = Duration::from_secs(365 * 24 * 60 * 60);

mod agent;
mod effort;
pub(crate) mod markdown;
mod runner;
mod tracker;
mod types;

pub mod acceptance;
pub mod blocked;
pub mod cmdcost;
pub mod diagnosis;
pub mod emit;
pub mod environment;
pub mod git;
pub mod github;
pub mod gitignore;
pub mod handoff;
pub mod init_session;
pub mod issues_draft;
pub mod knowledge;
pub mod ledger;
pub mod plan;
pub mod protocol;
pub mod queue_view;
pub mod references;
pub mod repo;
pub mod settings;
pub mod triage_draft;
pub mod verify;

pub use acceptance::{Verdict, VerdictKind};
pub use agent::Agent;
pub use blocked::{
    parse_blocked_by, parse_blocked_by_all, referenced_issues, structured_refs,
    CONSOLIDATED_SPEC_MARKER, PROMOTE_EVIDENCE_MARKER,
};
pub use diagnosis::{DiagnosisReport, RepoKind};
pub use effort::Effort;
pub use init_session::{
    build_diagnose_prompt, build_init_issues_prompt, build_triage_prompt, DraftRequest, IssuesMode,
    TriageRequest, PROMPT_CONSOLIDATE, PROMPT_DIAGNOSE, PROMPT_INIT_ISSUES, PROMPT_TRIAGE,
};
pub use issues_draft::{IssueDraft, IssuesDraft, MilestoneDraft};
pub use ledger::{read_project_rows, read_rows, UsageRow};
pub use queue_view::{resolve_queue_view, IssueView, QueueStatus, QueueView};
pub use ralphy_proc_util::find_program;
pub use references::Reference;
pub use repo::{GitRepo, Repo};
pub use runner::{
    first_stop_before, human_return_label, run_queue, BranchMode, IssueResult, QueueConfig,
    QueueReport, ResultStatus, RunClock, SkipReason, StopReason, WaitOutcome, WallClock,
    STOP_BEFORE_LABEL, TRIAGE_AGENT_LABEL,
};
pub use settings::{Settings, VerifySettings};
pub use tracker::{GhTracker, IssueTracker};
pub use triage_draft::{DraftIssue, TriageDraft, TriageItem, TriageVerdict};
pub use types::{Execution, Issue, Outcome, Plan, PlanLimit, Usage, Workspace};
pub use verify::{VerifyReport, VerifySpec};
