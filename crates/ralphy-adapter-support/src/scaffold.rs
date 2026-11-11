//! The vendor-neutral **plan/execute shell** the headless adapters (Codex,
//! `OpenCode`) share. Each adapter's `plan()`/`execute()` repeated the same
//! mechanical front and back around the one vendor-specific step (build the
//! command, snapshot usage, run headless): create the run/`.ralphy` dirs, write
//! the plan charter, remove a stale plan, then — after the run — walk the no-plan
//! ladder (typed limit → auth bail → generic "no plan") or the execute-time auth
//! bail. Both auth bails are reached only from a run that FAILED (no plan on
//! disk / an unclean exit) — see [`run_exec_session`] for why that gate is
//! load-bearing. [`run_plan_session`] and [`run_exec_session`] own that shell.
//!
//! It stays core-free (ADR-0002): the vendor step is a `run` closure returning a
//! [`HeadlessRun`] plus an opaque payload `P` (the usage snapshot the vendor folds
//! itself), the auth check and the typed-limit lift are closures, and the scaffold
//! names no `Plan`/`Outcome`/`PlanLimit`. The plan-time limit is threaded as an
//! `anyhow::Error` the caller constructs from its own core type and the scaffold
//! returns verbatim, so the runner's downstream `PlanLimit` downcast is preserved.

use std::fs;
use std::path::Path;

use anyhow::{bail, Context, Result};

use crate::{failed_run_stop, HeadlessRun, RunFacts, RunStop};

/// The paths and error wording for a [`run_plan_session`] call. Everything the
/// shared plan shell needs to create dirs, write the charter, and phrase the
/// no-plan bails; the `(see <log>)` / `at <plan>` suffixes are appended by the
/// runner, so `auth_msg`/`no_plan_msg` carry only the prefix.
pub struct PlanCfg<'a> {
    /// The current issue's number — identity for the finalized-plan trailer. When
    /// `plan_path`'s last line already carries this issue's trailer, the session
    /// resumes (keeps the plan, skips the run) instead of re-planning.
    pub issue_number: u64,
    /// The workspace `.ralphy` dir (created if absent).
    pub ralphy_dir: &'a Path,
    /// The run's log/scratch dir (created if absent).
    pub run_dir: &'a Path,
    /// The plan artifact the session is expected to write.
    pub plan_path: &'a Path,
    /// Where the full plan charter is written each call.
    pub plan_charter_path: &'a Path,
    /// The full plan charter body written to `plan_charter_path`.
    pub charter_body: &'a str,
    /// The combined log path referenced in the bail messages.
    pub log_path: &'a Path,
    /// Message when the auth detector fires (e.g. the adapter's `*_AUTH_ERROR_MSG`).
    pub auth_msg: &'a str,
    /// Message prefix when no plan was written (e.g. "codex produced no plan").
    pub no_plan_msg: &'a str,
}

/// Create `dir` and its parents, best-effort: the step that needs the directory
/// reports a failure with its own context.
pub(crate) fn ensure_dir(dir: &Path) {
    #[expect(
        clippy::unused_result_ok,
        reason = "the step that writes into the directory reports the failure with its own context"
    )]
    fs::create_dir_all(dir).ok();
}

/// Remove a file a previous run may have left, so it cannot pass for this run's
/// output. The file is usually absent, and a leftover that cannot be removed is
/// overwritten or reported by the step that writes it.
pub(crate) fn remove_stale_file(file: &Path) {
    // the file is usually absent, and the step that writes it reports a real failure
    if let Err(e) = fs::remove_file(file) {
        tracing::debug!("could not remove the old file: {e}");
    }
}

/// Drive the shared plan shell. FIRST checks the resume short-circuit: when
/// `cfg.plan_path` is already a finalized plan for `cfg.issue_number` (its last
/// line carries the trailer), returns `Ok(None)` WITHOUT touching the file or
/// running `run` — an abruptly-killed run resumes execution instead of re-planning.
/// Otherwise: create dirs, write the charter, drop the stale plan, run the vendor
/// `run` closure, then — if no plan landed — walk the no-plan ladder. `on_missing`
/// wins first (the typed limit lifted to an `anyhow::Error`), then `is_auth_error`
/// (the auth bail), then the generic `no_plan_msg`. Returns
/// `Ok(Some((HeadlessRun, P)))` when a fresh plan was produced.
pub fn run_plan_session<P>(
    cfg: PlanCfg,
    run: impl FnOnce() -> Result<(HeadlessRun, P)>,
    is_auth_error: impl Fn(&str) -> bool,
    on_missing: impl FnOnce(&str) -> Option<anyhow::Error>,
) -> Result<Option<(HeadlessRun, P)>> {
    // Resume before any side effect: a finalized plan for this issue is kept
    // byte-for-byte and the vendor run is skipped (zero planning tokens).
    if crate::resume::plan_is_finalized_for(cfg.plan_path, cfg.issue_number) {
        return Ok(None);
    }
    ensure_dir(cfg.ralphy_dir);
    ensure_dir(cfg.run_dir);
    // Plan fresh every run; never reuse a stale artifact.
    remove_stale_file(cfg.plan_path);
    // Full charter on disk (mirrors .ralphy/exec.md); rewritten each plan call so
    // a resumed session still finds it.
    fs::write(cfg.plan_charter_path, cfg.charter_body)
        .context("writing .ralphy/plan-charter.md")?;

    let (r, p) = run()?;

    if !cfg.plan_path.exists() {
        // A usage limit during planning is not a generic failure: surface it as
        // the caller's typed error so the runner routes it through stop-and-report
        // / auto-resume rather than aborting the whole run.
        if let Some(e) = on_missing(&r.log) {
            return Err(e);
        }
        // An auth failure won't self-heal, so bail with an actionable message.
        // A missing plan counts as a failed run, whatever the exit.
        let facts = RunFacts {
            exited_cleanly: false,
            ..RunFacts::from(&r)
        };
        if failed_run_stop(&facts, &r.log, &is_auth_error) == Some(RunStop::Auth) {
            bail!("{} (see {})", cfg.auth_msg, cfg.log_path.display());
        }
        bail!(
            "{} at {} (see {})",
            cfg.no_plan_msg,
            cfg.plan_path.display(),
            cfg.log_path.display()
        );
    }

    // The finalized-plan trailer is the resume marker (see [`crate::resume`]): the
    // plan prompt asks the LLM to write it as the plan's last line, but some vendors
    // (OpenCode) reliably end with a chat summary instead and omit it, silently
    // breaking resume. When the planner exited CLEANLY and left a plan without the
    // trailer, stamp it Rust-side so an abruptly-killed run can still resume. The
    // clean-exit gate is load-bearing: a plan truncated by a kill/timeout must NOT be
    // marked finalized. Idempotent (skipped when already present) and best-effort (a
    // write failure just forgoes resume, never fails the plan).
    if r.exited_cleanly && !crate::resume::plan_is_finalized_for(cfg.plan_path, cfg.issue_number) {
        stamp_plan_trailer(cfg.plan_path, cfg.issue_number);
    }
    Ok(Some((r, p)))
}

/// Append the finalized-plan [`crate::resume::plan_trailer`] as the plan's last line.
/// Used only as the clean-exit fallback in [`run_plan_session`] when the planner wrote
/// a plan but omitted the trailer; see that call site for why it is gated on a clean
/// exit. Best-effort — a read/write failure simply leaves the plan un-stamped.
fn stamp_plan_trailer(plan_path: &Path, issue_number: u64) {
    let Ok(md) = fs::read_to_string(plan_path) else {
        return;
    };
    let mut out = md.trim_end().to_owned();
    out.push_str("\n\n");
    out.push_str(&crate::resume::plan_trailer(issue_number));
    out.push('\n');
    // best-effort: a failed write only forgoes resume and never fails the plan
    if let Err(e) = fs::write(plan_path, out) {
        tracing::warn!("could not write the plan file: {e}");
    }
}

/// The paths and auth wording for a [`run_exec_session`] call — no plan/charter,
/// since execute reads the plan the planner already wrote.
pub struct ExecCfg<'a> {
    /// The workspace `.ralphy` dir (created if absent).
    pub ralphy_dir: &'a Path,
    /// The run's log/scratch dir (created if absent).
    pub run_dir: &'a Path,
    /// The combined log path referenced in the auth bail message.
    pub log_path: &'a Path,
    /// Message when the auth detector fires (e.g. the adapter's `*_AUTH_ERROR_MSG`).
    pub auth_msg: &'a str,
}

/// Drive the shared execute shell: create dirs, run the vendor `run` closure,
/// then — only when that run did NOT exit cleanly — bail on an auth error (a
/// signed-out account never makes progress). Returns the vendor's
/// `(HeadlessRun, P)` on success for it to classify itself.
///
/// THE FAILED-RUN GATE IS LOAD-BEARING, not a micro-optimisation. `r.log` is the
/// child's whole combined stdout+stderr, which for an agent CLI includes
/// everything the agent *read* — so an unconditional scan self-triggers the
/// moment the agent greps source that merely documents the vendor's auth banner,
/// and every adapter documents its own banner in its own `auth.rs`. Observed:
/// a green execute (clean exit, DONE sentinel, work committed) was thrown away —
/// aborting the whole queue with "run `codex login`" against an authenticated
/// account — because a ripgrep over the Codex adapter's sources echoed its
/// `401 Unauthorized` message into the log. The detector exists to explain a run
/// that FAILED (it would otherwise masquerade as `Outcome::Stuck`); a clean exit
/// cannot be an auth failure, because a signed-out CLI fails the command. The
/// plan shell reaches the same place from the other side: there the auth ladder
/// is already reached only when no plan landed. Should a vendor ever be observed
/// exiting 0 on an auth failure, the gate in [`failed_run_stop`] is what has to
/// change — not the detector.
pub fn run_exec_session<P>(
    cfg: ExecCfg,
    run: impl FnOnce() -> Result<(HeadlessRun, P)>,
    is_auth_error: impl Fn(&str) -> bool,
) -> Result<(HeadlessRun, P)> {
    ensure_dir(cfg.run_dir);
    ensure_dir(cfg.ralphy_dir);
    let (r, p) = run()?;
    if failed_run_stop(&RunFacts::from(&r), &r.log, is_auth_error) == Some(RunStop::Auth) {
        bail!("{} (see {})", cfg.auth_msg, cfg.log_path.display());
    }
    Ok((r, p))
}

#[cfg(test)]
mod tests;
