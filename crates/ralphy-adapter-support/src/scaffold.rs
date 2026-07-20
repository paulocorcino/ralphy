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
    let mut out = md.trim_end().to_string();
    out.push_str("\n\n");
    out.push_str(&crate::resume::plan_trailer(issue_number));
    out.push('\n');
    let _ = fs::write(plan_path, out);
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
            idle_killed: false,
            exit_code: Some(0),
        }
    }

    // A `HeadlessRun` that was killed on the wall timeout (unclean exit) — the plan on
    // disk may be truncated, so it must NOT be stamped as finalized.
    fn fake_run_killed(log: &str) -> HeadlessRun {
        HeadlessRun {
            stdout: String::new(),
            log: log.to_string(),
            exited_cleanly: false,
            timed_out: true,
            idle_killed: false,
            exit_code: None,
        }
    }

    fn tmp(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("ralphy-scaffold-{}-{}", std::process::id(), name))
    }

    fn plan_cfg<'a>(dir: &'a Path, plan_path: &'a Path, charter_path: &'a Path) -> PlanCfg<'a> {
        PlanCfg {
            issue_number: 0,
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
        .unwrap()
        .expect("fresh plan yields Some");
        assert_eq!(p, 7);
        assert!(r.log.is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn clean_plan_missing_trailer_is_stamped_for_resume() {
        let dir = tmp("stamp");
        fs::create_dir_all(&dir).unwrap();
        let plan_path = dir.join("plan.md");
        let charter = dir.join("charter.md");
        let mut cfg = plan_cfg(&dir, &plan_path, &charter);
        cfg.issue_number = 71;
        run_plan_session(
            cfg,
            || {
                // A plan that ends in prose, not the trailer — the OpenCode symptom.
                fs::write(&plan_path, "# plan\n## Steps\n- [ ] do a thing\n").unwrap();
                Ok((fake_run(""), 1u32))
            },
            |_| true,
            |_| None,
        )
        .unwrap()
        .expect("fresh plan yields Some");
        assert!(
            crate::resume::plan_is_finalized_for(&plan_path, 71),
            "a clean plan missing its trailer must be stamped so resume works"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn killed_plan_missing_trailer_is_not_stamped() {
        let dir = tmp("killed");
        fs::create_dir_all(&dir).unwrap();
        let plan_path = dir.join("plan.md");
        let charter = dir.join("charter.md");
        let mut cfg = plan_cfg(&dir, &plan_path, &charter);
        cfg.issue_number = 71;
        run_plan_session(
            cfg,
            || {
                // A plan truncated by a kill: the run did not exit cleanly.
                fs::write(&plan_path, "# plan\n## Steps\n- [ ] half-writ").unwrap();
                Ok((fake_run_killed(""), 1u32))
            },
            |_| true,
            |_| None,
        )
        .unwrap()
        .expect("a plan on disk still yields Some");
        assert!(
            !crate::resume::plan_is_finalized_for(&plan_path, 71),
            "an unclean (killed) plan must NOT be marked finalized"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn clean_plan_with_trailer_is_not_double_stamped() {
        let dir = tmp("idem");
        fs::create_dir_all(&dir).unwrap();
        let plan_path = dir.join("plan.md");
        let charter = dir.join("charter.md");
        let mut cfg = plan_cfg(&dir, &plan_path, &charter);
        cfg.issue_number = 71;
        let body = format!(
            "# plan\n## Steps\n- [ ] x\n\n{}\n",
            crate::resume::plan_trailer(71)
        );
        run_plan_session(
            cfg,
            || {
                fs::write(&plan_path, &body).unwrap();
                Ok((fake_run(""), 1u32))
            },
            |_| true,
            |_| None,
        )
        .unwrap()
        .expect("fresh plan yields Some");
        let after = fs::read_to_string(&plan_path).unwrap();
        assert_eq!(
            after, body,
            "an already-finalized plan must not be re-stamped"
        );
        assert_eq!(
            after.matches("ralphy-plan: issue=71").count(),
            1,
            "exactly one trailer"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn resume_keeps_finalized_plan_and_skips_run() {
        let dir = tmp("resume");
        fs::create_dir_all(&dir).unwrap();
        let plan_path = dir.join("plan.md");
        let charter = dir.join("charter.md");
        fs::write(&plan_path, "# plan\n<!-- ralphy-plan: issue=147 -->\n").unwrap();
        let before = fs::read(&plan_path).unwrap();
        let mut cfg = plan_cfg(&dir, &plan_path, &charter);
        cfg.issue_number = 147;
        let ran = std::cell::Cell::new(false);
        let out = run_plan_session(
            cfg,
            || {
                ran.set(true);
                Ok((fake_run(""), 1u32))
            },
            |_| true,
            |_| None,
        )
        .unwrap();
        assert!(out.is_none(), "finalized plan should resume (None)");
        assert!(!ran.get(), "run closure must NOT be invoked on resume");
        assert_eq!(
            fs::read(&plan_path).unwrap(),
            before,
            "plan bytes unchanged"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn resume_reruns_on_other_issue_trailer() {
        let dir = tmp("other-trailer");
        fs::create_dir_all(&dir).unwrap();
        let plan_path = dir.join("plan.md");
        let charter = dir.join("charter.md");
        fs::write(&plan_path, "# plan\n<!-- ralphy-plan: issue=999 -->\n").unwrap();
        let mut cfg = plan_cfg(&dir, &plan_path, &charter);
        cfg.issue_number = 147;
        let out = run_plan_session(
            cfg,
            || {
                fs::write(&plan_path, "# fresh plan").unwrap();
                Ok((fake_run(""), 5u32))
            },
            |_| true,
            |_| None,
        )
        .unwrap();
        assert!(out.is_some(), "other-issue trailer must re-plan (Some)");
        // The fresh plan replaced the #999 content; a clean run then re-stamps it for
        // THIS issue, so it is finalized for #147 and no longer for #999.
        let after = fs::read_to_string(&plan_path).unwrap();
        assert!(after.starts_with("# fresh plan"), "got: {after:?}");
        assert!(crate::resume::plan_is_finalized_for(&plan_path, 147));
        assert!(!crate::resume::plan_is_finalized_for(&plan_path, 999));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn resume_reruns_on_truncated_plan() {
        let dir = tmp("truncated");
        fs::create_dir_all(&dir).unwrap();
        let plan_path = dir.join("plan.md");
        let charter = dir.join("charter.md");
        fs::write(&plan_path, "# plan\n## Steps").unwrap();
        let mut cfg = plan_cfg(&dir, &plan_path, &charter);
        cfg.issue_number = 147;
        let out = run_plan_session(
            cfg,
            || {
                fs::write(&plan_path, "# fresh plan").unwrap();
                Ok((fake_run(""), 5u32))
            },
            |_| true,
            |_| None,
        )
        .unwrap();
        assert!(out.is_some(), "truncated plan (no trailer) must re-plan");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn resume_twice_is_side_effect_free() {
        let dir = tmp("resume-twice");
        fs::create_dir_all(&dir).unwrap();
        let plan_path = dir.join("plan.md");
        let charter = dir.join("charter.md");
        fs::write(&plan_path, "# plan\n<!-- ralphy-plan: issue=147 -->\n").unwrap();
        let before = fs::read(&plan_path).unwrap();
        let ran = std::cell::Cell::new(false);
        for _ in 0..2 {
            let mut cfg = plan_cfg(&dir, &plan_path, &charter);
            cfg.issue_number = 147;
            let out = run_plan_session(
                cfg,
                || {
                    ran.set(true);
                    Ok((fake_run(""), 1u32))
                },
                |_| true,
                |_| None,
            )
            .unwrap();
            assert!(out.is_none());
        }
        assert!(!ran.get(), "run closure never called across two resumes");
        assert_eq!(fs::read(&plan_path).unwrap(), before, "bytes identical");
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
