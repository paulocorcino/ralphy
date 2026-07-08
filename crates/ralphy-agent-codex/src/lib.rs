//! The Codex CLI adapter: drives `codex exec` behind the core [`Agent`] contract.
//! Everything Codex-specific — the binary, the model and reasoning-effort flags,
//! the headless invocation, and the signal→[`Outcome`] mapping — is confined here.
//! See docs/adr/0004.
//!
//! Unlike the Claude adapter (a live PTY session with a Stop-hook flag file),
//! Codex needs no interactive session: `plan` and `execute` both run headless
//! `codex exec` with the prompt piped on stdin, and completion is detected from
//! Codex-native signals — the `RALPHY_DONE_EXIT`/`RALPHY_BLOCKED_EXIT` sentinels
//! in the `-o` final-message file, the process exit code, and a HEAD-diff commit
//! check — mapped onto the same core [`Outcome`].

use std::fs;
use std::path::PathBuf;
use std::time::Instant;

use anyhow::{Context, Result};
use ralphy_adapter_support::{
    list_session_files, run_exec_session, run_plan_session, ExecCfg, IssueBudget, PlanCfg,
    PROMPT_EXECUTE,
};
use ralphy_core::{git, plan, Agent, Execution, Issue, Plan, PlanLimit, Workspace};
use tracing::info;

mod auth;
mod command;
mod outcome;
mod skills;
mod tasks;
mod usage;
/// `codex exec -i <FILE>` attaches images to the initial prompt, so the triage
/// session delivers fetched screenshots by argv path (ADR-0025 §4).
pub const ACCEPTS_IMAGES: bool = true;

use auth::{
    is_codex_auth_error, is_codex_limit_text, parse_codex_reset_hint, CODEX_AUTH_ERROR_MSG,
};
use command::{
    build_codex_command, codex_config_model, recommended_tier, tier_to_effort, DEFAULT_CODEX_MODEL,
};
use outcome::classify_codex_outcome;
use skills::materialize_codex_skills;
pub use tasks::{diagnose_repo, draft_issues, triage_issues};
use usage::{codex_sessions_dir, fold_rollout_usage};

/// The Codex planning prompt, embedded so the binary is self-contained as a global
/// tool. A variant of `prompt.plan.md` that emits a vendor-neutral
/// `low|medium|high` complexity tier (mapped to reasoning effort) instead of a
/// Claude model name. Copied to `.ralphy/plan-charter.md` for the live session
/// to read; only a one-line pointer is piped on stdin. Single source of truth
/// lives at `assets/prompts/`.
const PROMPT_PLAN_CODEX: &str = include_str!("../../../assets/prompts/prompt.plan.codex.md");

/// Drives the `codex` CLI. `model` is the operator override (else
/// [`DEFAULT_CODEX_MODEL`]); `run_dir` is where the captured logs live;
/// `max_minutes_per_issue` is the per-issue wall budget, clamped to `run_deadline`
/// when the run carries a global deadline.
pub struct CodexAgent {
    model: Option<String>,
    run_dir: PathBuf,
    budget: IssueBudget,
}

impl CodexAgent {
    pub fn new(model: Option<String>, run_dir: PathBuf) -> Self {
        Self {
            model,
            run_dir,
            budget: IssueBudget::new(ralphy_core::DEFAULT_MAX_MINUTES_PER_ISSUE),
        }
    }

    /// Set the per-issue wall-clock budget in minutes (mirrors `ClaudeAgent::with_max_minutes_per_issue`).
    pub fn with_max_minutes_per_issue(mut self, minutes: u64) -> Self {
        self.budget = self.budget.with_max_minutes_per_issue(minutes);
        self
    }

    /// Set the run's global wall-clock deadline (from `--deadline-hours`). Each
    /// issue's budget is then clamped to it, so an issue started just under the
    /// global limit can't overrun by a whole per-issue window (mirrors
    /// `ClaudeAgent::with_run_deadline`).
    pub fn with_run_deadline(mut self, run_deadline: Option<Instant>) -> Self {
        self.budget = self.budget.with_run_deadline(run_deadline);
        self
    }

    /// The deadline for the current issue: the per-issue budget, clamped to the
    /// run's global deadline when one is set. A budget of `0` disables the
    /// per-issue cap — the issue is then bounded only by the run deadline (or the
    /// far-future [`ralphy_core::UNBOUNDED_ISSUE_HORIZON`] when none is set).
    /// The plan/execute paths read the budget directly (`self.budget.timeout`);
    /// this stays as the deadline oracle the budget tests assert against.
    #[cfg(test)]
    fn issue_deadline(&self) -> Instant {
        self.budget.deadline(ralphy_core::UNBOUNDED_ISSUE_HORIZON)
    }

    /// The single model decision point, in precedence order: the explicit
    /// `--exec-model` override, then the `model` from the user's Codex config, then
    /// [`DEFAULT_CODEX_MODEL`]. Honouring the config means a ChatGPT-auth account —
    /// which rejects `gpt-5-codex` — picks up the model it is actually entitled to
    /// with no explicit flag. Codex routes complexity by reasoning effort, not a
    /// model swap (ADR-0004 D3), so this stays a single value.
    fn resolve_model(&self) -> String {
        if let Some(m) = self.model.as_deref() {
            return m.to_string();
        }
        codex_config_model().unwrap_or_else(|| DEFAULT_CODEX_MODEL.to_string())
    }
}

impl Agent for CodexAgent {
    fn name(&self) -> &'static str {
        "codex"
    }

    fn plan(&self, issue: &Issue, ws: &Workspace) -> Result<Plan> {
        let plan_path = ws.plan_path();
        let model = self.resolve_model();
        let out_path = ws.ralphy_dir().join("codex-last.txt");
        let log_path = self.run_dir.join("codex.log");
        // Snapshotting the rollout tree around the call is Codex-specific (ADR-0008
        // D10, appeared-over-grew): a file that APPEARED is this run's session, one
        // that merely grew is a concurrent pre-existing session and is excluded.
        let sessions_dir = codex_sessions_dir();
        let snapshot = || {
            sessions_dir
                .as_deref()
                .map(|d| list_session_files(d, "jsonl", true, Some("rollout-")))
                .unwrap_or_default()
        };

        let run = || {
            materialize_codex_skills(ws)?;
            let _ = fs::remove_file(&out_path);
            let before = snapshot();
            // Planning always runs at `high` effort (ADR-0004 D3).
            let cmd = build_codex_command(&model, "high", ws.repo_root(), &out_path);
            info!(model = %model, effort = "high", "planning with codex exec");
            // Clock the budget at the spawn, not method entry, so the run_deadline
            // clamp isn't eroded by the preceding dir/snapshot setup.
            let timeout = self.budget.timeout(ralphy_core::UNBOUNDED_ISSUE_HORIZON);
            let r = self.run_codex(cmd, ralphy_adapter_support::PLAN_CHARTER, timeout)?;
            let after = snapshot();
            Ok((r, (before, after)))
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
                charter_body: PROMPT_PLAN_CODEX,
                log_path: &log_path,
                auth_msg: CODEX_AUTH_ERROR_MSG,
                no_plan_msg: "codex produced no plan",
            },
            run,
            is_codex_auth_error,
            // A usage limit during planning is not a generic failure: surface it as
            // a typed `PlanLimit` (with the parsed reset hint) so the runner routes
            // it through the same stop-and-report / auto-resume path as an
            // execute-time `Outcome::Limit`, rather than aborting the whole run.
            |log| {
                ralphy_adapter_support::detect_limit(
                    log,
                    is_codex_limit_text,
                    parse_codex_reset_hint,
                )
                .map(|reset| PlanLimit { reset }.into())
            },
        )?;

        // None = resumed (finalized plan kept, no vendor run): no rollout payload to
        // fold, so report zero planning tokens — the whole point of the resume fix.
        let usage = match session {
            Some((_, (before, after))) => fold_rollout_usage(&before, &after, Some(model)),
            None => ralphy_core::Usage::default(),
        };
        let md = fs::read_to_string(&plan_path).context("reading the written plan.md")?;
        Ok(Plan {
            open_steps: plan::count_open_steps(&md),
            recommended_model: recommended_tier(&md),
            path: plan_path,
            usage,
        })
    }

    fn execute(&self, plan: &Plan, ws: &Workspace) -> Result<Execution> {
        let model = self.resolve_model();
        // Execution takes the plan's neutral complexity tier as reasoning effort.
        let effort = tier_to_effort(plan.recommended_model.as_deref());
        let out_path = ws.ralphy_dir().join("codex-last.txt");
        let log_path = self.run_dir.join("codex.log");
        // HEAD before/after bounds the work this call committed (progress guard).
        let before_sha = git::head_sha(ws.repo_root()).unwrap_or_default();
        // Snapshot the rollout tree around the call for appeared-over-grew token
        // capture (ADR-0008 D10), the same rule the Claude adapter uses.
        let sessions_dir = codex_sessions_dir();
        let snapshot = || {
            sessions_dir
                .as_deref()
                .map(|d| list_session_files(d, "jsonl", true, Some("rollout-")))
                .unwrap_or_default()
        };

        let run = || {
            materialize_codex_skills(ws)?;
            let _ = fs::remove_file(&out_path);
            let before = snapshot();
            let cmd = build_codex_command(&model, effort, ws.repo_root(), &out_path);
            info!(model = %model, effort, "executing with codex exec");
            // Clock the budget at the spawn, not method entry, so the run_deadline
            // clamp isn't eroded by the preceding dir/snapshot setup.
            let timeout = self.budget.timeout(ralphy_core::UNBOUNDED_ISSUE_HORIZON);
            let r = self.run_codex(cmd, PROMPT_EXECUTE, timeout)?;
            let after = snapshot();
            Ok((r, (before, after)))
        };

        let ralphy_dir = ws.ralphy_dir();
        let (r, (before, after)) = run_exec_session(
            ExecCfg {
                ralphy_dir: &ralphy_dir,
                run_dir: &self.run_dir,
                log_path: &log_path,
                auth_msg: CODEX_AUTH_ERROR_MSG,
            },
            run,
            is_codex_auth_error,
        )?;

        let after_sha = git::head_sha(ws.repo_root()).unwrap_or_default();
        let committed = before_sha != after_sha;
        let out = fs::read_to_string(&out_path).unwrap_or_default();

        let outcome =
            classify_codex_outcome(r.exited_cleanly, r.timed_out, committed, &out, &r.log);
        info!(
            ?outcome,
            exited_cleanly = r.exited_cleanly,
            timed_out = r.timed_out,
            committed,
            "codex execution ended"
        );
        Ok(Execution {
            outcome,
            usage: fold_rollout_usage(&before, &after, Some(model)),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::time::Duration;

    // ── with_max_minutes_per_issue ──────────────────────────────────────────

    #[test]
    fn codex_honours_max_minutes_per_issue() {
        assert_eq!(
            CodexAgent::new(None, PathBuf::from("/run"))
                .budget
                .max_minutes_per_issue,
            ralphy_core::DEFAULT_MAX_MINUTES_PER_ISSUE
        );
        let a = CodexAgent::new(None, PathBuf::from("/run")).with_max_minutes_per_issue(120);
        assert_eq!(a.budget.max_minutes_per_issue, 120);
        let short = CodexAgent::new(None, PathBuf::from("/run")).with_max_minutes_per_issue(1);
        let long = CodexAgent::new(None, PathBuf::from("/run")).with_max_minutes_per_issue(1000);
        assert!(long.issue_deadline() > short.issue_deadline());
        let rd = Instant::now() + Duration::from_secs(1);
        let clamped = CodexAgent::new(None, PathBuf::from("/run"))
            .with_max_minutes_per_issue(1000)
            .with_run_deadline(Some(rd));
        assert!(clamped.issue_deadline() <= rd);
    }

    #[test]
    fn codex_zero_minutes_disables_the_per_issue_cap() {
        // `0` → no per-issue cap: the deadline sits at the far-future horizon,
        // well past any finite budget.
        let uncapped = CodexAgent::new(None, PathBuf::from("/run")).with_max_minutes_per_issue(0);
        let capped = CodexAgent::new(None, PathBuf::from("/run")).with_max_minutes_per_issue(1000);
        assert!(uncapped.issue_deadline() > capped.issue_deadline());

        // …but an uncapped issue is still bounded by the run deadline when set.
        let rd = Instant::now() + Duration::from_secs(1);
        let bounded = CodexAgent::new(None, PathBuf::from("/run"))
            .with_max_minutes_per_issue(0)
            .with_run_deadline(Some(rd));
        assert!(bounded.issue_deadline() <= rd);
    }

    // ── resolve_model ───────────────────────────────────────────────────────

    #[test]
    fn resolve_model_override_wins() {
        // The explicit --exec-model override wins over config and default, with no
        // dependence on the machine's Codex config.
        let overridden = CodexAgent::new(Some("gpt-5".into()), PathBuf::from("/run"));
        assert_eq!(overridden.resolve_model(), "gpt-5");
    }

    // ── trait binding (compile-level) ───────────────────────────────────────

    #[test]
    fn codex_agent_is_a_dyn_agent() {
        // Proves `CodexAgent: Agent` and that it can be handed to the core as a
        // `&dyn Agent` (the core never learns the vendor).
        let agent = CodexAgent::new(None, PathBuf::from("/run"));
        let _as_dyn: &dyn Agent = &agent;
    }

    // ── PROMPT_PLAN_CODEX reviewer step ────────────────────────────────────

    #[test]
    fn plan_charter_file_carries_full_prompt() {
        // The full charter lands on disk (mirrors exec.md) and per-issue stdin
        // stays a one-line pointer — pins the byte reduction issue #80 delivers.
        let base =
            std::env::temp_dir().join(format!("ralphy-codex-charter-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        let ws = Workspace::new(&base);
        fs::create_dir_all(ws.ralphy_dir()).unwrap();

        fs::write(ws.plan_charter_path(), PROMPT_PLAN_CODEX).unwrap();
        assert_eq!(
            fs::read_to_string(ws.plan_charter_path()).unwrap(),
            PROMPT_PLAN_CODEX
        );
        assert!(ralphy_adapter_support::PLAN_CHARTER.len() * 50 < PROMPT_PLAN_CODEX.len());

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn prompt_plan_codex_contains_reviewer_step() {
        assert!(
            PROMPT_PLAN_CODEX.contains("reviewer"),
            "planning prompt must reference the reviewer skill"
        );
        let lower = PROMPT_PLAN_CODEX.to_lowercase();
        assert!(
            lower.contains("only") && lower.contains("commits you made"),
            "reviewer step must scope to this issue's own commits"
        );
        assert!(
            !PROMPT_PLAN_CODEX.contains("independent subagent"),
            "must not use Claude 'independent subagent' phrasing"
        );
    }

    #[test]
    fn prompt_plan_codex_carries_finalize_trailer() {
        assert!(
            PROMPT_PLAN_CODEX.contains("<!-- ralphy-plan: issue="),
            "planning prompt must instruct writing the finalized-plan trailer"
        );
    }
}
