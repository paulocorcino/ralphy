//! `ralphy-core` — the execution-mode-agnostic heart of Ralphy.
//!
//! It owns the *method*: the queue, the run lifecycle, branch policy, and the
//! [`Agent`] contract. It never names an agent vendor, an execution mode, a PTY,
//! or a model. Everything vendor-specific lives behind [`Agent`], inside an
//! adapter. See docs/adr/0002 for the boundary this enforces.

use std::time::Duration;

/// Default per-issue wall-clock budget in minutes, used when neither
/// `--max-minutes-per-issue` nor `claude.max_minutes_per_issue` is set. The
/// single source of truth for every adapter's default — keep adapter `Default`
/// impls pointing here rather than re-spelling a literal.
pub const DEFAULT_MAX_MINUTES_PER_ISSUE: u64 = 90;

/// The horizon an adapter substitutes for the per-issue deadline when the budget
/// is disabled (`max_minutes_per_issue == 0`): far enough out to never fire in a
/// real run, but finite so `Instant` arithmetic and subprocess timeouts stay
/// well-defined. The issue is then bounded only by the run-level deadline
/// (`--deadline-hours`), if one is set.
pub const UNBOUNDED_ISSUE_HORIZON: Duration = Duration::from_secs(365 * 24 * 60 * 60);

mod agent;
pub(crate) mod markdown;
mod runner;
mod tracker;
mod types;

pub mod acceptance;
pub mod blocked;
pub mod diagnosis;
pub mod git;
pub mod github;
pub mod gitignore;
pub mod handoff;
pub mod init_session;
pub mod issues_draft;
pub mod knowledge;
pub mod ledger;
pub mod plan;
pub mod settings;
pub mod verify;

pub use acceptance::{Verdict, VerdictKind};
pub use agent::Agent;
pub use blocked::parse_blocked_by;
pub use diagnosis::{DiagnosisReport, RepoKind};
pub use init_session::{
    build_diagnose_prompt, build_init_issues_prompt, DraftRequest, IssuesMode, PROMPT_DIAGNOSE,
    PROMPT_INIT_ISSUES,
};
pub use issues_draft::{IssueDraft, IssuesDraft, MilestoneDraft};
pub use ledger::{read_project_rows, read_rows, UsageRow};
pub use runner::{
    run, run_queue, BranchMode, IssueResult, QueueConfig, QueueReport, RunClock, RunConfig,
    RunOutcome, RunReport, StopReason, WaitOutcome, WallClock, STOP_BEFORE_LABEL,
};
pub use settings::{ClaudeSettings, OpenCodeSettings, Settings, VerifySettings};
pub use tracker::{GhTracker, IssueTracker};
pub use types::{Execution, Issue, Outcome, Plan, PlanLimit, Usage, Workspace};
pub use verify::{VerifyReport, VerifySpec};
