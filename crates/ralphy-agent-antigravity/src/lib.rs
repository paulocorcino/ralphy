//! The Antigravity CLI adapter: drives headless `agy` behind the core agent
//! contract. Everything specific to `agy` is in this crate: the argv, the child's
//! environment, the model and effort rules, and the root Ralphy owns for it
//! (ADR-0086).
//!
//! Every run points `agy` at one owned root per OS user (`--gemini_dir`, D8).
//! `prepare_root` writes what that root must hold before any child starts:
//! the hooks file with the guard (D9), `swarm` turned off (D13) and Ralphy's
//! skills.
// D7 of the lint plan: production code here parses outside data; indexing must not panic.
#![cfg_attr(not(test), deny(clippy::indexing_slicing))]

use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::Result;
use ralphy_adapter_support::IssueBudget;

mod auth;
mod catalog;
mod command;
mod console;
mod effort;
mod outcome;
mod quota;
mod root;
mod session;
mod settings;
mod skills;
mod status;
mod tasks;
mod usage;

/// The owned root a workbench console needs, ready on disk.
pub use console::prepare_console_root;

/// The price-table key a model id bills under: the id without its effort suffix.
pub use effort::price_key;

/// The per-phase models and efforts persisted under `antigravity.*` in
/// `.ralphy/settings.json`.
pub use settings::AntigravitySettings;

/// Drives the `agy` CLI. `exec_model` is the model of `execute()` (set with
/// `new`), `plan_model` the model of `plan()`. `None` omits `--model`, and `agy`
/// then uses the default of the operator's plan (D4).
#[derive(Debug)]
#[must_use]
#[expect(dead_code, reason = "used once the Agent impl lands")]
pub struct AntigravityAgent {
    exec_model: Option<String>,
    plan_model: Option<String>,
    plan_effort: Option<String>,
    exec_effort: Option<String>,
    run_dir: PathBuf,
    budget: IssueBudget,
}

impl AntigravityAgent {
    pub fn new(model: Option<String>, run_dir: PathBuf) -> Self {
        Self {
            exec_model: model,
            plan_model: None,
            plan_effort: None,
            exec_effort: None,
            run_dir,
            budget: IssueBudget::new(ralphy_core::DEFAULT_MAX_MINUTES_PER_ISSUE),
        }
    }

    /// Set the model of `plan()`.
    pub fn with_plan_model(mut self, model: Option<String>) -> Self {
        self.plan_model = model;
        self
    }

    /// Set the effort word of `plan()` (ADR-0044 D5), passed as `--effort`.
    pub fn with_plan_effort(mut self, effort: Option<String>) -> Self {
        self.plan_effort = effort;
        self
    }

    /// Set the effort word of `execute()` (ADR-0044 D5), passed as `--effort`.
    pub fn with_exec_effort(mut self, effort: Option<String>) -> Self {
        self.exec_effort = effort;
        self
    }

    /// Set the per-issue wall-clock budget in minutes.
    pub fn with_max_minutes_per_issue(mut self, minutes: u64) -> Self {
        self.budget = self.budget.with_max_minutes_per_issue(minutes);
        self
    }

    /// Set the idle window in minutes: the child is ended after that long with
    /// no output. `0` turns the watch off (ADR-0038).
    pub fn with_idle_minutes(mut self, minutes: u64) -> Self {
        self.budget = self.budget.with_idle_minutes(minutes);
        self
    }

    /// Set the run's global wall-clock deadline (from `--deadline-hours`).
    pub fn with_run_deadline(mut self, run_deadline: Option<Instant>) -> Self {
        self.budget = self.budget.with_run_deadline(run_deadline);
        self
    }
}

/// Write what the owned `root` holds before any `agy` child starts: the hooks
/// file and the config (D9, D13), then Ralphy's skills (D8). Every run, console
/// and one-shot calls it, so a root deleted between two runs is made again.
pub(crate) fn prepare_root(root: &Path) -> Result<()> {
    root::ensure(root)?;
    skills::materialize(root)
}
