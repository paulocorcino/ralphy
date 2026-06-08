//! `ralphy-core` — the execution-mode-agnostic heart of Ralphy.
//!
//! It owns the *method*: the queue, the run lifecycle, branch policy, and the
//! [`Agent`] contract. It never names an agent vendor, an execution mode, a PTY,
//! or a model. Everything vendor-specific lives behind [`Agent`], inside an
//! adapter. See docs/adr/0002 for the boundary this enforces.

mod agent;
mod runner;
mod tracker;
mod types;

pub mod git;
pub mod github;
pub mod gitignore;
pub mod plan;

pub use agent::Agent;
pub use runner::{
    run, run_queue, IssueResult, QueueConfig, QueueReport, RunClock, RunConfig, RunOutcome,
    RunReport, StopReason, WallClock,
};
pub use tracker::{GhTracker, IssueTracker};
pub use types::{Issue, Outcome, Plan, Workspace};
