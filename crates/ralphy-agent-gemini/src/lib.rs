//! The Gemini CLI adapter: drives headless `gemini` behind the core [`Agent`]
//! contract. Everything Gemini-specific — the binary, the argv, the stream-json
//! fold, the exit-code taxonomy and the policy document — is confined here.
//! See docs/adr/0043.
//!
//! Like Codex, Kimi, OpenCode, Copilot and Cursor (and unlike Claude's live PTY
//! session), Gemini needs no interactive session: `plan` and `execute` both pipe
//! the charter on **stdin** (ADR-0043 D2).
//!
//! Three behaviours here exist to refuse a vendor default, and they gate the run
//! rather than decorate it:
//! - [`root`] points every child at a configuration root Ralphy owns
//!   (`GEMINI_CLI_HOME`), so the operator's `~/.gemini` is never read or written
//!   (D4);
//! - [`policy`] hands the child a policy document on argv that always denies
//!   `invoke_agent` and imports only the operator's *restrictive* rules (D5);
//! - [`command`] scrubs every inherited authentication variable outside an
//!   explicit allowlist, so unrelated cloud tooling cannot redirect the run to
//!   another account (D7).

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Context, Result};
use ralphy_adapter_support::{
    run_exec_session, run_plan_session, ExecCfg, IssueBudget, PlanCfg, PLAN_CHARTER, PROMPT_EXECUTE,
};
use ralphy_core::{git, plan, Agent, Execution, Issue, Outcome, Plan, PlanLimit, Usage, Workspace};
use tracing::info;

mod auth;
mod command;
mod model;
mod outcome;
mod policy;
mod revocation;
mod root;
mod settings;

/// The vendor's id grammar (ADR-0043 D8): which ids may be pinned, and the
/// price-table key each one bills under — the MANDATORY transform between a
/// model id the run recorded and a `PriceTable` lookup.
pub use model::{is_pinnable_model, price_key, PINNABLE_MODELS};

/// The per-phase model pins persisted under `gemini.*` in `.ralphy/settings.json`.
pub use settings::GeminiSettings;

/// Whether the operator is authenticated, from the vendor's own exit code
/// (ADR-0043 D6) — what `ralphy init`'s gate reports.
pub use auth::{probe_gemini_login, GEMINI_AUTH_ERROR_MSG};

/// Locating the vendor's binary, which npm installs without an executable
/// extension on Windows (ADR-0043 D16) — `ralphy init`'s presence gate goes
/// through this.
pub use command::locate_gemini;

use command::{build_gemini_command, check_stdin_ceiling, mint_session_id};
use outcome::{classify_gemini_outcome, fold_gemini_stream};

/// `true` (ADR-0043 D14): the headless surface accepts image attachments via the
/// `@<path>` interpolation. Wiring a triage attachment into the prompt belongs to
/// the triage slice; the constant states the vendor's capability, which
/// `ralphy init`'s gate asserts.
pub const ACCEPTS_IMAGES: bool = true;

/// The Gemini planning prompt, embedded so the binary is self-contained as a
/// global tool. Assembled from `assets/prompts/plan/template.md` +
/// `overlay.gemini.md`; the single source of truth lives at `assets/prompts/`.
const PROMPT_PLAN_GEMINI: &str = include_str!("../../../assets/prompts/prompt.plan.gemini.md");

/// The two phases a `GeminiAgent` drives, each with its own model source.
#[derive(Clone, Copy)]
enum Phase {
    Plan,
    Execute,
}

/// Drives the `gemini` CLI. `exec_model` is the operator override for `execute()`
/// (set via `new`); `plan_model` is the override for `plan()` (set via
/// `with_plan_model`). `None` on either omits `-m` entirely, which on this vendor
/// means the account default — there is no per-invocation state to inherit,
/// because Ralphy owns the configuration root (D4).
pub struct GeminiAgent {
    exec_model: Option<String>,
    plan_model: Option<String>,
    run_dir: PathBuf,
    budget: IssueBudget,
}

impl GeminiAgent {
    pub fn new(model: Option<String>, run_dir: PathBuf) -> Self {
        Self {
            exec_model: model,
            plan_model: None,
            run_dir,
            budget: IssueBudget::new(ralphy_core::DEFAULT_MAX_MINUTES_PER_ISSUE),
        }
    }

    /// Set the model override used for `plan()`.
    pub fn with_plan_model(mut self, model: Option<String>) -> Self {
        self.plan_model = model;
        self
    }

    /// Set the per-issue wall-clock budget in minutes.
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

    /// Set the run's global wall-clock deadline (from `--deadline-hours`).
    pub fn with_run_deadline(mut self, run_deadline: Option<Instant>) -> Self {
        self.budget = self.budget.with_run_deadline(run_deadline);
        self
    }

    fn phase_model(&self, phase: Phase) -> Option<&str> {
        match phase {
            Phase::Plan => self.plan_model.as_deref(),
            Phase::Execute => self.exec_model.as_deref(),
        }
    }

    /// The deadline oracle the budget tests assert against.
    #[cfg(test)]
    fn issue_deadline(&self) -> Instant {
        self.budget.deadline(ralphy_core::UNBOUNDED_ISSUE_HORIZON)
    }
}

impl GeminiAgent {
    /// Everything that must exist on disk BEFORE a child is spawned, on every
    /// path: the owned configuration root (D4) and the sovereign policy document
    /// (D5). Returns what `build_gemini_command` needs to point the child at them.
    ///
    /// Deliberately NOT done once at construction — `plan` and `execute` each
    /// call it before their spawn, so a root deleted between phases is recreated
    /// rather than silently falling back to the operator's own. (The login probe
    /// calls `root::ensure` directly and carries no policy; see `run_gemini`.)
    ///
    /// It is also where the administrator's own tier is READ and REPORTED (D5).
    /// Both `plan` and `execute` propagate this with `?` from inside their `run`
    /// closure, so an autonomy-disabling control stops the run before any child
    /// exists — on every path, since nothing between `root::ensure` and the bail
    /// can swallow it. `auth::probe_gemini_login` deliberately does NOT gain the
    /// check: it makes no model call and must still answer `ralphy init`'s
    /// onboarding gate on a managed machine.
    fn prepare_root(&self, base: &Path) -> Result<(root::GeminiRoot, PathBuf, Option<String>)> {
        let root = root::ensure(base)?;
        let admin = revocation::read_admin_tier();
        for control in &admin {
            tracing::warn!("gemini: {}", control.message());
        }
        if let Some(stop) = admin
            .iter()
            .find(|c| matches!(c, revocation::AdminControl::AutonomyDisabled(_)))
        {
            anyhow::bail!("{}", stop.message());
        }
        tracing::debug!(
            home = %root.home.display(),
            settings = %root.settings.display(),
            "gemini: owned configuration root ready (D4)"
        );
        let operator = root::operator_root();
        let auth_type = root::operator_auth_type(operator.as_deref());
        let imported = policy::import_deny_rules(operator.map(|r| r.join("policies")).as_deref());
        let policy_path = policy::write_policy(&root, &policy::ralphy_policy(&imported))?;
        Ok((root, policy_path, auth_type))
    }
}

impl Agent for GeminiAgent {
    fn name(&self) -> &'static str {
        "gemini"
    }

    fn plan(&self, issue: &Issue, ws: &Workspace) -> Result<Plan> {
        let plan_path = ws.plan_path();
        let log_path = self.run_dir.join("gemini.log");
        let session_id = mint_session_id();
        let model = self.phase_model(Phase::Plan);
        let ralphy_dir = ws.ralphy_dir();
        // D2: refuse a charter the vendor would silently truncate BEFORE the emit,
        // so a run that cannot be delivered whole never publishes a `planning`
        // event for work that never began.
        check_stdin_ceiling(PLAN_CHARTER)?;

        let run = || {
            let (root, policy_path, auth_type) = self.prepare_root(&ralphy_dir)?;
            let cmd = build_gemini_command(
                &session_id,
                model,
                ws.repo_root(),
                &root.home,
                &policy_path,
                auth_type.as_deref(),
            );
            ralphy_core::emit::planning("gemini", model.unwrap_or(DEFAULT_MODEL), "");
            // Clock the budget at the spawn, not method entry, so the run_deadline
            // clamp isn't eroded by the preceding root setup.
            let timeout = self.budget.timeout(ralphy_core::UNBOUNDED_ISSUE_HORIZON);
            let r = self.run_gemini(cmd, PLAN_CHARTER, timeout)?;
            Ok((r, ()))
        };

        let charter_path = ws.plan_charter_path();
        let session = run_plan_session(
            PlanCfg {
                issue_number: issue.number,
                ralphy_dir: &ralphy_dir,
                run_dir: &self.run_dir,
                plan_path: &plan_path,
                plan_charter_path: &charter_path,
                charter_body: PROMPT_PLAN_GEMINI,
                log_path: &log_path,
                auth_msg: GEMINI_AUTH_ERROR_MSG,
                no_plan_msg: "gemini produced no plan",
            },
            run,
            auth::is_gemini_auth_error,
            // A usage limit during planning is not a generic failure: surface it
            // as a typed `PlanLimit` so the runner routes it through the same
            // stop-and-report / auto-resume path as an execute-time
            // `Outcome::Limit`, rather than aborting the run with "produced no
            // plan". This vendor reserves NO exit code for quota (D11), so the
            // text is the only signal there is, and no reset hint is recoverable
            // — the ADR-0030 synthetic cadence sets the wait.
            //
            // A revocation that is a HARD STOP is checked first and is
            // deliberately not a limit: Strict Mode or a refused workspace will
            // not heal on a retry, so scheduling one would burn the issue's whole
            // budget re-asking a question the administrator already answered.
            //
            // The informational variants must NOT pre-empt the limit. On a
            // managed host the tool-server notice is in every plan log, so
            // ordering them first would permanently misroute every plan-phase
            // quota exhaustion on that host into an untyped hard error, losing
            // ADR-0030's stop-and-report / auto-resume path.
            |log| {
                let rev = revocation::detect_revocation(log);
                rev.filter(|r| r.is_hard_stop())
                    .map(|r| anyhow::anyhow!("{}", r.message(None, log)))
                    .or_else(|| {
                        outcome::gemini_limit_note(log).map(|_| PlanLimit { reset: None }.into())
                    })
                    .or_else(|| rev.map(|r| anyhow::anyhow!("{}", r.message(None, log))))
            },
        )?;

        if let Some((r, ())) = session.as_ref() {
            note_vendor_error(&fold_gemini_stream(&r.stdout), &r.log);
        }

        let md = fs::read_to_string(&plan_path).context("reading the written plan.md")?;
        Ok(Plan {
            open_steps: plan::count_open_steps(&md),
            // This vendor's model axis is an account entitlement, not a
            // complexity tier: nothing here recommends one.
            recommended_model: None,
            path: plan_path,
            usage: phase_usage(model),
            // `None` = a finalized plan was RESUMED and no `gemini` ran.
            session_id: session.map(|_| session_id),
        })
    }

    fn execute(&self, _plan: &Plan, ws: &Workspace) -> Result<Execution> {
        let log_path = self.run_dir.join("gemini.log");
        let session_id = mint_session_id();
        // HEAD before/after bounds the work this call committed — the stream
        // carries no file-change accounting for work done through the shell.
        let before_sha = git::head_sha(ws.repo_root()).unwrap_or_default();
        let model = self.phase_model(Phase::Execute);
        let ralphy_dir = ws.ralphy_dir();
        check_stdin_ceiling(PROMPT_EXECUTE)?;

        let run = || {
            let (root, policy_path, auth_type) = self.prepare_root(&ralphy_dir)?;
            let cmd = build_gemini_command(
                &session_id,
                model,
                ws.repo_root(),
                &root.home,
                &policy_path,
                auth_type.as_deref(),
            );
            ralphy_core::emit::executing("gemini", 0, model.unwrap_or(DEFAULT_MODEL), "");
            let timeout = self.budget.timeout(ralphy_core::UNBOUNDED_ISSUE_HORIZON);
            let r = self.run_gemini(cmd, PROMPT_EXECUTE, timeout)?;
            Ok((r, ()))
        };

        let (r, ()) = run_exec_session(
            ExecCfg {
                ralphy_dir: &ralphy_dir,
                run_dir: &self.run_dir,
                log_path: &log_path,
                auth_msg: GEMINI_AUTH_ERROR_MSG,
            },
            run,
            auth::is_gemini_auth_error,
        )?;

        let after_sha = git::head_sha(ws.repo_root()).unwrap_or_default();
        let committed = before_sha != after_sha;
        let fold = fold_gemini_stream(&r.stdout);
        note_vendor_error(&fold, &r.log);
        let outcome: Outcome = classify_gemini_outcome(
            &fold,
            &r.log,
            r.exited_cleanly,
            r.timed_out,
            committed,
            r.exit_code,
            model,
        );
        info!(
            ?outcome,
            exited_cleanly = r.exited_cleanly,
            timed_out = r.timed_out,
            exit_code = ?r.exit_code,
            committed,
            saw_result = fold.saw_result,
            status = ?fold.status,
            "gemini execution ended"
        );
        Ok(Execution {
            outcome,
            usage: phase_usage(model),
            session_id: Some(session_id),
        })
    }
}

/// The model name attributed when Ralphy sent no `-m`. The vendor's own word for
/// the routed path, and what an absent flag selects.
const DEFAULT_MODEL: &str = "auto";

/// Token usage for one phase.
///
/// **Zero counts, deliberately** (ADR-0040 Amendment 1: stating the gap is the
/// deliverable). Usage accounting for this vendor is a separate slice of #252;
/// the stream's usage envelope is not parsed here, and inventing a partial number
/// would feed the cost report a figure nobody can reconcile. The model is still
/// attributed so the run report can tell a pinned run from a routed one, and
/// `cost_usd_by_model` skips zero-token entries, so no spurious cost appears.
fn phase_usage(model: Option<&str>) -> Usage {
    tracing::debug!(
        "gemini: the stream's usage envelope is not parsed yet (usage is a later slice)"
    );
    Usage {
        // The ledger key and the price key must be ONE string, or a routed run
        // costs out against another vendor's `auto` row (ADR-0034 amendment).
        model: Some(price_key(model.unwrap_or(DEFAULT_MODEL))),
        ..Default::default()
    }
}

/// Surface the vendor's own reason for stopping, verbatim. Never changes the
/// outcome — what it buys is that the stop is not mute: a refusal reads as itself
/// in the run log instead of as an unexplained `Stuck`.
///
/// `log` is stdout+stderr COMBINED, and it is consulted as a second tier because
/// under `stream-json` the diagnosis routinely goes to stderr while stdout
/// carries only records — reading the fold alone is how a self-describing
/// failure becomes mute.
fn note_vendor_error(fold: &outcome::GeminiFold, log: &str) {
    if let Some(msg) = fold.vendor_error.as_deref() {
        tracing::warn!("gemini stopped the turn: {msg}");
    }
    if let Some(note) = outcome::gemini_limit_note(log) {
        tracing::warn!("{note}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn accepts_images_is_true() {
        // Read through a binding: a bare `assert!(CONST)` is constant-folded and
        // clippy rejects it, but the invariant is worth pinning here — the CLI's
        // onboarding gate asserts the same const from the other side.
        let accepts: bool = ACCEPTS_IMAGES;
        assert!(
            accepts,
            "ADR-0043 D14: the headless surface takes `@<path>`"
        );
    }

    #[test]
    fn gemini_agent_is_a_dyn_agent() {
        let agent = GeminiAgent::new(None, PathBuf::from("/run"));
        let _as_dyn: &dyn Agent = &agent;
        assert_eq!(agent.name(), "gemini");
    }

    #[test]
    fn the_phase_model_reads_the_matching_override() {
        let agent = GeminiAgent::new(Some("exec-m".into()), PathBuf::from("/run"))
            .with_plan_model(Some("plan-m".into()));
        assert_eq!(agent.phase_model(Phase::Plan), Some("plan-m"));
        assert_eq!(agent.phase_model(Phase::Execute), Some("exec-m"));
        let bare = GeminiAgent::new(None, PathBuf::from("/run"));
        assert_eq!(bare.phase_model(Phase::Plan), None);
        assert_eq!(bare.phase_model(Phase::Execute), None);
    }

    #[test]
    fn gemini_honours_max_minutes_per_issue() {
        assert_eq!(
            GeminiAgent::new(None, PathBuf::from("/run"))
                .budget
                .max_minutes_per_issue,
            ralphy_core::DEFAULT_MAX_MINUTES_PER_ISSUE
        );
        let short = GeminiAgent::new(None, PathBuf::from("/run")).with_max_minutes_per_issue(1);
        let long = GeminiAgent::new(None, PathBuf::from("/run")).with_max_minutes_per_issue(1000);
        assert!(long.issue_deadline() > short.issue_deadline());
        let rd = Instant::now() + Duration::from_secs(1);
        let clamped = GeminiAgent::new(None, PathBuf::from("/run"))
            .with_max_minutes_per_issue(1000)
            .with_run_deadline(Some(rd));
        assert!(clamped.issue_deadline() <= rd);
    }

    /// D2's reason: the charter alone is a large fraction of the Windows ~32 KB
    /// argv ceiling before the issue body is appended, so stdin is the only safe
    /// channel. The floor pins the ORDER of magnitude, not a byte count every
    /// prompt edit would churn.
    #[test]
    fn plan_charter_exceeds_argv_safe_size() {
        assert!(
            PROMPT_PLAN_GEMINI.len() > 23_000,
            "charter is {} bytes",
            PROMPT_PLAN_GEMINI.len()
        );
    }

    #[test]
    fn prompt_plan_gemini_carries_finalize_trailer() {
        assert!(
            PROMPT_PLAN_GEMINI.contains("<!-- ralphy-plan: issue=<N> -->"),
            "planning prompt must instruct writing the exact finalized-plan trailer"
        );
    }

    /// D12: the vendor's native plan mode writes into a vendor-private directory
    /// regardless of instruction, so the overlay must tell the planner to write
    /// the file itself.
    #[test]
    fn prompt_plan_gemini_requires_the_planner_to_write_the_file() {
        assert!(
            PROMPT_PLAN_GEMINI.contains("you MUST write `.ralphy/plan.md` yourself"),
            "D12: the planner writes its own plan on this vendor"
        );
    }

    /// The executor is PLAN-AGNOSTIC: it consumes whatever `.ralphy/plan.md` the
    /// planning pass left, whichever adapter wrote it, and it bounds the commit by
    /// reading HEAD around the child rather than trusting the stream (which carries
    /// no file-change accounting for work done through the shell).
    ///
    /// Pinned on the source because both properties are ABSENCES — a `_plan` never
    /// inspected, and a `before_sha` read before the spawn — and an absence is what
    /// a behavioural test cannot see.
    #[test]
    fn execute_is_plan_agnostic_and_bounds_the_commit() {
        // Split on the test module, NOT on `#[cfg(test)]`: an earlier one guards
        // `issue_deadline`, which would truncate the production half before
        // `execute` and make every assertion below vacuously unreachable.
        let prod = include_str!("lib.rs")
            .split("\nmod tests {")
            .next()
            .unwrap();
        const SIG: &str = "fn execute(&self, _plan: &Plan, ws: &Workspace)";
        // …and scope every assertion to `execute`'s own body: `plan` above it has
        // its own `let run = ||`, which a whole-file `find` reaches first.
        let start = prod
            .find(SIG)
            .unwrap_or_else(|| panic!("execute's signature must read exactly {SIG:?}"));
        let src = &prod[start..];
        // The underscore is a convention, not a compiler guarantee — `_plan.…` is
        // legal Rust. The pin is that the binding is never MENTIONED again inside
        // the body, which is the only thing that makes the executor plan-agnostic.
        let body_end = src.find("\n    }\n").unwrap_or(src.len());
        assert!(
            !src[SIG.len()..body_end].contains("_plan"),
            "the plan artifact is never read: `_plan` must not appear in execute's body"
        );
        assert_eq!(
            src.matches("self.run_gemini(cmd, PROMPT_EXECUTE, timeout)")
                .count(),
            1,
            "the execute path sends the shared vendor-neutral charter, once"
        );
        let at = |needle: &str| {
            src.find(needle)
                .unwrap_or_else(|| panic!("execute's body must still contain {needle:?}"))
        };
        assert!(
            at("let before_sha") < at("let run = ||"),
            "HEAD must be sampled BEFORE the child can commit anything"
        );
        assert!(
            at("run_exec_session(") < at("let after_sha"),
            "…and again only after the session has ended"
        );
        assert!(at("let after_sha") < at("let committed = before_sha != after_sha;"));
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
