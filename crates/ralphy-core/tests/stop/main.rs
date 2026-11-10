//! The cooperative stop (docs/adr/0054), end to end through `run_queue`.
//!
//! **This file is its own test binary on purpose, and nothing else may join it.**
//! `ralphy_core::stop` is a PROCESS-GLOBAL flag, and `cargo test` shares one
//! process per test binary (nextest does not — both gate this repo, so the
//! weaker runner sets the rule). A stop test living in `queue.rs` would stop
//! every queue test that ran after it, and it would read as flake rather than as
//! a leak. Every test here takes [`StopFlagGuard`], which serializes them and
//! restores the flag even through a panicking assertion.

#![expect(
    clippy::disallowed_methods,
    reason = "nextest runs each test in its own process, so a test may change its environment"
)]

use std::cell::RefCell;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use ralphy_core::{
    run_queue, Agent, BranchMode, Execution, Issue, IssueTracker, Outcome, Plan, QueueConfig,
    ResetAt, RunClock, StopReason, Usage, WaitOutcome, Workspace,
};

// ---- the flag guard ---------------------------------------------------------

static STOP_FLAG_LOCK: Mutex<()> = Mutex::new(());

struct StopFlagGuard(
    #[expect(
        dead_code,
        reason = "the guard holds the lock for its drop and is never read"
    )]
    MutexGuard<'static, ()>,
);

impl StopFlagGuard {
    fn acquire() -> Self {
        // A panicking test poisons the lock; recover rather than cascade — the
        // flag is reset right below, so there is no state left to protect.
        let lock = STOP_FLAG_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        ralphy_core::stop::clear();
        Self(lock)
    }
}

impl Drop for StopFlagGuard {
    fn drop(&mut self) {
        ralphy_core::stop::clear();
    }
}

mod during_run;
mod fakes;
mod gates;
mod repo;

use fakes::*;
use repo::*;
