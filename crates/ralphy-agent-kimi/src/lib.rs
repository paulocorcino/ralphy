//! The Kimi CLI adapter: drives `kimi --print` behind the core [`Agent`] contract.
//! Everything Kimi-specific — the binary, the model flag, the headless invocation,
//! the `stream-json` final-text parser, and the signal→[`Outcome`] mapping — is
//! confined here. See docs/adr/0028.
//!
//! Like the Codex and OpenCode adapters (and unlike Claude's live PTY session),
//! Kimi needs no interactive session: `plan` and `execute` both run headless
//! `kimi --print` with the charter piped on stdin, and completion is detected from
//! the `RALPHY_DONE_EXIT`/`RALPHY_BLOCKED_EXIT` sentinels parsed out of the final
//! assistant message in Kimi's `stream-json` stream, the process exit code, and a
//! HEAD-diff commit check — mapped onto the same core [`Outcome`].
//!
//! This is the walking-skeleton slice (ADR-0028): exit 75 maps to
//! `Outcome::Limit(None)` and `--stop-on-limit` is forced for Kimi (D9, #153);
//! the one-shot init flows (diagnose/draft-issues/triage) go through `tasks.rs`.

use std::fs;
use std::path::PathBuf;
use std::time::Instant;

use anyhow::{Context, Result};
use ralphy_adapter_support::{
    list_session_files, run_exec_session, run_plan_session, ExecCfg, IssueBudget, PlanCfg,
    PROMPT_EXECUTE,
};
use ralphy_core::{git, plan, Agent, Execution, Issue, Plan, Usage, Workspace};
use tracing::info;

mod auth;
mod command;
mod outcome;
mod skills;
mod tasks;
mod usage;

/// `false`, settled at validation (ADR-0028 / 0028-kimi-validation). The model
/// advertises `image_in`/`video_in`, but `kimi --print` exposes **no** attachment
/// or image flag — its only input is a text/stream-json charter on stdin — so
/// there is no verified multimodal path to deliver a fetched image on. Setting
/// `true` would make triage attachment-fetch (ADR-0025 §4) pull images the adapter
/// cannot hand to the CLI. Stays `false` until Kimi ships a `--print` image channel.
pub const ACCEPTS_IMAGES: bool = false;

use auth::{is_kimi_auth_error, KIMI_AUTH_ERROR_MSG};
use command::{build_kimi_command, DEFAULT_KIMI_MODEL};
use outcome::{classify_kimi_outcome, kimi_final_text};
use skills::materialize_kimi_skills;
pub use tasks::{consolidate_knowledge, diagnose_repo, draft_issues, triage_issues};
use usage::{fold_wire_usage, kimi_sessions_dir, wire_session_id};

/// The Kimi planning prompt, embedded so the binary is self-contained as a global
/// tool. A variant of `prompt.plan.md` with no `## Execution model` tier line
/// (Kimi drives a single model, ADR-0028 D3/D8) and the reviewer step committed to
/// the inline `reviewer` skill auto-discovered from the `--skills-dir` store.
/// Copied to `.ralphy/plan-charter.md` for the live session to read; only a
/// one-line pointer is piped on stdin. Single source of truth lives at
/// `assets/prompts/`.
const PROMPT_PLAN_KIMI: &str = include_str!("../../../assets/prompts/prompt.plan.kimi.md");

/// Drives the `kimi` CLI. `model` is the operator override (else
/// [`DEFAULT_KIMI_MODEL`]); `run_dir` is where the captured logs live;
/// `max_minutes_per_issue` is the per-issue wall budget, clamped to `run_deadline`
/// when the run carries a global deadline.
pub struct KimiAgent {
    model: Option<String>,
    run_dir: PathBuf,
    budget: IssueBudget,
}

impl KimiAgent {
    pub fn new(model: Option<String>, run_dir: PathBuf) -> Self {
        Self {
            model,
            run_dir,
            budget: IssueBudget::new(ralphy_core::DEFAULT_MAX_MINUTES_PER_ISSUE),
        }
    }

    /// Set the per-issue wall-clock budget in minutes (mirrors `CodexAgent::with_max_minutes_per_issue`).
    pub fn with_max_minutes_per_issue(mut self, minutes: u64) -> Self {
        self.budget = self.budget.with_max_minutes_per_issue(minutes);
        self
    }

    /// Set the run's global wall-clock deadline (from `--deadline-hours`). Each
    /// issue's budget is then clamped to it (mirrors `CodexAgent::with_run_deadline`).
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

    /// The single model decision: the explicit `--exec-model` override, else
    /// [`DEFAULT_KIMI_MODEL`]. No config parse in this slice (ADR-0028 D4).
    fn resolve_model(&self) -> String {
        self.model
            .clone()
            .unwrap_or_else(|| DEFAULT_KIMI_MODEL.to_string())
    }
}

impl Agent for KimiAgent {
    fn name(&self) -> &'static str {
        "kimi"
    }

    fn plan(&self, issue: &Issue, ws: &Workspace) -> Result<Plan> {
        let plan_path = ws.plan_path();
        let log_path = self.run_dir.join("kimi.log");
        let model = self.resolve_model();
        let sessions_dir = kimi_sessions_dir();
        let snapshot = || {
            sessions_dir
                .as_deref()
                .map(|d| list_session_files(d, "jsonl", true, Some("wire")))
                .unwrap_or_default()
        };

        let run = || {
            let skills_dir = materialize_kimi_skills(ws)?;
            let cmd = build_kimi_command(&model, ws.repo_root(), &skills_dir);
            info!(model = %model, "planning with kimi --print");
            // Clock the budget at the spawn, not method entry, so the run_deadline
            // clamp isn't eroded by the preceding dir/skills setup.
            let timeout = self.budget.timeout(ralphy_core::UNBOUNDED_ISSUE_HORIZON);
            let before = snapshot();
            let r = self.run_kimi(cmd, ralphy_adapter_support::PLAN_CHARTER, timeout)?;
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
                charter_body: PROMPT_PLAN_KIMI,
                log_path: &log_path,
                auth_msg: KIMI_AUTH_ERROR_MSG,
                no_plan_msg: "kimi produced no plan",
            },
            run,
            is_kimi_auth_error,
            // No plan-time usage limit is surfaced for Kimi in this slice (D9).
            |_log| None,
        )?;

        // None = resumed (finalized plan kept, no vendor run): no wire payload to
        // fold, so report zero planning tokens.
        let (usage, session_id) = match session {
            Some((_, (before, after))) => (
                fold_wire_usage(&before, &after, Some(model)),
                wire_session_id(&before, &after),
            ),
            None => (Usage::default(), None),
        };
        let md = fs::read_to_string(&plan_path).context("reading the written plan.md")?;
        Ok(Plan {
            open_steps: plan::count_open_steps(&md),
            // Kimi drives a single model, no complexity tier (ADR-0028 D3).
            recommended_model: None,
            path: plan_path,
            usage,
            session_id,
        })
    }

    fn execute(&self, _plan: &Plan, ws: &Workspace) -> Result<Execution> {
        let log_path = self.run_dir.join("kimi.log");
        let model = self.resolve_model();
        // HEAD before/after bounds the work this call committed (progress guard).
        let before_sha = git::head_sha(ws.repo_root()).unwrap_or_default();
        let sessions_dir = kimi_sessions_dir();
        let snapshot = || {
            sessions_dir
                .as_deref()
                .map(|d| list_session_files(d, "jsonl", true, Some("wire")))
                .unwrap_or_default()
        };

        let run = || {
            let skills_dir = materialize_kimi_skills(ws)?;
            let cmd = build_kimi_command(&model, ws.repo_root(), &skills_dir);
            info!(model = %model, "executing with kimi --print");
            let timeout = self.budget.timeout(ralphy_core::UNBOUNDED_ISSUE_HORIZON);
            let before = snapshot();
            let r = self.run_kimi(cmd, PROMPT_EXECUTE, timeout)?;
            let after = snapshot();
            Ok((r, (before, after)))
        };

        let ralphy_dir = ws.ralphy_dir();
        let (r, (before, after)) = run_exec_session(
            ExecCfg {
                ralphy_dir: &ralphy_dir,
                run_dir: &self.run_dir,
                log_path: &log_path,
                auth_msg: KIMI_AUTH_ERROR_MSG,
            },
            run,
            is_kimi_auth_error,
        )?;

        let after_sha = git::head_sha(ws.repo_root()).unwrap_or_default();
        let committed = before_sha != after_sha;
        let final_text = kimi_final_text(&r.stdout);
        let outcome = classify_kimi_outcome(
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
            "kimi execution ended"
        );
        Ok(Execution {
            outcome,
            usage: fold_wire_usage(&before, &after, Some(model)),
            session_id: wire_session_id(&before, &after),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::time::Duration;

    #[test]
    fn kimi_agent_is_a_dyn_agent() {
        let agent = KimiAgent::new(None, PathBuf::from("/run"));
        let _as_dyn: &dyn Agent = &agent;
    }

    #[test]
    fn kimi_honours_max_minutes_per_issue() {
        assert_eq!(
            KimiAgent::new(None, PathBuf::from("/run"))
                .budget
                .max_minutes_per_issue,
            ralphy_core::DEFAULT_MAX_MINUTES_PER_ISSUE
        );
        let a = KimiAgent::new(None, PathBuf::from("/run")).with_max_minutes_per_issue(120);
        assert_eq!(a.budget.max_minutes_per_issue, 120);
        let short = KimiAgent::new(None, PathBuf::from("/run")).with_max_minutes_per_issue(1);
        let long = KimiAgent::new(None, PathBuf::from("/run")).with_max_minutes_per_issue(1000);
        assert!(long.issue_deadline() > short.issue_deadline());
        let rd = Instant::now() + Duration::from_secs(1);
        let clamped = KimiAgent::new(None, PathBuf::from("/run"))
            .with_max_minutes_per_issue(1000)
            .with_run_deadline(Some(rd));
        assert!(clamped.issue_deadline() <= rd);
    }

    #[test]
    fn kimi_zero_minutes_disables_the_per_issue_cap() {
        let uncapped = KimiAgent::new(None, PathBuf::from("/run")).with_max_minutes_per_issue(0);
        let capped = KimiAgent::new(None, PathBuf::from("/run")).with_max_minutes_per_issue(1000);
        assert!(uncapped.issue_deadline() > capped.issue_deadline());

        let rd = Instant::now() + Duration::from_secs(1);
        let bounded = KimiAgent::new(None, PathBuf::from("/run"))
            .with_max_minutes_per_issue(0)
            .with_run_deadline(Some(rd));
        assert!(bounded.issue_deadline() <= rd);
    }

    #[test]
    fn resolve_model_override_wins() {
        let overridden = KimiAgent::new(Some("x".into()), PathBuf::from("/run"));
        assert_eq!(overridden.resolve_model(), "x");
        let default = KimiAgent::new(None, PathBuf::from("/run"));
        assert_eq!(default.resolve_model(), DEFAULT_KIMI_MODEL);
    }

    #[test]
    fn plan_charter_file_carries_full_prompt() {
        let base = std::env::temp_dir().join(format!("ralphy-kimi-charter-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        let ws = Workspace::new(&base);
        fs::create_dir_all(ws.ralphy_dir()).unwrap();

        fs::write(ws.plan_charter_path(), PROMPT_PLAN_KIMI).unwrap();
        assert_eq!(
            fs::read_to_string(ws.plan_charter_path()).unwrap(),
            PROMPT_PLAN_KIMI
        );
        assert!(ralphy_adapter_support::PLAN_CHARTER.len() * 50 < PROMPT_PLAN_KIMI.len());

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn prompt_plan_kimi_has_no_execution_model_line() {
        assert!(
            !PROMPT_PLAN_KIMI.contains("## Execution model"),
            "the Kimi plan prompt must drop the complexity tier line (D3/D8)"
        );
    }

    #[test]
    fn prompt_plan_kimi_keeps_reviewer_step() {
        assert!(
            PROMPT_PLAN_KIMI.contains("reviewer"),
            "planning prompt must reference the reviewer skill"
        );
        let lower = PROMPT_PLAN_KIMI.to_lowercase();
        assert!(
            lower.contains("only") && lower.contains("commits you made"),
            "reviewer step must scope to this issue's own commits"
        );
    }

    #[test]
    fn prompt_plan_kimi_carries_finalize_trailer() {
        assert!(
            PROMPT_PLAN_KIMI.contains("<!-- ralphy-plan: issue=<N> -->"),
            "planning prompt must instruct writing the exact finalized-plan trailer"
        );
    }
}
