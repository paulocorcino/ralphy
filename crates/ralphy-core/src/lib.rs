//! `ralphy-core` — the execution-mode-agnostic heart of Ralphy.
//!
//! It owns the *method*: the queue, the run lifecycle, branch policy, and the
//! [`Agent`] contract. It never names an agent vendor, an execution mode, a PTY,
//! or a model. Everything vendor-specific lives behind [`Agent`], inside an
//! adapter. See docs/adr/0002 for the boundary this enforces.

use std::time::Duration;

/// Default per-issue wall-clock budget in minutes: resolved via
/// `--max-minutes-per-issue` (flag) → persisted `claude.max_minutes_per_issue`
/// (settings) → this constant, in that precedence order. The default is a
/// finite backstop (60 min) so an unset flag/setting cannot silently produce
/// an unbounded per-issue budget. Pass `0` explicitly (flag or
/// `claude.max_minutes_per_issue = 0`) to opt out of the cap entirely — `0`
/// stays a valid, deliberate "no per-issue cap" sentinel, just no longer the
/// default. The single source of truth for every adapter's default — keep
/// adapter `Default` impls pointing here rather than re-spelling a literal.
pub const DEFAULT_MAX_MINUTES_PER_ISSUE: u64 = 0;

/// The finite window the runner-enforced verify gate (ADR-0011) borrows when the
/// per-issue budget is disabled (`max_minutes_per_issue == 0`). The gate normally
/// inherits the issue's remaining time budget, but a disabled cap must not collapse
/// it to a 0s timeout — so verify falls back to this bounded window instead. Large
/// enough that a real verify command has room to run, finite so the timeout stays
/// well-defined.
pub const VERIFY_GATE_FALLBACK_MINUTES: u64 = 90;

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
pub mod cmdcost;
pub mod diagnosis;
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
    QueueReport, RunClock, StopReason, WaitOutcome, WallClock, STOP_BEFORE_LABEL,
    TRIAGE_AGENT_LABEL,
};
pub use settings::{Settings, VerifySettings};
pub use tracker::{GhTracker, IssueTracker};
pub use triage_draft::{DraftIssue, TriageDraft, TriageItem, TriageVerdict};
pub use types::{Execution, Issue, Outcome, Plan, PlanLimit, Usage, Workspace};
pub use verify::{VerifyReport, VerifySpec};
