use super::*;
use crate::scratch::remove_scratch_dir;

// A `HeadlessRun` fabricated for the ladder tests — no real child process; the
// scaffold only reads `.log`.
fn fake_run(log: &str) -> HeadlessRun {
    HeadlessRun {
        stdout: String::new(),
        log: log.to_string(),
        exited_cleanly: true,
        timed_out: false,
        idle_killed: false,
        stopped: false,
        exit_code: Some(0),
    }
}

// A `HeadlessRun` the vendor CLI itself failed (non-zero exit, no kill) — the
// shape a signed-out account produces.
fn fake_run_failed(log: &str) -> HeadlessRun {
    HeadlessRun {
        exited_cleanly: false,
        exit_code: Some(1),
        ..fake_run(log)
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
        stopped: false,
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
    remove_scratch_dir(&dir);
}

#[test]
fn plan_missing_auth_beats_generic() {
    let dir = tmp("auth");
    fs::create_dir_all(&dir).unwrap();
    let plan_path = dir.join("plan-does-not-exist.md");
    let charter = dir.join("charter.md");
    let cfg = plan_cfg(&dir, &plan_path, &charter);
    let err = run_plan_session(cfg, || Ok((fake_run("boom"), ())), |_| true, |_| None).unwrap_err();
    assert!(err.to_string().starts_with("AUTH_BAIL"), "got: {err}");
    remove_scratch_dir(&dir);
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
    remove_scratch_dir(&dir);
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
    assert_eq!(r.log, "");
    remove_scratch_dir(&dir);
}

#[test]
fn a_fresh_plan_call_writes_the_charter_and_drops_the_stale_plan_before_the_run() {
    let dir = tmp("fresh");
    fs::create_dir_all(&dir).unwrap();
    let plan_path = dir.join("plan.md");
    let charter = dir.join("charter.md");
    // A plan left by an earlier run (no trailer, so no resume) and an old
    // charter body that differs from the one this call writes.
    fs::write(&plan_path, "# stale plan\n").unwrap();
    fs::write(&charter, "old charter").unwrap();
    let cfg = plan_cfg(&dir, &plan_path, &charter);
    // What the vendor run sees when it starts is the payload.
    let (_, (stale_seen, charter_seen)) = run_plan_session(
        cfg,
        || {
            let seen = (plan_path.exists(), fs::read_to_string(&charter).unwrap());
            fs::write(&plan_path, "# fresh plan").unwrap();
            Ok((fake_run(""), seen))
        },
        |_| false,
        |_| None,
    )
    .unwrap()
    .expect("fresh plan yields Some");
    assert!(!stale_seen, "the stale plan is removed before the run");
    assert_eq!(
        charter_seen, "charter",
        "the charter is written before the run"
    );
    remove_scratch_dir(&dir);
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
    remove_scratch_dir(&dir);
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
    remove_scratch_dir(&dir);
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
    remove_scratch_dir(&dir);
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
    remove_scratch_dir(&dir);
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
    remove_scratch_dir(&dir);
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
    remove_scratch_dir(&dir);
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
    remove_scratch_dir(&dir);
}

fn exec_cfg(dir: &Path) -> ExecCfg<'_> {
    ExecCfg {
        ralphy_dir: dir,
        run_dir: dir,
        log_path: dir,
        auth_msg: "EXEC_AUTH",
    }
}

#[test]
fn exec_auth_bails_on_a_failed_run_else_ok() {
    let dir = tmp("exec");
    fs::create_dir_all(&dir).unwrap();
    let err = run_exec_session(
        exec_cfg(&dir),
        || Ok((fake_run_failed("nope"), ())),
        |_| true,
    )
    .unwrap_err();
    assert!(err.to_string().contains("EXEC_AUTH"), "got: {err}");

    let (_, p) = run_exec_session(
        exec_cfg(&dir),
        || Ok((fake_run_failed("fine"), 9u32)),
        |_| false,
    )
    .unwrap();
    assert_eq!(p, 9);
    remove_scratch_dir(&dir);
}

#[test]
fn exec_auth_is_not_consulted_after_a_clean_run() {
    // The regression oracle: the detector fires (`|_| true`) because the agent
    // read source documenting the vendor's own auth banner, yet the run exited
    // cleanly — so the work it just did must survive. Before the gate this
    // bailed and took the whole queue with it.
    let dir = tmp("exec-clean");
    fs::create_dir_all(&dir).unwrap();
    let (r, p) = run_exec_session(
        exec_cfg(&dir),
        || Ok((fake_run("…401 Unauthorized… (quoted from auth.rs)"), 9u32)),
        |_| true,
    )
    .expect("a clean run is never an auth failure");
    assert!(r.exited_cleanly);
    assert_eq!(p, 9);
    remove_scratch_dir(&dir);
}

#[test]
fn exec_auth_still_bails_on_a_killed_run() {
    // A timeout kill is also an unclean exit: the gate is "did not exit
    // cleanly", not "exited non-zero", so a killed child still reaches the
    // detector.
    let dir = tmp("exec-killed");
    fs::create_dir_all(&dir).unwrap();
    let err = run_exec_session(
        exec_cfg(&dir),
        || Ok((fake_run_killed("nope"), ())),
        |_| true,
    )
    .unwrap_err();
    assert!(err.to_string().contains("EXEC_AUTH"), "got: {err}");
    remove_scratch_dir(&dir);
}
