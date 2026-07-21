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
//! The same two gates cover the one-shot verbs in [`tasks`], which run outside the
//! `Agent` contract entirely. Token usage is captured in [`usage`] from the
//! stream's terminal `result` record and summed per invocation — records are
//! incremental, not cumulative (ADR-0042 D11).

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
mod guards;
mod model;
mod outcome;
mod settings;
mod skills;
mod tasks;
mod usage;

/// Whether the operator is logged into Cursor, from the vendor's own structured
/// answer (ADR-0042 D8) — what `ralphy init`'s gate reports.
pub use auth::{probe_cursor_login, CURSOR_AUTH_ERROR_MSG};

/// Locating the vendor's binary, which is on `PATH` on neither platform
/// (ADR-0042 D14) — `ralphy init`'s presence gate goes through this.
pub use command::locate_cursor;

/// Persisted settings for `--agent cursor` (ADR-0042 D6). See [`CursorSettings`].
pub use settings::CursorSettings;

/// The four one-shot verbs (`init`/`triage`/`consolidate`/`diagnose`), each behind
/// D6's indexing gate. See [`tasks`].
pub use tasks::{consolidate_knowledge, diagnose_repo, draft_issues, triage_issues};

/// The vendor's id grammar, normalized to the billing family (ADR-0042 D5) — the
/// price table's key. Vendor-specific by ADR-0004, so it lives here and
/// `PriceTable::resolve` stays neutral.
pub use model::model_family;

use command::{build_cursor_command, mint_session_id};
use model::model_refusal_stop;
use outcome::{classify_cursor_outcome, fold_cursor_stream};
use skills::materialize_cursor_skills;
use usage::parse_cursor_usage;

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
        // D6 BEFORE the emit, not just before the spawn: a refused run must not
        // publish a `planning` event (ADR-0019/0039) for work that never began.
        // `run_cursor` re-asserts it — that is the cross-path invariant, and this is
        // the event-hygiene one.
        guards::indexing_gate(ws.repo_root(), self.allow_indexing)?;
        let _skills = materialize_cursor_skills(ws)?;

        let run = || {
            let cmd = build_cursor_command(&session_id, model, ws.repo_root(), &self.config_dir());
            ralphy_core::emit::planning("cursor", model.unwrap_or(command::AUTO_MODEL), "");
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
            // A `--model` refusal is checked FIRST and outranks a quota stop: an
            // entitlement refusal will not heal on a retry, so scheduling
            // ADR-0030's ~30-minute wait for it would burn the issue's budget
            // re-asking an already-answered question (same ordering as
            // `ralphy-agent-gemini/src/lib.rs`). The closure fires only when no
            // plan file was written, which is exactly the zero-record refusal
            // and quota-stop shape (D13, #266).
            |log| {
                model_refusal_stop(log, model).or_else(|| {
                    outcome::cursor_limit_note(&outcome::fold_cursor_stream(log))
                        .map(|_| PlanLimit { reset: None }.into())
                })
            },
        )?;

        // A RESUMED plan (no child ran, `session` is `None`) must keep the
        // zero-token attributed value, never a stale one.
        let plan_usage = session
            .as_ref()
            .map(|(r, _)| parse_cursor_usage(&r.stdout, model))
            .unwrap_or_else(|| requested_model_usage(model));

        if let Some((r, _)) = session.as_ref() {
            let fold = fold_cursor_stream(&r.stdout);
            Self::verify_session_adoption(&session_id, fold.session_id.as_deref())?;
            note_degraded(&fold);
            note_vendor_error(&fold);
            note_usage_provenance(&self.config_dir(), &session_id);
        }

        let md = fs::read_to_string(&plan_path).context("reading the written plan.md")?;
        Ok(Plan {
            open_steps: plan::count_open_steps(&md),
            // Cursor's model axis is a plan entitlement, not a complexity tier (D5).
            recommended_model: None,
            path: plan_path,
            // `result.usage` is the only accounting this vendor has (D11); see
            // `usage.rs` for the stream capture and the incremental-sum rule.
            usage: plan_usage,
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
        // See `plan`: the gate precedes the `executing` event, and `run_cursor`
        // re-asserts it on every spawn path.
        guards::indexing_gate(ws.repo_root(), self.allow_indexing)?;
        let _skills = materialize_cursor_skills(ws)?;

        let run = || {
            let cmd = build_cursor_command(&session_id, model, ws.repo_root(), &self.config_dir());
            ralphy_core::emit::executing("cursor", 0, model.unwrap_or(command::AUTO_MODEL), "");
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

        // BEFORE classification: a refusal wears the same zero-record shape as a
        // truncation (D3 rule 2), so classifying first would report it as `Stuck`
        // and lose the one sentence that fixes it. Gated on that shape rather than
        // on the text alone, because `r.log` is stdout+stderr MERGED and a working
        // run's transcript can quote the sentence — in this very repository it can
        // read the committed refusal fixtures. Two independent gates: the shape
        // here, the line-start match in `model_refusal_stop`.
        if fold.saw_no_records() {
            if let Some(e) = model_refusal_stop(&r.log, model) {
                return Err(e);
            }
        }

        Self::verify_session_adoption(&session_id, fold.session_id.as_deref())?;
        note_degraded(&fold);
        note_vendor_error(&fold);
        note_usage_provenance(&self.config_dir(), &session_id);
        // #266: on a quota stop, name any work this call already committed so it
        // is not silently discarded. `7.min(len)` guards the empty-sha default
        // both shas fall back to (`unwrap_or_default()` above) from panicking on
        // the slice.
        let range = committed.then(|| {
            format!(
                "{}..{}",
                &before_sha[..7.min(before_sha.len())],
                &after_sha[..7.min(after_sha.len())]
            )
        });
        if let Some(n) = outcome::limit_stop_note(&fold, range.as_deref()) {
            tracing::warn!("{n}");
        }
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
            // See `plan`: `result.usage` is the stream's own accounting (D11).
            usage: parse_cursor_usage(&r.stdout, model),
            session_id: Some(session_id),
        })
    }
}

/// Attribute the model Ralphy REQUESTED, normalized to its billing family. Token
/// counts stay zero until #249, and `cost_usd_by_model` skips zero-token entries,
/// so this adds an attribution without a spurious cost. No pin is attributed as
/// the literal `auto` — the vendor's own name for the routed path, and what an
/// absent `--model` would have sent anyway (D4).
pub(crate) fn requested_model_usage(model: Option<&str>) -> ralphy_core::Usage {
    ralphy_core::Usage {
        model: Some(model_family(model.unwrap_or(command::AUTO_MODEL))),
        ..Default::default()
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

/// Surface the vendor's own reason for stopping, verbatim. The outcome is already
/// non-green when this fires — what it buys is that the stop is not mute: an
/// account-quota refusal reads as itself in the run log instead of as an
/// unexplained `Stuck`. A quota-class refusal is ALSO classified `Limit(None)`
/// and logged again via `limit_stop_note` (#266) — the two calls are
/// deliberately redundant on that path: this one fires unconditionally so a
/// future non-quota `vendor_error` stays visible.
fn note_vendor_error(fold: &outcome::CursorFold) {
    if let Some(msg) = fold.vendor_error.as_deref() {
        tracing::warn!("cursor stopped the turn: {msg}");
    }
}

/// State the credit/token unit mismatch once per phase (story 33). The
/// on-disk store lookup is informational only — the vendor's stores hold no
/// token count either way, so a `None` result ("no on-disk record") is
/// normal, not an error, and this never turns an `Err` return into a
/// different outcome.
fn note_usage_provenance(config_dir: &std::path::Path, session_id: &str) {
    let store = usage::cursor_session_store(config_dir, session_id)
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "no on-disk record".to_string());
    tracing::warn!("{} (session store: {store})", usage::CURSOR_CREDIT_NOTE);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// Story 21: a pinned run must be distinguishable from a routed one in the run
    /// report, and the routed one must not read as "not reported".
    #[test]
    fn the_requested_model_is_attributed_and_auto_is_named() {
        assert_eq!(
            requested_model_usage(Some("composer-2.5-fast"))
                .model
                .as_deref(),
            Some("composer-2.5")
        );
        assert_eq!(requested_model_usage(None).model.as_deref(), Some("auto"));
        // Token counts stay zero until #249, so no cost is fabricated.
        assert_eq!(requested_model_usage(None).input, 0);
    }

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

    /// ADR-0042 D3: this vendor opens with ~8.1 s of silence and shows inter-record
    /// gaps up to ~7.4 s, so a watchdog in seconds would reap healthy runs. Unlike
    /// `max_minutes_per_issue`, `IssueBudget::new` leaves `idle_minutes` at `0` —
    /// the CLI wiring layer (`run/wiring.rs`) applies `DEFAULT_IDLE_MINUTES` before
    /// handing a `CursorAgent` to a run, so this pins the CONSTANT the wiring
    /// relies on plus the plumbing, rather than a fresh agent's own field.
    #[test]
    fn the_idle_watchdog_default_tolerates_the_vendor_cadence() {
        // Read through a binding: a bare constant assertion is constant-folded and
        // clippy rejects it (see `accepts_images_is_false`).
        let idle_minutes: u64 = ralphy_core::DEFAULT_IDLE_MINUTES;
        assert!(
            idle_minutes * 60 >= 60,
            "measured ~8.1s opening silence, ~7.4s inter-record gaps"
        );
        let agent = CursorAgent::new(None, PathBuf::from("/run"))
            .with_idle_minutes(ralphy_core::DEFAULT_IDLE_MINUTES);
        assert_eq!(agent.budget.idle_minutes, ralphy_core::DEFAULT_IDLE_MINUTES);
    }

    /// Story 33: both phases must report the stream's OWN usage, not the
    /// requested-model-only attribution — deleting either call site keeps the
    /// suite green unless this pin catches it (#249).
    #[test]
    fn both_phases_report_stream_usage() {
        let src = include_str!("lib.rs");
        let call = concat!("parse_cursor_usage(", "&r.stdout,");
        assert_eq!(
            src.matches(call).count(),
            2,
            "both phases must report the stream's own usage (#249)"
        );
    }

    /// Story 33: both phases must state the credit/token unit mismatch —
    /// deleting either call site keeps the suite green unless this pin catches
    /// it.
    #[test]
    fn every_run_notes_the_credit_unit_mismatch() {
        let src = include_str!("lib.rs");
        let call = concat!("note_usage_provenance(", "&self");
        assert_eq!(
            src.matches(call).count(),
            2,
            "story 33: both phases must state the credit/token unit mismatch"
        );
    }

    /// A source-text pin in the style of `outcome.rs::the_gate_runs_before_any_child_is_spawned`:
    /// the operator-visible degraded-tool-call note must be raised on BOTH the
    /// `plan` and `execute` paths, and on `execute` only after the fold has run —
    /// deleting either call keeps the suite green unless this pin catches it.
    #[test]
    fn execute_notes_the_degraded_calls() {
        let src = include_str!("lib.rs");
        let call = concat!("note_degraded(", "&fold);");
        assert_eq!(
            src.matches(call).count(),
            2,
            "note_degraded(&fold) must be called on both the plan and execute paths"
        );
        let fold_call = concat!("fold_cursor_stream(", "&r.stdout);");
        let last_fold = src.rfind(fold_call).expect("execute's fold call site");
        let last_note = src.rfind(call).expect("execute's note_degraded call site");
        assert!(
            last_note > last_fold,
            "execute must fold the stream before it can note the degraded calls"
        );
        // Same pin for the vendor's own stop reason: dropping it is exactly the
        // regression that made a quota refusal arrive as a mute `Stuck`.
        let vendor_call = concat!("note_vendor_error(", "&fold);");
        assert_eq!(
            src.matches(vendor_call).count(),
            2,
            "note_vendor_error(&fold) must be called on both the plan and execute paths"
        );
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

    /// #266: the plan path routes a quota stop to `PlanLimit`, and a hard
    /// `--model` refusal is checked FIRST — it will not heal on a retry, so
    /// scheduling a wait for it would burn the issue's budget re-asking an
    /// already-answered question.
    #[test]
    fn the_plan_path_routes_a_quota_stop_to_plan_limit() {
        let src = include_str!("lib.rs");
        let refusal = concat!("model_refusal_stop(", "log, model)");
        let limit = concat!("PlanLimit { reset: ", "None }");
        let at_refusal = src
            .find(refusal)
            .expect("plan()'s on_missing must check the model refusal");
        let at_limit = src
            .find(limit)
            .expect("plan()'s on_missing must route a quota stop to PlanLimit");
        assert!(
            at_refusal < at_limit,
            "a hard refusal must be checked before the limit"
        );
    }

    /// #266: whatever reaches Ralphy already exhausted the vendor's own retries —
    /// no production path may re-spawn on a quota stop. Pins BOTH halves: the
    /// crate's single `HeadlessCall` site stays singular, and no production
    /// source loops around a limit check.
    #[test]
    fn no_adapter_side_retry_of_a_quota_stop() {
        fn sources(dir: &std::path::Path, out: &mut Vec<String>) {
            for entry in std::fs::read_dir(dir).expect("readable src dir") {
                let path = entry.expect("entry").path();
                if path.is_dir() {
                    sources(&path, out);
                } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
                    let body = std::fs::read_to_string(&path).expect("read source");
                    out.push(body.split("#[cfg(test)]").next().unwrap_or("").to_string());
                }
            }
        }
        let mut production = Vec::new();
        sources(
            std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/src")),
            &mut production,
        );
        let spawn = concat!("HeadlessCall::", "new(cmd,");
        let spawn_count: usize = production.iter().map(|s| s.matches(spawn).count()).sum();
        assert_eq!(
            spawn_count, 1,
            "the crate's single HeadlessCall site must stay singular"
        );
        // Scoped to the files that call `cursor_limit_note`, not the whole crate:
        // `model.rs` has its own unrelated fixpoint `loop {}` (decoration
        // stripping), which is not a retry and must not trip this pin.
        let limit_call = "cursor_limit_note(";
        for body in &production {
            if body.contains(limit_call) {
                assert!(
                    !body.contains("loop {") && !body.contains("while "),
                    "no production path may loop around a limit check — a quota \
                     stop already exhausted the vendor's own retries"
                );
            }
        }
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
