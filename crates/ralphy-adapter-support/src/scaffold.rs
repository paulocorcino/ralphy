//! The vendor-neutral **plan/execute shell** the headless adapters (Codex,
//! OpenCode) share. Each adapter's `plan()`/`execute()` repeated the same
//! mechanical front and back around the one vendor-specific step (build the
//! command, snapshot usage, run headless): create the run/`.ralphy` dirs, write
//! the plan charter, remove a stale plan, then — after the run — walk the no-plan
//! ladder (typed limit → auth bail → generic "no plan") or the execute-time auth
//! bail. [`run_plan_session`] and [`run_exec_session`] own that shell.
//!
//! It stays core-free (ADR-0004): the vendor step is a `run` closure returning a
//! [`HeadlessRun`] plus an opaque payload `P` (the usage snapshot the vendor folds
//! itself), the auth check and the typed-limit lift are closures, and the scaffold
//! names no `Plan`/`Outcome`/`PlanLimit`. The plan-time limit is threaded as an
//! `anyhow::Error` the caller constructs from its own core type and the scaffold
//! returns verbatim, so the runner's downstream `PlanLimit` downcast is preserved.

use std::fs;
use std::path::Path;

use anyhow::{bail, Context, Result};

use crate::HeadlessRun;

/// The paths and error wording for a [`run_plan_session`] call. Everything the
/// shared plan shell needs to create dirs, write the charter, and phrase the
/// no-plan bails; the `(see <log>)` / `at <plan>` suffixes are appended by the
/// runner, so `auth_msg`/`no_plan_msg` carry only the prefix.
pub struct PlanCfg<'a> {
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

/// Drive the shared plan shell: create dirs, write the charter, drop a stale plan,
/// run the vendor `run` closure, then — if no plan landed — walk the no-plan
/// ladder. `on_missing` wins first (the typed limit lifted to an `anyhow::Error`),
/// then `is_auth_error` (the auth bail), then the generic `no_plan_msg`. Returns
/// the vendor's `(HeadlessRun, P)` when a plan exists.
pub fn run_plan_session<P>(
    cfg: PlanCfg,
    run: impl FnOnce() -> Result<(HeadlessRun, P)>,
    is_auth_error: impl Fn(&str) -> bool,
    on_missing: impl FnOnce(&str) -> Option<anyhow::Error>,
) -> Result<(HeadlessRun, P)> {
    fs::create_dir_all(cfg.ralphy_dir).ok();
    fs::create_dir_all(cfg.run_dir).ok();
    // Plan fresh every run; never reuse a stale artifact.
    let _ = fs::remove_file(cfg.plan_path);
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
        if is_auth_error(&r.log) {
            bail!("{} (see {})", cfg.auth_msg, cfg.log_path.display());
        }
        bail!(
            "{} at {} (see {})",
            cfg.no_plan_msg,
            cfg.plan_path.display(),
            cfg.log_path.display()
        );
    }
    Ok((r, p))
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

/// Drive the shared execute shell: create dirs, run the vendor `run` closure, then
/// bail unconditionally on an auth error (a signed-out account never makes
/// progress). Returns the vendor's `(HeadlessRun, P)` on success for it to
/// classify itself.
pub fn run_exec_session<P>(
    cfg: ExecCfg,
    run: impl FnOnce() -> Result<(HeadlessRun, P)>,
    is_auth_error: impl Fn(&str) -> bool,
) -> Result<(HeadlessRun, P)> {
    fs::create_dir_all(cfg.run_dir).ok();
    fs::create_dir_all(cfg.ralphy_dir).ok();
    let (r, p) = run()?;
    if is_auth_error(&r.log) {
        bail!("{} (see {})", cfg.auth_msg, cfg.log_path.display());
    }
    Ok((r, p))
}

#[cfg(test)]
mod tests {
    use super::*;

    // A `HeadlessRun` fabricated for the ladder tests — no real child process; the
    // scaffold only reads `.log`.
    fn fake_run(log: &str) -> HeadlessRun {
        HeadlessRun {
            stdout: String::new(),
            log: log.to_string(),
            exited_cleanly: true,
            timed_out: false,
        }
    }

    fn tmp(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("ralphy-scaffold-{}-{}", std::process::id(), name))
    }

    fn plan_cfg<'a>(dir: &'a Path, plan_path: &'a Path, charter_path: &'a Path) -> PlanCfg<'a> {
        PlanCfg {
            ralphy_dir: dir,
            run_dir: dir,
            plan_path,
            plan_charter_path: charter_path,
            charter_body: "charter",
            log_path: dir,
            auth_msg: "AUTH_BAIL",
            no_plan_msg: "NO_PLAN_BAIL",
        }
    }

    #[test]
    fn plan_missing_limit_beats_auth() {
        let dir = tmp("limit");
        fs::create_dir_all(&dir).unwrap();
        let plan_path = dir.join("plan-does-not-exist.md");
        let charter = dir.join("charter.md");
        let cfg = plan_cfg(&dir, &plan_path, &charter);
        let err = run_plan_session(
            cfg,
            || Ok((fake_run("hit usage limit"), ())),
            |_| true,
            |_| Some(anyhow::anyhow!("limit")),
        )
        .unwrap_err();
        assert_eq!(err.to_string(), "limit");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn plan_missing_auth_beats_generic() {
        let dir = tmp("auth");
        fs::create_dir_all(&dir).unwrap();
        let plan_path = dir.join("plan-does-not-exist.md");
        let charter = dir.join("charter.md");
        let cfg = plan_cfg(&dir, &plan_path, &charter);
        let err =
            run_plan_session(cfg, || Ok((fake_run("boom"), ())), |_| true, |_| None).unwrap_err();
        assert!(err.to_string().starts_with("AUTH_BAIL"), "got: {err}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn plan_missing_falls_through_to_generic() {
        let dir = tmp("generic");
        fs::create_dir_all(&dir).unwrap();
        let plan_path = dir.join("plan-does-not-exist.md");
        let charter = dir.join("charter.md");
        let cfg = plan_cfg(&dir, &plan_path, &charter);
        let err =
            run_plan_session(cfg, || Ok((fake_run("boom"), ())), |_| false, |_| None).unwrap_err();
        assert!(err.to_string().starts_with("NO_PLAN_BAIL"), "got: {err}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn plan_present_returns_ok() {
        let dir = tmp("present");
        fs::create_dir_all(&dir).unwrap();
        let plan_path = dir.join("plan.md");
        let charter = dir.join("charter.md");
        let cfg = plan_cfg(&dir, &plan_path, &charter);
        // The scaffold drops a stale plan before the run, so — as the vendor does —
        // the run closure is what writes the fresh plan.
        let (r, p) = run_plan_session(
            cfg,
            || {
                fs::write(&plan_path, "# plan").unwrap();
                Ok((fake_run(""), 7u32))
            },
            |_| true,
            |_| Some(anyhow::anyhow!("limit")),
        )
        .unwrap();
        assert_eq!(p, 7);
        assert!(r.log.is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn exec_auth_bails_else_ok() {
        let dir = tmp("exec");
        fs::create_dir_all(&dir).unwrap();
        let cfg = ExecCfg {
            ralphy_dir: &dir,
            run_dir: &dir,
            log_path: &dir,
            auth_msg: "EXEC_AUTH",
        };
        let err = run_exec_session(cfg, || Ok((fake_run("nope"), ())), |_| true).unwrap_err();
        assert!(err.to_string().contains("EXEC_AUTH"), "got: {err}");

        let cfg_ok = ExecCfg {
            ralphy_dir: &dir,
            run_dir: &dir,
            log_path: &dir,
            auth_msg: "EXEC_AUTH",
        };
        let (_, p) = run_exec_session(cfg_ok, || Ok((fake_run("fine"), 9u32)), |_| false).unwrap();
        assert_eq!(p, 9);
        let _ = fs::remove_dir_all(&dir);
    }
}
