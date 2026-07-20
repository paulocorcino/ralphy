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
//! `--session-id` ([`usage`], ADR-0041 D10). Skills are materialized into
//! `.agents/skills` and their load receipt asserted by [`skills`] (ADR-0041 D9).
//! The one-shot `init`/`triage`/`consolidate` flows go through [`tasks`].

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
mod catalog;
mod command;
mod effort;
mod guards;
mod outcome;
mod settings;
mod skills;
mod tasks;
mod usage;

/// The free model catalog the preflight learns from one `copilot` subprocess
/// (ADR-0041; issue #231). `fetch_catalog` doubles as the login/entitlement probe.
pub use catalog::{
    fetch_catalog, parse_catalog, CopilotCatalog, CopilotModel, CopilotPrices,
    COPILOT_CATALOG_ERROR_MSG, COPILOT_PROBE_BILLED_MSG,
};

/// Persisted per-phase model overrides (ADR-0041 D4). See [`CopilotSettings`].
pub use settings::CopilotSettings;

/// Membership test for the reasoning-effort vocabulary, so `ralphy config set`
/// can refuse a typo without the ORDERING leaving this crate (ADR-0041 D5a).
pub use effort::is_known_effort;

/// `true` (ADR-0041 D12): `copilot --attachment <path>` attaches an image or
/// native document to the initial prompt in non-interactive mode, so a triage
/// attachment fetched per ADR-0025 §4 has a real delivery channel.
/// `build_copilot_command` emits one `--attachment <path>` per image; the
/// triage/`tasks.rs` slice is what will supply a non-empty slice.
pub const ACCEPTS_IMAGES: bool = true;

use auth::{is_copilot_auth_error, COPILOT_AUTH_ERROR_MSG};
use command::{build_copilot_command, mint_session_id};
use outcome::{classify_copilot_outcome, copilot_final_text};
use skills::materialize_copilot_skills;
pub use tasks::{consolidate_knowledge, diagnose_repo, draft_issues, triage_issues};
use usage::copilot_usage;

/// The Copilot planning prompt, embedded so the binary is self-contained as a
/// global tool. A variant of `prompt.plan.md` with no `## Execution model` tier
/// line (Copilot's model is the operator's account default, ADR-0041 D6). Copied
/// to `.ralphy/plan-charter.md` for the session to read; only a one-line pointer
/// is piped on stdin. Single source of truth lives at `assets/prompts/`.
const PROMPT_PLAN_COPILOT: &str = include_str!("../../../assets/prompts/prompt.plan.copilot.md");

/// The two phases a `CopilotAgent` drives, each with its own model source
/// (`--plan-model` / persisted `copilot.plan_model` for `Plan`; `--exec-model` /
/// persisted `copilot.exec_model` for `Execute`, ADR-0041 D4).
#[derive(Clone, Copy)]
enum Phase {
    Plan,
    Execute,
}

/// Drives the `copilot` CLI. `exec_model` is the operator override for
/// `execute()` (set via `new`); `plan_model` is the override for `plan()` (set
/// via `with_plan_model`). Resolution order for each phase, outside this crate:
/// the phase's `--plan-model`/`--exec-model` flag, then the persisted
/// `copilot.plan_model`/`copilot.exec_model`, then omission — omitting `--model`
/// selects the account's current default, the correct default rather than a
/// degraded fallback (ADR-0041 D4). `run_dir` is where the captured logs live;
/// `max_minutes_per_issue` is the per-issue wall budget, clamped to
/// `run_deadline` when the run carries a global deadline.
pub struct CopilotAgent {
    exec_model: Option<String>,
    plan_model: Option<String>,
    exec_effort: Option<String>,
    plan_effort: Option<String>,
    /// The free model catalog, fetched at most once and ONLY when a phase actually
    /// requested an effort — a default run must spawn no probe (see [`Self::catalog`]).
    catalog: std::sync::OnceLock<Option<CopilotCatalog>>,
    /// The D7 escape hatch (`copilot.allow_builtin_mcp_servers_i_understand_the_risk`):
    /// drops `--disable-builtin-mcps` from the argv AND skips the receipt guard.
    allow_builtin_mcps: bool,
    run_dir: PathBuf,
    budget: IssueBudget,
}

impl CopilotAgent {
    pub fn new(model: Option<String>, run_dir: PathBuf) -> Self {
        Self {
            exec_model: model,
            plan_model: None,
            exec_effort: None,
            plan_effort: None,
            catalog: std::sync::OnceLock::new(),
            allow_builtin_mcps: false,
            run_dir,
            budget: IssueBudget::new(ralphy_core::DEFAULT_MAX_MINUTES_PER_ISSUE),
        }
    }

    /// Set the model override used for `plan()` (mirrors `ClaudeAgent`'s builder
    /// style; ADR-0041 D4).
    pub fn with_plan_model(mut self, model: Option<String>) -> Self {
        self.plan_model = model;
        self
    }

    /// Set the reasoning effort requested for `plan()` (ADR-0041 D5a). The value
    /// is the operator's REQUEST, not what is sent: it is clamped per model.
    pub fn with_plan_effort(mut self, effort: Option<String>) -> Self {
        self.plan_effort = effort;
        self
    }

    /// Set the reasoning effort requested for `execute()` (ADR-0041 D5a).
    pub fn with_exec_effort(mut self, effort: Option<String>) -> Self {
        self.exec_effort = effort;
        self
    }

    /// Hand Copilot's builtin MCP surface back to the operator (ADR-0041 D7's
    /// escape hatch). `true` both drops `--disable-builtin-mcps` and skips the
    /// in-band receipt guard — suppressing the check while still passing the flag
    /// would grant the operator nothing.
    pub fn with_allow_builtin_mcps(mut self, allow: bool) -> Self {
        self.allow_builtin_mcps = allow;
        self
    }

    /// D7's in-band receipt guard, the single seam both phases call. `Err` aborts
    /// the run: a connected builtin MCP is a safety-envelope violation, not a work
    /// outcome. Skipped entirely under the escape hatch.
    pub(crate) fn check_builtin_mcps(&self, stdout: &str, require_receipt: bool) -> Result<()> {
        if self.allow_builtin_mcps {
            return Ok(());
        }
        match guards::builtin_mcp_violation(stdout, require_receipt) {
            Some(msg) => Err(anyhow::anyhow!("{msg}")),
            None => Ok(()),
        }
    }

    /// D9's in-band load receipt, the single seam both phases call. `Err` aborts
    /// the run: a charter whose skill invocations silently do nothing is a run that
    /// only looks like it worked. No escape hatch — unlike D7's builtin MCPs, a
    /// missing skill grants the operator no capability worth opting into.
    pub(crate) fn check_skills_loaded(
        &self,
        stdout: &str,
        required: &[String],
        require_receipt: bool,
    ) -> Result<()> {
        match skills::skills_load_violation(stdout, required, require_receipt) {
            Some(msg) => Err(anyhow::anyhow!("{msg}")),
            None => Ok(()),
        }
    }

    fn phase_model(&self, phase: Phase) -> Option<&str> {
        match phase {
            Phase::Plan => self.plan_model.as_deref(),
            Phase::Execute => self.exec_model.as_deref(),
        }
    }

    fn phase_effort(&self, phase: Phase) -> Option<&str> {
        match phase {
            Phase::Plan => self.plan_effort.as_deref(),
            Phase::Execute => self.exec_effort.as_deref(),
        }
    }

    /// The model catalog, fetched lazily and memoized for the agent's lifetime.
    ///
    /// Callers MUST reach this only once they know an effort was requested (guard
    /// with `phase_effort(..).and_then(..)`), so a default run pays nothing. The
    /// probe itself is free — zero model calls (#231) — but it is still a
    /// subprocess. A failed fetch memoizes `None`, which degrades to omitting
    /// `--effort` rather than failing the run.
    fn catalog(&self) -> Option<&CopilotCatalog> {
        self.catalog
            .get_or_init(|| match catalog::fetch_catalog() {
                Ok(c) => Some(c),
                Err(e) => {
                    tracing::warn!(error = %e, "no Copilot catalog: --effort will be omitted");
                    None
                }
            })
            .as_ref()
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

/// D5a's post-hoc verification: compare what the argv asked for against what the
/// vendor actually recorded for the session, and `warn!` on a divergence. Never
/// changes a return value or fails a run — the request is not the truth, but a
/// silent divergence is what would hide a clamp bug.
/// The early return is load-bearing, not style: Rust evaluates both arguments
/// before `effort_mismatch` can short-circuit on its own `?`, and
/// `copilot_recorded_effort` COPIES the whole vendor session store. Without the
/// guard a default run — which requested nothing — would pay that copy twice per
/// issue.
fn warn_effort_mismatch(requested: Option<&str>, session_id: &str) {
    warn_effort_mismatch_with(requested, session_id, || {
        usage::copilot_recorded_effort(session_id)
    })
}

/// The reader is a closure so a test can COUNT its invocations — the "a default
/// run reads no store" property is otherwise unobservable from outside.
fn warn_effort_mismatch_with(
    requested: Option<&str>,
    session_id: &str,
    read_recorded: impl FnOnce() -> Option<String>,
) {
    let Some(requested) = requested else {
        return;
    };
    if let Some(msg) = usage::effort_mismatch(Some(requested), read_recorded().as_deref()) {
        tracing::warn!(session_id, "{}", msg);
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

        // `None` (the default) omits `--model` entirely, which selects the
        // account's own default — the correct default, not a fallback (D4).
        let model = self.phase_model(Phase::Plan);
        // `and_then`, never an unconditional `self.catalog()` binding: the probe
        // must not be spawned when no effort was requested (D5a).
        let effort = self
            .phase_effort(Phase::Plan)
            .and_then(|e| effort::resolve_effort(Some(e), model, self.catalog()));

        // Hoisted ABOVE the closure (unlike Codex, which materializes inside it):
        // `required` is read by the D9 guard after the session wrapper returns, and
        // the closure is `Fn`, so it borrows this rather than producing it.
        // Materializing here still precedes every `copilot` spawn.
        let required = materialize_copilot_skills(ws)?;

        let run = || {
            let cmd = build_copilot_command(
                &session_id,
                model,
                effort.as_deref(),
                ws.repo_root(),
                self.allow_builtin_mcps,
                &[],
            );
            ralphy_core::emit::planning(
                "copilot",
                model.unwrap_or(""),
                effort.as_deref().unwrap_or(""),
            );
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

        // D7's receipt guard runs AFTER `run_plan_session` returns, never inside
        // `run`: the closure's error would pre-empt the auth and plan-limit
        // handlers the wrapper applies to `r.log`, turning a logged-out run into
        // "MCP receipt missing".
        if let Some((r, _)) = session.as_ref() {
            self.check_builtin_mcps(&r.stdout, r.exited_cleanly)
                .map_err(|e| anyhow::anyhow!("{e} (see {})", log_path.display()))?;
            // Cross-path invariant: the SAFETY receipt (D7) keeps precedence over
            // the CAPABILITY receipt (D9) on every return path.
            self.check_skills_loaded(&r.stdout, &required, r.exited_cleanly)
                .map_err(|e| anyhow::anyhow!("{e} (see {})", log_path.display()))?;
        }

        let md = fs::read_to_string(&plan_path).context("reading the written plan.md")?;
        // Only when a `copilot` process actually ran: a RESUMED finalized plan
        // wrote no rows under this session id, so there is nothing to verify.
        if session.is_some() {
            warn_effort_mismatch(effort.as_deref(), &session_id);
        }
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

        let model = self.phase_model(Phase::Execute);
        let effort = self
            .phase_effort(Phase::Execute)
            .and_then(|e| effort::resolve_effort(Some(e), model, self.catalog()));

        // See `plan`: hoisted so the D9 guard can read it after the wrapper returns.
        let required = materialize_copilot_skills(ws)?;

        let run = || {
            let cmd = build_copilot_command(
                &session_id,
                model,
                effort.as_deref(),
                ws.repo_root(),
                self.allow_builtin_mcps,
                &[],
            );
            ralphy_core::emit::executing(
                "copilot",
                0,
                model.unwrap_or(""),
                effort.as_deref().unwrap_or(""),
            );
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

        // Same ordering invariant as `plan`: after the session wrapper, so
        // auth errors keep precedence over the receipt verdict (D7). The LIMIT
        // half of that precedence is carried by `require_receipt`, not by
        // ordering: a limit is classified below, so fail-closing here on a run
        // that died before emitting the receipt would overwrite `Limit`/`Timeout`
        // with "receipt missing". A CONNECTED server still fails unconditionally.
        self.check_builtin_mcps(&r.stdout, r.exited_cleanly)
            .map_err(|e| anyhow::anyhow!("{e} (see {})", log_path.display()))?;
        // D7 before D9 here too: the safety receipt keeps precedence.
        self.check_skills_loaded(&r.stdout, &required, r.exited_cleanly)
            .map_err(|e| anyhow::anyhow!("{e} (see {})", log_path.display()))?;

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
        warn_effort_mismatch(effort.as_deref(), &session_id);
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

    /// The hatch is the ONLY thing that turns a connected builtin server from a
    /// run-failing violation into a pass — the same stream, the two agents.
    #[test]
    fn escape_hatch_suppresses_the_connected_failure() {
        let stream = concat!(
            r#"{"type":"session.mcp_servers_loaded","data":{"servers":[{"name":"github-mcp-server","status":"connected","source":"builtin","transport":"http"}]},"ephemeral":true}"#,
            "\n"
        );
        let strict = CopilotAgent::new(None, PathBuf::from("/run"));
        let err = strict
            .check_builtin_mcps(stream, true)
            .expect_err("a connected builtin must fail the run by default");
        assert!(err.to_string().contains("github-mcp-server"), "{err}");

        let permissive =
            CopilotAgent::new(None, PathBuf::from("/run")).with_allow_builtin_mcps(true);
        assert!(
            permissive.check_builtin_mcps(stream, true).is_ok(),
            "the operator's explicit hatch must suppress the failure"
        );
        // …and the hatch does not blanket-suppress: it is not a "skip all checks"
        // switch for a stream that never carried a receipt either way.
        assert!(permissive.check_builtin_mcps("", true).is_ok());
        assert!(
            strict.check_builtin_mcps("", true).is_err(),
            "an absent receipt still fails closed by default"
        );
    }

    /// The guard is only worth its tests if it is actually WIRED. Every D7 test
    /// calls the predicate directly, and no test here builds a `Workspace`, so
    /// `plan`/`execute` are invisible to the suite — deleting both call sites
    /// would leave everything green and silently turn ADR-0041 D7 into a no-op.
    /// This pins the call sites in the source, the same mechanism
    /// `no_direct_command_new` and `runstate/capture.rs` use. Fragments are
    /// assembled with `concat!` so the assertion cannot match ITSELF.
    #[test]
    fn the_receipt_guard_is_wired_into_both_phases() {
        let src = include_str!("lib.rs");
        let call = concat!("self.check_builtin_mcps(", "&r.stdout, r.exited_cleanly)");
        assert_eq!(
            src.matches(call).count(),
            2,
            "D7's guard must be called on BOTH the plan and the execute path"
        );
    }

    /// The D9 seam itself, not just its source-text pin: replacing
    /// `check_skills_loaded`'s body with `Ok(())` must RED something. Mirrors the
    /// D7 seam test above — a pin alone counts substrings and cannot see a gutted
    /// body. Also proves D9 carries NO escape hatch: the D7 hatch must not
    /// suppress a missing skill.
    #[test]
    fn check_skills_loaded_fails_a_run_missing_a_ralphy_skill() {
        let required = vec!["reviewer".to_string(), "staged-plan".to_string()];
        let missing = concat!(
            r#"{"type":"session.skills_loaded","data":{"skills":[{"name":"reviewer"}]},"ephemeral":true}"#,
            "\n"
        );
        let agent = CopilotAgent::new(None, PathBuf::from("/run"));
        let err = agent
            .check_skills_loaded(missing, &required, true)
            .expect_err("a missing ralphy skill must fail the run");
        assert!(err.to_string().contains("staged-plan"), "{err}");

        // The D7 hatch is scoped to D7: it must NOT suppress the capability guard.
        let permissive =
            CopilotAgent::new(None, PathBuf::from("/run")).with_allow_builtin_mcps(true);
        assert!(
            permissive
                .check_skills_loaded(missing, &required, true)
                .is_err(),
            "D9 has no escape hatch; the D7 hatch must not suppress it"
        );

        // A receipt listing both passes through the seam.
        let complete = concat!(
            r#"{"type":"session.skills_loaded","data":{"skills":[{"name":"reviewer"},{"name":"staged-plan"}]},"ephemeral":true}"#,
            "\n"
        );
        assert!(agent.check_skills_loaded(complete, &required, true).is_ok());
    }

    /// Same reasoning as D7's pin, for D9: no test here constructs a `Workspace`,
    /// so deleting either call site would leave the suite green and ADR-0041 D9 a
    /// silent no-op. Pins both the materialization and the receipt assertion.
    #[test]
    fn the_skills_guard_is_wired_into_both_phases() {
        let src = include_str!("lib.rs");
        let call = concat!(
            "self.check_skills_loaded(",
            "&r.stdout, &required, r.exited_cleanly)"
        );
        assert_eq!(
            src.matches(call).count(),
            2,
            "D9's guard must be called on BOTH the plan and the execute path"
        );
        assert_eq!(
            src.matches(concat!("materialize_copilot", "_skills(ws)?"))
                .count(),
            2,
            "skills must be materialized on BOTH the plan and the execute path"
        );
    }

    fn argv(cmd: &std::process::Command) -> Vec<String> {
        cmd.get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn plan_phase_uses_plan_model_in_argv() {
        let agent = CopilotAgent::new(Some("exec-pin".into()), PathBuf::from("/run"))
            .with_plan_model(Some("plan-pin".into()));
        let cmd = build_copilot_command(
            "s1",
            agent.phase_model(Phase::Plan),
            None,
            std::path::Path::new("/repo"),
            false,
            &[],
        );
        let args = argv(&cmd);
        let i = args.iter().position(|a| a == "--model").unwrap();
        assert_eq!(args[i + 1], "plan-pin");
    }

    #[test]
    fn execute_phase_uses_exec_model_in_argv() {
        let agent = CopilotAgent::new(Some("exec-pin".into()), PathBuf::from("/run"))
            .with_plan_model(Some("plan-pin".into()));
        let cmd = build_copilot_command(
            "s1",
            agent.phase_model(Phase::Execute),
            None,
            std::path::Path::new("/repo"),
            false,
            &[],
        );
        let args = argv(&cmd);
        let i = args.iter().position(|a| a == "--model").unwrap();
        assert_eq!(args[i + 1], "exec-pin");
    }

    #[test]
    fn both_phases_omit_model_when_unpinned() {
        let agent = CopilotAgent::new(None, PathBuf::from("/run"));
        for phase in [Phase::Plan, Phase::Execute] {
            let cmd = build_copilot_command(
                "s1",
                agent.phase_model(phase),
                None,
                std::path::Path::new("/repo"),
                false,
                &[],
            );
            let args = argv(&cmd);
            assert!(!args.iter().any(|a| a == "--model"), "argv: {args:?}");
        }
    }

    fn fixture_catalog() -> CopilotCatalog {
        parse_catalog(
            include_str!("../fixtures/capi-models-2026-07-20.log"),
            "probe-1",
        )
        .expect("the fixture parses")
    }

    /// The end-to-end shape of D5a on the plan phase: an `xhigh` request against a
    /// model that publishes only `low/medium/high` rides the argv as `high`.
    #[test]
    fn plan_phase_clamps_its_effort_in_argv() {
        let agent = CopilotAgent::new(None, PathBuf::from("/run"))
            .with_plan_model(Some("gpt-5-mini".into()))
            .with_plan_effort(Some("xhigh".into()));
        let cat = fixture_catalog();
        let model = agent.phase_model(Phase::Plan);
        let effort = agent
            .phase_effort(Phase::Plan)
            .and_then(|e| effort::resolve_effort(Some(e), model, Some(&cat)));
        let cmd = build_copilot_command(
            "s1",
            model,
            effort.as_deref(),
            std::path::Path::new("/repo"),
            false,
            &[],
        );
        let args = argv(&cmd);
        let i = args
            .iter()
            .position(|a| a == "--effort")
            .unwrap_or_else(|| panic!("--effort missing: {args:?}"));
        assert_eq!(args[i + 1], "high");
    }

    /// The default run: no effort requested, no `--effort` token, and the catalog
    /// is never consulted (`phase_effort` short-circuits before `and_then`).
    #[test]
    fn both_phases_omit_effort_when_unset() {
        let agent = CopilotAgent::new(None, PathBuf::from("/run"));
        for phase in [Phase::Plan, Phase::Execute] {
            let effort = agent
                .phase_effort(phase)
                .and_then(|e| effort::resolve_effort(Some(e), None, None));
            assert_eq!(effort, None);
            let cmd = build_copilot_command(
                "s1",
                agent.phase_model(phase),
                effort.as_deref(),
                std::path::Path::new("/repo"),
                false,
                &[],
            );
            let args = argv(&cmd);
            assert!(!args.iter().any(|a| a == "--effort"), "argv: {args:?}");
        }
    }

    /// A default run must not touch the vendor's session store for effort: the
    /// reader COPIES the whole database, and `effort_mismatch`'s own `?` cannot
    /// prevent it — Rust evaluates arguments before the call. The counter is what
    /// makes "reads nothing" observable; without it the eager form passes too.
    #[test]
    fn no_effort_requested_reads_no_session_store() {
        use std::cell::Cell;
        let reads = Cell::new(0);
        warn_effort_mismatch_with(None, "s1", || {
            reads.set(reads.get() + 1);
            Some("high".into())
        });
        assert_eq!(reads.get(), 0, "a default run must read no store");

        warn_effort_mismatch_with(Some("high"), "s1", || {
            reads.set(reads.get() + 1);
            Some("medium".into())
        });
        assert_eq!(reads.get(), 1, "a requested effort IS verified post-hoc");
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
