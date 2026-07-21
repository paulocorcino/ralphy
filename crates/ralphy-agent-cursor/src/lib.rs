//! The Cursor Agent CLI adapter: drives headless `cursor-agent` behind the core
//! [`Agent`] contract. Everything Cursor-specific — the binary, the argv, the
//! record-stream fold, and the signal→[`Outcome`] mapping — is confined here.
//! See docs/adr/0042.
//!
//! Like Codex, Kimi, OpenCode and Copilot (and unlike Claude's live PTY session),
//! Cursor needs no interactive session: `plan` and `execute` both pipe the charter
//! on **stdin** (ADR-0042 D2).
//!
//! Two of this adapter's behaviours exist to refuse a vendor default, and they are
//! not optional garnish on the run — they gate it:
//! - [`guards`] refuses to spawn in a repository that has not opted out of the
//!   codebase upload (D6), and
//! - every invocation runs against a scratch `CURSOR_CONFIG_DIR` seeded from the
//!   operator's own, so a `--model` never reassigns the default model of their
//!   interactive Cursor sessions (D4/D17).
//!
//! Token usage, skills materialization and the one-shot verbs are each their own
//! slice of #242 and are deliberately absent here.

use std::fs;
use std::path::PathBuf;
use std::time::Instant;

use anyhow::{Context, Result};
use ralphy_adapter_support::{
    run_exec_session, run_plan_session, ExecCfg, IssueBudget, PlanCfg, PROMPT_EXECUTE,
};
use ralphy_core::{git, plan, Agent, Execution, Issue, Outcome, Plan, Workspace};
use tracing::info;

mod auth;
mod command;
mod guards;
mod outcome;
mod settings;

/// Whether the operator is logged into Cursor, from the vendor's own structured
/// answer (ADR-0042 D8) — what `ralphy init`'s gate reports.
pub use auth::{cursor_status_verdict, probe_cursor_login, CURSOR_AUTH_ERROR_MSG};

/// Locating the vendor's binary, which is on `PATH` on neither platform
/// (ADR-0042 D14) — `ralphy init`'s presence gate goes through this.
pub use command::locate_cursor;

/// Persisted settings for `--agent cursor` (ADR-0042 D6). See [`CursorSettings`].
pub use settings::CursorSettings;

use command::{build_cursor_command, mint_session_id};
use outcome::{classify_cursor_outcome, fold_cursor_stream};

/// `false` (ADR-0042 D15): no attachment channel appears anywhere in Cursor's
/// headless surface, so a triage attachment fetched per ADR-0025 §4 has no
/// delivery path on this vendor.
pub const ACCEPTS_IMAGES: bool = false;

/// The Cursor planning prompt, embedded so the binary is self-contained as a
/// global tool. Assembled from `assets/prompts/plan/template.md` +
/// `overlay.cursor.md`; the single source of truth lives at `assets/prompts/`.
const PROMPT_PLAN_CURSOR: &str = include_str!("../../../assets/prompts/prompt.plan.cursor.md");

/// The scratch configuration directory's name under the run directory. Under
/// `run_dir` rather than a `tempfile` so the artifact is inspectable and its
/// lifetime is the run's (D17).
const CONFIG_DIR_NAME: &str = "cursor-config";

/// The two phases a `CursorAgent` drives, each with its own model source.
#[derive(Clone, Copy)]
enum Phase {
    Plan,
    Execute,
}

/// Drives the `cursor-agent` CLI. `exec_model` is the operator override for
/// `execute()` (set via `new`); `plan_model` is the override for `plan()` (set via
/// `with_plan_model`). `None` on either does NOT omit `--model` — it sends
/// `auto`, because on this vendor an absent flag means "whatever the last
/// invocation left behind" (ADR-0042 D4).
pub struct CursorAgent {
    exec_model: Option<String>,
    plan_model: Option<String>,
    /// D6's escape hatch: run even where the repository has not opted out of the
    /// codebase upload.
    allow_indexing: bool,
    run_dir: PathBuf,
    budget: IssueBudget,
}

impl CursorAgent {
    pub fn new(model: Option<String>, run_dir: PathBuf) -> Self {
        Self {
            exec_model: model,
            plan_model: None,
            allow_indexing: false,
            run_dir,
            budget: IssueBudget::new(ralphy_core::DEFAULT_MAX_MINUTES_PER_ISSUE),
        }
    }

    /// Set the model override used for `plan()`.
    pub fn with_plan_model(mut self, model: Option<String>) -> Self {
        self.plan_model = model;
        self
    }

    /// Hand the operator back the codebase indexing D6 refuses by default
    /// (persisted as `cursor.allow_codebase_indexing_i_understand_the_risk`).
    /// Ralphy never denies a capability — it denies a *silent* one.
    pub fn with_allow_indexing(mut self, allow: bool) -> Self {
        self.allow_indexing = allow;
        self
    }

    /// Set the per-issue wall-clock budget in minutes.
    pub fn with_max_minutes_per_issue(mut self, minutes: u64) -> Self {
        self.budget = self.budget.with_max_minutes_per_issue(minutes);
        self
    }

    /// Set the idle watchdog window in minutes: reap the child after that long
    /// with no output at all. `0` disables it (docs/adr/0038).
    ///
    /// The default is `ralphy_core::DEFAULT_IDLE_MINUTES` and stays there
    /// deliberately: this vendor opens with ~8 s of silence and shows inter-record
    /// gaps up to ~7.4 s (D3), so a watchdog in seconds would reap healthy runs.
    pub fn with_idle_minutes(mut self, minutes: u64) -> Self {
        self.budget = self.budget.with_idle_minutes(minutes);
        self
    }

    /// Set the run's global wall-clock deadline (from `--deadline-hours`).
    pub fn with_run_deadline(mut self, run_deadline: Option<Instant>) -> Self {
        self.budget = self.budget.with_run_deadline(run_deadline);
        self
    }

    /// The run's scratch `CURSOR_CONFIG_DIR` (D17). One per run, re-seeded before
    /// every spawn, and never copied back.
    pub(crate) fn config_dir(&self) -> PathBuf {
        self.run_dir.join(CONFIG_DIR_NAME)
    }

    fn phase_model(&self, phase: Phase) -> Option<&str> {
        match phase {
            Phase::Plan => self.plan_model.as_deref(),
            Phase::Execute => self.exec_model.as_deref(),
        }
    }

    /// D10: the minted id is ADOPTED, not documented as adopted. A `system/init`
    /// that echoes a different id means every later store lookup would address
    /// another session, so it is a hard error rather than a warning.
    fn verify_session_adoption(minted: &str, observed: Option<&str>) -> Result<()> {
        match observed {
            Some(seen) if seen != minted => anyhow::bail!(
                "cursor did not adopt the session id ralphy minted: sent {minted}, \
                 `system/init` reported {seen}"
            ),
            _ => Ok(()),
        }
    }

    /// The deadline oracle the budget tests assert against.
    #[cfg(test)]
    fn issue_deadline(&self) -> Instant {
        self.budget.deadline(ralphy_core::UNBOUNDED_ISSUE_HORIZON)
    }
}

impl Agent for CursorAgent {
    fn name(&self) -> &'static str {
        "cursor"
    }

    fn plan(&self, issue: &Issue, ws: &Workspace) -> Result<Plan> {
        let plan_path = ws.plan_path();
        let log_path = self.run_dir.join("cursor.log");
        let session_id = mint_session_id();
        let model = self.phase_model(Phase::Plan);

        let run = || {
            let cmd = build_cursor_command(&session_id, model, ws.repo_root(), &self.config_dir());
            ralphy_core::emit::planning("cursor", model.unwrap_or(""), "");
            // Clock the budget at the spawn, not method entry, so the run_deadline
            // clamp isn't eroded by the preceding dir setup.
            let timeout = self.budget.timeout(ralphy_core::UNBOUNDED_ISSUE_HORIZON);
            let r = self.run_cursor(
                cmd,
                ralphy_adapter_support::PLAN_CHARTER,
                timeout,
                ws.repo_root(),
            )?;
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
                charter_body: PROMPT_PLAN_CURSOR,
                log_path: &log_path,
                auth_msg: CURSOR_AUTH_ERROR_MSG,
                no_plan_msg: "cursor produced no plan",
            },
            run,
            auth::is_cursor_auth_error,
            // D13 is open: no limit signature has ever been observed on this
            // vendor, so a limit surfaces as an ordinary failure rather than a
            // guessed phrase match that would park the queue on a false positive.
            |_log| None,
        )?;

        if let Some((r, _)) = session.as_ref() {
            let fold = fold_cursor_stream(&r.stdout);
            Self::verify_session_adoption(&session_id, fold.session_id.as_deref())?;
            note_degraded(&fold);
        }

        let md = fs::read_to_string(&plan_path).context("reading the written plan.md")?;
        Ok(Plan {
            open_steps: plan::count_open_steps(&md),
            // Cursor's model axis is a plan entitlement, not a complexity tier (D5).
            recommended_model: None,
            path: plan_path,
            // Usage accounting is its own slice of #242: `result.usage` is the only
            // accounting this vendor has (D11), and reporting a number without that
            // slice's sum-vs-keep-last fixture test is exactly the failure ADR-0040
            // C6 warns about.
            usage: Default::default(),
            // `None` = a finalized plan was RESUMED and no `cursor-agent` ran.
            session_id: session.map(|_| session_id),
        })
    }

    fn execute(&self, _plan: &Plan, ws: &Workspace) -> Result<Execution> {
        let log_path = self.run_dir.join("cursor.log");
        let session_id = mint_session_id();
        // HEAD before/after bounds the work this call committed. Load-bearing: the
        // stream's only progress fields belong to the edit tool, so work done
        // through the shell reports zero (spike §2).
        let before_sha = git::head_sha(ws.repo_root()).unwrap_or_default();
        let model = self.phase_model(Phase::Execute);

        let run = || {
            let cmd = build_cursor_command(&session_id, model, ws.repo_root(), &self.config_dir());
            ralphy_core::emit::executing("cursor", 0, model.unwrap_or(""), "");
            let timeout = self.budget.timeout(ralphy_core::UNBOUNDED_ISSUE_HORIZON);
            let r = self.run_cursor(cmd, PROMPT_EXECUTE, timeout, ws.repo_root())?;
            Ok((r, ()))
        };

        let ralphy_dir = ws.ralphy_dir();
        let (r, ()) = run_exec_session(
            ExecCfg {
                ralphy_dir: &ralphy_dir,
                run_dir: &self.run_dir,
                log_path: &log_path,
                auth_msg: CURSOR_AUTH_ERROR_MSG,
            },
            run,
            auth::is_cursor_auth_error,
        )?;

        let after_sha = git::head_sha(ws.repo_root()).unwrap_or_default();
        let committed = before_sha != after_sha;
        let fold = fold_cursor_stream(&r.stdout);
        Self::verify_session_adoption(&session_id, fold.session_id.as_deref())?;
        note_degraded(&fold);
        let outcome: Outcome =
            classify_cursor_outcome(&fold, r.exited_cleanly, r.timed_out, committed, r.exit_code);
        info!(
            ?outcome,
            exited_cleanly = r.exited_cleanly,
            timed_out = r.timed_out,
            exit_code = ?r.exit_code,
            committed,
            saw_envelope = fold.saw_envelope,
            // D3 rule 2: zero records + a non-zero exit is a PREFLIGHT rejection,
            // not a truncation — the two look identical without this field.
            saw_no_records = fold.saw_no_records(),
            "cursor execution ended"
        );
        Ok(Execution {
            outcome,
            // See `plan`: usage accounting is its own slice of #242.
            usage: Default::default(),
            session_id: Some(session_id),
        })
    }
}

/// Surface a green run that quietly did less (D7): a failed tool call, or one the
/// operator's own `permissions.deny` blocked. Never changes the outcome — the
/// vendor reports `success` for both, and that is its answer, not Ralphy's.
fn note_degraded(fold: &outcome::CursorFold) {
    if let Some(note) = fold.degraded_note() {
        tracing::warn!("cursor: {note}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn accepts_images_is_false() {
        // Read through a binding: a bare `assert!(!CONST)` is constant-folded and
        // clippy rejects it, but the invariant is still worth pinning here — the
        // CLI's onboarding gate asserts the same const from the other side.
        let accepts: bool = ACCEPTS_IMAGES;
        assert!(
            !accepts,
            "ADR-0042 D15: no attachment channel exists in the headless surface"
        );
    }

    #[test]
    fn cursor_agent_is_a_dyn_agent() {
        let agent = CursorAgent::new(None, PathBuf::from("/run"));
        let _as_dyn: &dyn Agent = &agent;
        assert_eq!(agent.name(), "cursor");
    }

    #[test]
    fn cursor_honours_max_minutes_per_issue() {
        assert_eq!(
            CursorAgent::new(None, PathBuf::from("/run"))
                .budget
                .max_minutes_per_issue,
            ralphy_core::DEFAULT_MAX_MINUTES_PER_ISSUE
        );
        let short = CursorAgent::new(None, PathBuf::from("/run")).with_max_minutes_per_issue(1);
        let long = CursorAgent::new(None, PathBuf::from("/run")).with_max_minutes_per_issue(1000);
        assert!(long.issue_deadline() > short.issue_deadline());
        let rd = Instant::now() + Duration::from_secs(1);
        let clamped = CursorAgent::new(None, PathBuf::from("/run"))
            .with_max_minutes_per_issue(1000)
            .with_run_deadline(Some(rd));
        assert!(clamped.issue_deadline() <= rd);
    }

    /// D17: the scratch dir is per RUN and under the run dir, never the operator's.
    #[test]
    fn the_config_dir_lives_under_the_run_dir() {
        let agent = CursorAgent::new(None, PathBuf::from("/run/abc"));
        assert_eq!(agent.config_dir(), PathBuf::from("/run/abc/cursor-config"));
    }

    /// D10: adoption is verified, not assumed. A mismatch is an error because every
    /// later store lookup would otherwise address another session.
    #[test]
    fn a_session_id_mismatch_is_an_error() {
        let minted = "868f1553-01ac-4335-89c6-6c1f101d6009";
        assert!(CursorAgent::verify_session_adoption(minted, Some(minted)).is_ok());
        // No `system/init` at all (a truncated stream) is not a mismatch — the
        // missing envelope is what classifies that run.
        assert!(CursorAgent::verify_session_adoption(minted, None).is_ok());
        let err = CursorAgent::verify_session_adoption(minted, Some("other-id"))
            .expect_err("a different id must abort");
        assert!(err.to_string().contains("other-id"), "{err}");
    }

    /// The default hatch is OFF: a fresh agent refuses an un-opted-out repository.
    #[test]
    fn indexing_is_refused_by_default_and_reachable_on_request() {
        assert!(!CursorAgent::new(None, PathBuf::from("/run")).allow_indexing);
        assert!(
            CursorAgent::new(None, PathBuf::from("/run"))
                .with_allow_indexing(true)
                .allow_indexing
        );
    }

    /// D2's reason: the charter alone is within ~30 % of the Windows ~32 KB argv
    /// ceiling before the issue body is appended, so stdin is the only safe channel.
    /// The floor pins the ORDER of magnitude, not a byte count every prompt edit
    /// would churn.
    #[test]
    fn plan_charter_exceeds_argv_safe_size() {
        assert!(
            PROMPT_PLAN_CURSOR.len() > 23_000,
            "charter is {} bytes",
            PROMPT_PLAN_CURSOR.len()
        );
    }

    #[test]
    fn prompt_plan_cursor_carries_finalize_trailer() {
        assert!(
            PROMPT_PLAN_CURSOR.contains("<!-- ralphy-plan: issue=<N> -->"),
            "planning prompt must instruct writing the exact finalized-plan trailer"
        );
    }

    /// D9: the vendor's native plan mode is hard read-only and overrides the
    /// charter, so the overlay must tell the planner to write the file itself.
    #[test]
    fn prompt_plan_cursor_requires_the_planner_to_write_the_file() {
        assert!(
            PROMPT_PLAN_CURSOR.contains("you MUST write `.ralphy/plan.md` yourself"),
            "D9: the planner writes its own plan on this vendor"
        );
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
}
