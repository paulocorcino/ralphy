//! The GitHub Copilot CLI adapter: drives headless `copilot` behind the core
//! [`Agent`] contract. Everything Copilot-specific — the binary, the argv, the
//! JSON-lines stream parser, and the signal→[`Outcome`] mapping — is confined
//! here. See docs/adr/0041.
//!
//! Like the Codex, Kimi and OpenCode adapters (and unlike Claude's live PTY
//! session), Copilot needs no interactive session: `plan` and `execute` both pipe
//! the charter on **stdin** with no `-p` at all — `prompt.execute.md` is 23 884
//! bytes before the issue body is appended, against a Windows argv ceiling of
//! ~32 KB (ADR-0041 D2).
//!
//! Token usage is read back from Copilot's own `session-store.db` by the minted
//! `--session-id` ([`usage`], ADR-0041 D10). `tasks.rs` and `skills.rs` still
//! belong to later slices (ADR-0040 Tier 1).

use std::fs;
use std::path::PathBuf;
use std::time::Instant;

use anyhow::{Context, Result};
use ralphy_adapter_support::{
    run_exec_session, run_plan_session, ExecCfg, IssueBudget, PlanCfg, PROMPT_EXECUTE,
};
use ralphy_core::{git, plan, Agent, Execution, Issue, Outcome, Plan, PlanLimit, Workspace};
use tracing::info;

mod auth;
mod command;
mod outcome;
mod usage;

/// `true` (ADR-0041 D12): `copilot --attachment <path>` attaches an image or
/// native document to the initial prompt in non-interactive mode, so a triage
/// attachment fetched per ADR-0025 §4 has a real delivery channel. The flag is
/// unused in this slice; the constant advertises the capability the later triage
/// slice will exercise.
pub const ACCEPTS_IMAGES: bool = true;

use auth::{is_copilot_auth_error, COPILOT_AUTH_ERROR_MSG};
use command::{build_copilot_command, mint_session_id};
use outcome::{classify_copilot_outcome, copilot_final_text};
use usage::copilot_usage;

/// The Copilot planning prompt, embedded so the binary is self-contained as a
/// global tool. A variant of `prompt.plan.md` with no `## Execution model` tier
/// line (Copilot's model is the operator's account default, ADR-0041 D6). Copied
/// to `.ralphy/plan-charter.md` for the session to read; only a one-line pointer
/// is piped on stdin. Single source of truth lives at `assets/prompts/`.
const PROMPT_PLAN_COPILOT: &str = include_str!("../../../assets/prompts/prompt.plan.copilot.md");

/// Drives the `copilot` CLI. `model` is the operator override, omitted from argv
/// when `None` — omission selects the account's current default, which is the
/// correct default rather than a degraded fallback (ADR-0041 D4). `run_dir` is
/// where the captured logs live; `max_minutes_per_issue` is the per-issue wall
/// budget, clamped to `run_deadline` when the run carries a global deadline.
pub struct CopilotAgent {
    model: Option<String>,
    run_dir: PathBuf,
    budget: IssueBudget,
}

impl CopilotAgent {
    pub fn new(model: Option<String>, run_dir: PathBuf) -> Self {
        Self {
            model,
            run_dir,
            budget: IssueBudget::new(ralphy_core::DEFAULT_MAX_MINUTES_PER_ISSUE),
        }
    }

    /// Set the per-issue wall-clock budget in minutes (mirrors `KimiAgent::with_max_minutes_per_issue`).
    pub fn with_max_minutes_per_issue(mut self, minutes: u64) -> Self {
        self.budget = self.budget.with_max_minutes_per_issue(minutes);
        self
    }

    /// Set the idle watchdog window in minutes: reap the child after that long
    /// with no output at all. `0` disables it (docs/adr/0038).
    pub fn with_idle_minutes(mut self, minutes: u64) -> Self {
        self.budget = self.budget.with_idle_minutes(minutes);
        self
    }

    /// Set the run's global wall-clock deadline (from `--deadline-hours`). Each
    /// issue's budget is then clamped to it.
    pub fn with_run_deadline(mut self, run_deadline: Option<Instant>) -> Self {
        self.budget = self.budget.with_run_deadline(run_deadline);
        self
    }

    /// The deadline oracle the budget tests assert against; the plan/execute paths
    /// read the budget directly (`self.budget.timeout`).
    #[cfg(test)]
    fn issue_deadline(&self) -> Instant {
        self.budget.deadline(ralphy_core::UNBOUNDED_ISSUE_HORIZON)
    }
}

impl Agent for CopilotAgent {
    fn name(&self) -> &'static str {
        "copilot"
    }

    fn plan(&self, issue: &Issue, ws: &Workspace) -> Result<Plan> {
        let plan_path = ws.plan_path();
        let log_path = self.run_dir.join("copilot.log");
        let session_id = mint_session_id();

        let run = || {
            // `None` (the default) omits `--model` entirely, which selects the
            // account's own default — the correct default, not a fallback (D4).
            let cmd = build_copilot_command(&session_id, self.model.as_deref(), ws.repo_root());
            ralphy_core::emit::planning("copilot", self.model.as_deref().unwrap_or(""), "");
            // Clock the budget at the spawn, not method entry, so the run_deadline
            // clamp isn't eroded by the preceding dir setup.
            let timeout = self.budget.timeout(ralphy_core::UNBOUNDED_ISSUE_HORIZON);
            let r = self.run_copilot(cmd, ralphy_adapter_support::PLAN_CHARTER, timeout)?;
            Ok((r, ()))
        };

        let ralphy_dir = ws.ralphy_dir();
        let charter_path = ws.plan_charter_path();
        let session = run_plan_session(
            PlanCfg {
                issue_number: issue.number,
                ralphy_dir: &ralphy_dir,
                run_dir: &self.run_dir,
                plan_path: &plan_path,
                plan_charter_path: &charter_path,
                charter_body: PROMPT_PLAN_COPILOT,
                log_path: &log_path,
                auth_msg: COPILOT_AUTH_ERROR_MSG,
                no_plan_msg: "copilot produced no plan",
            },
            run,
            is_copilot_auth_error,
            // A usage limit during planning is not a generic failure: surface it as
            // a typed `PlanLimit` so the runner routes it through the same
            // stop-and-report / auto-resume path as an execute-time
            // `Outcome::Limit`, rather than aborting the run with "produced no
            // plan". No reset hint is recoverable (D11), so the ADR-0030 synthetic
            // cadence sets the wait.
            |log| {
                ralphy_adapter_support::detect_limit(log, auth::is_copilot_limit_text, |_| None)
                    .map(|reset| PlanLimit { reset }.into())
            },
        )?;

        let md = fs::read_to_string(&plan_path).context("reading the written plan.md")?;
        Ok(Plan {
            open_steps: plan::count_open_steps(&md),
            // Copilot runs the account's own default model, no complexity tier (D6).
            recommended_model: None,
            path: plan_path,
            // A RESUMED finalized plan ran no `copilot` process, so no session by
            // this id exists to read: report zero rather than another run's rows.
            usage: session
                .as_ref()
                .map(|_| copilot_usage(&session_id))
                .unwrap_or_default(),
            // `None` = a finalized plan was RESUMED and no `copilot` process ran,
            // so no session by this id exists in the store.
            session_id: session.map(|_| session_id),
        })
    }

    fn execute(&self, _plan: &Plan, ws: &Workspace) -> Result<Execution> {
        let log_path = self.run_dir.join("copilot.log");
        let session_id = mint_session_id();
        // HEAD before/after bounds the work this call committed (progress guard).
        // Load-bearing: `result.usage.codeChanges` counts the vendor's own
        // write-tool activity, NOT repository change (spike §2).
        let before_sha = git::head_sha(ws.repo_root()).unwrap_or_default();

        let run = || {
            let cmd = build_copilot_command(&session_id, self.model.as_deref(), ws.repo_root());
            ralphy_core::emit::executing("copilot", 0, self.model.as_deref().unwrap_or(""), "");
            let timeout = self.budget.timeout(ralphy_core::UNBOUNDED_ISSUE_HORIZON);
            let r = self.run_copilot(cmd, PROMPT_EXECUTE, timeout)?;
            Ok((r, ()))
        };

        let ralphy_dir = ws.ralphy_dir();
        let (r, ()) = run_exec_session(
            ExecCfg {
                ralphy_dir: &ralphy_dir,
                run_dir: &self.run_dir,
                log_path: &log_path,
                auth_msg: COPILOT_AUTH_ERROR_MSG,
            },
            run,
            is_copilot_auth_error,
        )?;

        let after_sha = git::head_sha(ws.repo_root()).unwrap_or_default();
        let committed = before_sha != after_sha;
        let final_text = copilot_final_text(&r.stdout);
        let outcome: Outcome = classify_copilot_outcome(
            r.exited_cleanly,
            r.timed_out,
            committed,
            r.exit_code,
            &final_text,
            &r.log,
        );
        info!(
            ?outcome,
            exited_cleanly = r.exited_cleanly,
            timed_out = r.timed_out,
            exit_code = ?r.exit_code,
            committed,
            "copilot execution ended"
        );
        Ok(Execution {
            outcome,
            usage: copilot_usage(&session_id),
            session_id: Some(session_id),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::time::Duration;

    #[test]
    fn copilot_agent_is_a_dyn_agent() {
        let agent = CopilotAgent::new(None, PathBuf::from("/run"));
        let _as_dyn: &dyn Agent = &agent;
    }

    #[test]
    fn copilot_honours_max_minutes_per_issue() {
        assert_eq!(
            CopilotAgent::new(None, PathBuf::from("/run"))
                .budget
                .max_minutes_per_issue,
            ralphy_core::DEFAULT_MAX_MINUTES_PER_ISSUE
        );
        let a = CopilotAgent::new(None, PathBuf::from("/run")).with_max_minutes_per_issue(120);
        assert_eq!(a.budget.max_minutes_per_issue, 120);
        let short = CopilotAgent::new(None, PathBuf::from("/run")).with_max_minutes_per_issue(1);
        let long = CopilotAgent::new(None, PathBuf::from("/run")).with_max_minutes_per_issue(1000);
        assert!(long.issue_deadline() > short.issue_deadline());
        let rd = Instant::now() + Duration::from_secs(1);
        let clamped = CopilotAgent::new(None, PathBuf::from("/run"))
            .with_max_minutes_per_issue(1000)
            .with_run_deadline(Some(rd));
        assert!(clamped.issue_deadline() <= rd);
    }

    #[test]
    fn copilot_zero_minutes_disables_the_per_issue_cap() {
        let uncapped = CopilotAgent::new(None, PathBuf::from("/run")).with_max_minutes_per_issue(0);
        let capped =
            CopilotAgent::new(None, PathBuf::from("/run")).with_max_minutes_per_issue(1000);
        assert!(uncapped.issue_deadline() > capped.issue_deadline());

        let rd = Instant::now() + Duration::from_secs(1);
        let bounded = CopilotAgent::new(None, PathBuf::from("/run"))
            .with_max_minutes_per_issue(0)
            .with_run_deadline(Some(rd));
        assert!(bounded.issue_deadline() <= rd);
    }

    /// ADR-0040 Tier 1: adapter tests are inline `#[cfg(test)] mod tests`, never a
    /// `tests/` directory — an integration dir would re-link the crate and lose
    /// access to the `pub(crate)` seams every test here asserts on.
    #[test]
    fn no_tests_directory() {
        assert!(
            !std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/tests")).exists(),
            "adapter tests stay inline (ADR-0040 Tier 1)"
        );
    }

    #[test]
    fn prompt_plan_copilot_has_no_execution_model_line() {
        assert!(
            !PROMPT_PLAN_COPILOT.contains("## Execution model"),
            "the Copilot plan prompt must drop the complexity tier line (D6)"
        );
    }

    #[test]
    fn prompt_plan_copilot_carries_finalize_trailer() {
        assert!(
            PROMPT_PLAN_COPILOT.contains("<!-- ralphy-plan: issue=<N> -->"),
            "planning prompt must instruct writing the exact finalized-plan trailer"
        );
    }

    /// The reason the charter goes on stdin and never on argv (D2): at 23 884 bytes
    /// it alone is within ~30 % of the Windows ~32 KB argv ceiling, before the issue
    /// body is even appended. The floor is 23 000 — a real margin under today's
    /// size, so the test pins the ORDER of magnitude rather than the exact byte
    /// count, which every prompt edit would otherwise churn.
    #[test]
    fn exec_charter_exceeds_argv_safe_size() {
        assert!(
            ralphy_adapter_support::PROMPT_EXECUTE.len() > 23_000,
            "charter is {} bytes",
            ralphy_adapter_support::PROMPT_EXECUTE.len()
        );
    }
}
