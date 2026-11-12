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

use std::cell::Cell;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{bail, Context, Result};
use ralphy_adapter_support::{
    classify, run_exec_session, run_plan_session, CompletionSignals, ExecCfg, HeadlessRun,
    IssueBudget, PlanCfg, PLAN_CHARTER, PROMPT_EXECUTE,
};
use ralphy_core::{git, plan, Agent, Execution, Issue, Outcome, Plan, PlanLimit, Workspace};

use crate::outcome::{Fold, RunEnd};
use crate::session::Session;

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

/// Whether the operator is signed in, and the message that says how to sign
/// in for Ralphy.
pub use auth::{auth_message, probe_antigravity_login};

/// The models `agy models` lists for the signed-in plan.
pub use catalog::{fetch_catalog, ModelEntry};

/// The owned root a workbench console needs, ready on disk.
pub use console::prepare_console_root;

/// The price-table key a model id bills under: the id without its effort suffix.
pub use effort::price_key;

/// The per-phase models and efforts persisted under `antigravity.*` in
/// `.ralphy/settings.json`.
pub use settings::AntigravitySettings;

/// The four one-shot verbs (`ralphy diagnose`, `init --issues`, `triage`,
/// `consolidate`), each one held session in the owned root.
pub use tasks::{
    consolidate_knowledge, diagnose_repo, draft_issues, triage_issues, OneShotOptions,
};

/// `agy` reads an image by its path in the prompt (D16).
pub const ACCEPTS_IMAGES: bool = true;

/// The agent name: in events, ledger rows and `--agent antigravity`.
const NAME: &str = "antigravity";

/// The run's `agy` log, in the run dir.
const LOG_FILE: &str = "antigravity.log";

/// The planning prompt, assembled from `assets/prompts/plan/` and embedded so
/// the binary is self-contained.
const PROMPT_PLAN_ANTIGRAVITY: &str =
    include_str!("../../../assets/prompts/prompt.plan.antigravity.md");

/// Drives the `agy` CLI. `exec_model` is the model of `execute()` (set with
/// `new`), `plan_model` the model of `plan()`. `None` omits `--model`, and `agy`
/// then uses the default of the operator's plan (D4).
#[derive(Debug)]
#[must_use]
pub struct AntigravityAgent {
    exec_model: Option<String>,
    plan_model: Option<String>,
    plan_effort: Option<String>,
    exec_effort: Option<String>,
    run_dir: PathBuf,
    budget: IssueBudget,
    waiver: bool,
    program: Option<PathBuf>,
    root: Option<PathBuf>,
}

/// The two phases, each with its own model and effort.
#[derive(Debug, Clone, Copy)]
enum Phase {
    Plan,
    Execute,
}

/// The program, root and log of one `plan` or `execute` call.
#[derive(Debug)]
struct Launch {
    program: PathBuf,
    root: PathBuf,
    log_path: PathBuf,
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
            waiver: false,
            program: None,
            root: None,
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

    /// Let a session whose hooks did not prove they run go on, with a warning
    /// (the local-only waiver setting).
    pub fn with_run_without_guard_proof(mut self, waiver: bool) -> Self {
        self.waiver = waiver;
        self
    }

    /// Test seam: run `program` instead of the `agy` the locator finds.
    pub fn with_program(mut self, program: PathBuf) -> Self {
        self.program = Some(program);
        self
    }

    /// Test seam: use `root` instead of the owned root of this OS user.
    pub fn with_root(mut self, root: PathBuf) -> Self {
        self.root = Some(root);
        self
    }

    fn launch(&self) -> Result<Launch> {
        let root = match &self.root {
            Some(root) => root.clone(),
            None => ralphy_proc_util::antigravity::owned_root()?,
        };
        Ok(Launch {
            program: self.program.clone().unwrap_or_else(command::agy_program),
            root,
            log_path: self.run_dir.join(LOG_FILE),
        })
    }

    fn model_and_effort(&self, phase: Phase) -> (Option<&str>, Option<&str>) {
        match phase {
            Phase::Plan => (self.plan_model.as_deref(), self.plan_effort.as_deref()),
            Phase::Execute => (self.exec_model.as_deref(), self.exec_effort.as_deref()),
        }
    }

    /// The session of `phase` in `work_dir`, its budget clocked from now.
    fn session<'a>(&'a self, launch: &'a Launch, phase: Phase, work_dir: &'a Path) -> Session<'a> {
        let (model, effort) = self.model_and_effort(phase);
        Session {
            program: &launch.program,
            root: &launch.root,
            work_dir,
            run_dir: &self.run_dir,
            log_path: &launch.log_path,
            model,
            effort,
            waiver: self.waiver,
            timeout: self.budget.timeout(ralphy_core::UNBOUNDED_ISSUE_HORIZON),
            idle_minutes: self.budget.idle_minutes,
        }
    }
}

impl Agent for AntigravityAgent {
    fn name(&self) -> &'static str {
        NAME
    }

    fn plan(&self, issue: &Issue, ws: &Workspace) -> Result<Plan> {
        let launch = self.launch()?;
        let (model, effort) = self.model_and_effort(Phase::Plan);
        let plan_path = ws.plan_path();
        let auth_msg = auth_message(&launch.root);
        let stop = Cell::new(None);
        let run = || {
            ralphy_core::emit::planning(NAME, model.unwrap_or(""), effort.unwrap_or(""), "");
            let session = self.session(&launch, Phase::Plan, ws.repo_root());
            let r = session.run(PLAN_CHARTER)?;
            stop.set(plan_stop(&r, &launch, model));
            Ok((r, ()))
        };
        let session = run_plan_session(
            PlanCfg {
                issue_number: issue.number,
                ralphy_dir: &ws.ralphy_dir(),
                run_dir: &self.run_dir,
                plan_path: &plan_path,
                plan_charter_path: &ws.plan_charter_path(),
                charter_body: PROMPT_PLAN_ANTIGRAVITY,
                log_path: &launch.log_path,
                auth_msg: &auth_msg,
                no_plan_msg: "antigravity produced no plan",
            },
            run,
            auth::is_auth_error,
            |_log| stop.take(),
        )?;
        let fold = session
            .as_ref()
            .map(|(r, ())| outcome::fold(&r.stdout))
            .unwrap_or_default();
        let md = fs::read_to_string(&plan_path).context("reading the written plan.md")?;
        Ok(Plan {
            open_steps: plan::count_open_steps(&md),
            recommended_model: None,
            path: plan_path,
            usage: usage::usage(&fold, model),
            session_id: fold.conversation_id,
        })
    }

    fn execute(&self, _plan: &Plan, ws: &Workspace) -> Result<Execution> {
        let launch = self.launch()?;
        let (model, effort) = self.model_and_effort(Phase::Execute);
        let before_sha = git::head_sha(ws.repo_root()).unwrap_or_default();
        let auth_msg = auth_message(&launch.root);
        let run = || {
            ralphy_core::emit::executing(NAME, 0, model.unwrap_or(""), effort.unwrap_or(""), "");
            let session = self.session(&launch, Phase::Execute, ws.repo_root());
            Ok((session.run(PROMPT_EXECUTE)?, ()))
        };
        let (r, ()) = run_exec_session(
            ExecCfg {
                ralphy_dir: &ws.ralphy_dir(),
                run_dir: &self.run_dir,
                log_path: &launch.log_path,
                auth_msg: &auth_msg,
            },
            run,
            auth::is_auth_error,
        )?;
        let committed = git::head_sha(ws.repo_root()).unwrap_or_default() != before_sha;
        let (outcome, fold) = execute_outcome(&r, &launch, model, committed)?;
        let exit_code = r.exit_code;
        tracing::info!(
            ?outcome,
            ?exit_code,
            committed,
            "antigravity execution ended"
        );
        Ok(Execution {
            outcome,
            usage: usage::usage(&fold, model),
            session_id: fold.conversation_id,
        })
    }
}

/// The fault of this machine that ended the run, as an error (D9).
fn machine_fault(r: &HeadlessRun, launch: &Launch) -> Result<()> {
    let hooks = root::hooks_file(&launch.root);
    match outcome::machine_fault(r.exit_code, outcome::stderr_of(r), &hooks) {
        Some(fault) => bail!("{fault} (see {})", launch.log_path.display()),
        None => Ok(()),
    }
}

/// The signals of a run that ended (D3); `committed` is false for a plan.
fn signals(r: &HeadlessRun, fold: &Fold, committed: bool) -> CompletionSignals {
    let end = RunEnd {
        code: r.exit_code,
        timed_out: r.timed_out,
        committed,
    };
    outcome::completion_signals(fold, outcome::stderr_of(r), end)
}

/// How an execute run ended: the shared ladder over its signals, with the
/// reset of a limit read from the quota read-out (D6).
fn execute_outcome(
    r: &HeadlessRun,
    launch: &Launch,
    model: Option<&str>,
    committed: bool,
) -> Result<(Outcome, Fold)> {
    machine_fault(r, launch)?;
    let fold = outcome::fold(&r.stdout);
    let mut signals = signals(r, &fold, committed);
    if signals.limit.is_some() {
        signals.limit = Some(quota::read_reset(&launch.program, &launch.root, model));
    }
    Ok((classify(signals), fold))
}

/// Why a plan run left no plan, when it is not the generic case: the fault
/// of this machine, or a limit with its reset.
fn plan_stop(r: &HeadlessRun, launch: &Launch, model: Option<&str>) -> Option<anyhow::Error> {
    if let Err(fault) = machine_fault(r, launch) {
        return Some(fault);
    }
    let fold = outcome::fold(&r.stdout);
    signals(r, &fold, false).limit.map(|_| {
        let reset = quota::read_reset(&launch.program, &launch.root, model);
        PlanLimit { reset }.into()
    })
}

/// Write what the owned `root` holds before any `agy` child starts: the hooks
/// file and the config (D9, D13), then Ralphy's skills (D8). Every run, console
/// and one-shot calls it, so a root deleted between two runs is made again.
pub(crate) fn prepare_root(root: &Path) -> Result<()> {
    root::ensure(root)?;
    skills::materialize(root)
}
