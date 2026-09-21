//! Usage limits: the stop-on-limit stance, plan-limit resumes, the no-reset
//! wait and the deadline during a wait.

use super::*;

#[test]
fn stop_on_limit_opt_out_stops_as_limit() {
    // With `--stop-on-limit`, a usage limit stops and reports the reset (the
    // pre-auto-resume behaviour) instead of waiting.
    let repo = init_repo("limit");
    let queue = vec![issue(10)];
    let agent = ScriptedAgent::new(vec![Outcome::Limit(Some("15:00".into()))]);
    let tracker = RecordingTracker::default();
    let clock = ScriptedClock::never();

    let report = run_queue(
        &cfg_stop_on_limit(&repo, "stamp-limit"),
        &queue,
        &agent,
        &tracker,
        &clock,
    )
    .unwrap();

    match report.stop {
        Some(StopReason::Limit { number, reset }) => {
            assert_eq!(number, 10);
            assert_eq!(reset, Some("15:00".into()));
        }
        other => panic!("expected Limit stop, got {other:?}"),
    }
    // The opt-out never waits.
    assert_eq!(
        *agent.executed.borrow(),
        vec![10],
        "executed once, no resume"
    );
    assert!(
        clock.waited_for.borrow().is_empty(),
        "stop-on-limit never calls wait_for_reset"
    );

    fs::remove_dir_all(&repo).ok();
}

#[test]
fn plan_limit_with_stop_on_limit_stops_and_reports() {
    // A usage limit during *planning* (before any plan is written) under
    // `--stop-on-limit` (always the case for Codex) stops the run and reports the
    // reset — it never waits and never executes.
    let repo = init_repo("plan-limit-stop");
    let queue = vec![issue(10)];
    let agent = ScriptedAgent::new(vec![Outcome::Done])
        .with_plan_scripts(vec![PlanScript::Limit(Some("12:23 AM".into()))]);
    let tracker = RecordingTracker::default();
    let clock = ScriptedClock::never();

    let report = run_queue(
        &cfg_stop_on_limit(&repo, "stamp-plan-limit"),
        &queue,
        &agent,
        &tracker,
        &clock,
    )
    .unwrap();

    match report.stop {
        Some(StopReason::Limit { number, reset }) => {
            assert_eq!(number, 10);
            assert_eq!(reset, Some("12:23 AM".into()));
        }
        other => panic!("expected Limit stop, got {other:?}"),
    }
    assert_eq!(*agent.plan_attempts.borrow(), 1, "planned once, no resume");
    assert!(
        agent.executed.borrow().is_empty(),
        "a plan-time limit never reaches execute"
    );
    assert!(
        clock.waited_for.borrow().is_empty(),
        "stop-on-limit never waits for a plan-time reset"
    );
    // The limit is recorded on the issue result, not swallowed as a hard error.
    let worked = &report.worked;
    assert_eq!(worked.len(), 1);
    assert_eq!(worked[0].number, 10);
    assert_eq!(
        worked[0].outcome,
        Some(Outcome::Limit(Some("12:23 AM".into())))
    );
    assert!(!worked[0].closed);

    fs::remove_dir_all(&repo).ok();
}

#[test]
fn per_phase_limit_plans_resume_but_execute_stops() {
    // The split-run asymmetry (ADR-0009): with `stop_on_limit_plan = false` and
    // `stop_on_limit_exec = true`, a plan-time limit auto-resumes (the planner
    // waits and re-plans) while an execute-time limit stops the run and reports
    // the reset. A single `stop_on_limit` field could not express this split — so
    // this test FAILS before the field was split and PASSES after.
    let repo = init_repo("split-limit");
    let queue = vec![issue(8)];
    // One plan-time limit (resolves via resume), then an execute-time limit (stops).
    let agent = ScriptedAgent::new(vec![Outcome::Limit(Some("15:00".into()))])
        .with_plan_scripts(vec![PlanScript::Limit(Some("12:00".into()))]);
    let tracker = RecordingTracker::default();
    let clock = ScriptedClock::never();

    let report = run_queue(
        &cfg_split_limit(&repo, "stamp-split-limit"),
        &queue,
        &agent,
        &tracker,
        &clock,
    )
    .unwrap();

    // Plan auto-resumed: two plan attempts, and the only wait was for the plan
    // reset — the execute-time limit stopped immediately without waiting.
    assert_eq!(
        *agent.plan_attempts.borrow(),
        2,
        "plan auto-resumed once through the plan-time limit"
    );
    assert_eq!(
        *clock.waited_for.borrow(),
        vec!["12:00".to_string()],
        "waited only for the plan reset; the execute limit never waits"
    );
    assert_eq!(
        *agent.executed.borrow(),
        vec![8],
        "executed once, then the execute-time limit stopped the run"
    );

    // Execute stopped as a reported limit carrying the execute-time reset.
    match report.stop {
        Some(StopReason::Limit { number, reset }) => {
            assert_eq!(number, 8);
            assert_eq!(reset, Some("15:00".into()));
        }
        other => panic!("expected Limit stop from the execute phase, got {other:?}"),
    }
    assert!(
        tracker.closes.borrow().is_empty(),
        "a limit-stopped issue is never closed"
    );

    fs::remove_dir_all(&repo).ok();
}

#[test]
fn plan_limit_auto_resumes_by_replanning() {
    // With auto-resume (the default), a plan-time limit waits for the reset and
    // re-plans the SAME issue, then proceeds to execute and close it green.
    let repo = init_repo("plan-limit-resume");
    let queue = vec![issue(7)];
    let agent = ScriptedAgent::new(vec![Outcome::Done])
        .with_plan_scripts(vec![PlanScript::Limit(Some("15:00".into()))]);
    let tracker = RecordingTracker::default();
    let clock = ScriptedClock::never();

    let report = run_queue(
        &cfg(&repo, "stamp-plan-resume", false),
        &queue,
        &agent,
        &tracker,
        &clock,
    )
    .unwrap();

    assert!(report.stop.is_none(), "resume then green leaves no stop");
    assert_eq!(
        *agent.plan_attempts.borrow(),
        2,
        "planned twice: limit, then success after the reset"
    );
    assert_eq!(*clock.waited_for.borrow(), vec!["15:00".to_string()]);
    assert_eq!(*agent.executed.borrow(), vec![7], "executed after re-plan");
    let closed: Vec<u64> = tracker.closes.borrow().iter().map(|(n, _)| *n).collect();
    assert_eq!(closed, vec![7], "green issue closed after the resume");

    fs::remove_dir_all(&repo).ok();
}

#[test]
fn repeated_plan_limits_hit_the_cap_and_stop() {
    // A reset that never actually clears (e.g. a past/garbage hint) must not spin
    // the resume loop forever: after MAX_PLAN_LIMIT_RESUMES no-progress waits the
    // runner stops and reports the limit.
    let repo = init_repo("plan-limit-cap");
    let queue = vec![issue(3)];
    let agent = ScriptedAgent::new(vec![Outcome::Done]).with_plan_scripts(vec![
        PlanScript::Limit(Some("09:00".into())),
        PlanScript::Limit(Some("09:00".into())),
        PlanScript::Limit(Some("09:00".into())),
    ]);
    let tracker = RecordingTracker::default();
    let clock = ScriptedClock::never();

    let report = run_queue(
        &cfg(&repo, "stamp-plan-cap", false),
        &queue,
        &agent,
        &tracker,
        &clock,
    )
    .unwrap();

    assert_eq!(
        *agent.plan_attempts.borrow(),
        3,
        "two resumes then the third limit hits the cap"
    );
    assert_eq!(
        *clock.waited_for.borrow(),
        vec!["09:00".to_string(), "09:00".to_string()],
        "waited twice before the cap stopped it"
    );
    assert!(agent.executed.borrow().is_empty(), "never executed");
    match report.stop {
        Some(StopReason::Limit { number, reset }) => {
            assert_eq!(number, 3);
            assert_eq!(reset, Some("09:00".into()));
        }
        other => panic!("expected Limit stop, got {other:?}"),
    }

    fs::remove_dir_all(&repo).ok();
}

#[test]
fn plan_limit_deadline_beats_resume() {
    // A plan-time reset landing past the run deadline stops the run (deadline beats
    // resume) instead of waiting, just like an execute-time limit.
    let repo = init_repo("plan-limit-deadline");
    let queue = vec![issue(4)];
    let agent = ScriptedAgent::new(vec![Outcome::Done])
        .with_plan_scripts(vec![PlanScript::Limit(Some("15:00".into()))]);
    let tracker = RecordingTracker::default();
    let clock = ScriptedClock::deadline_on_wait();

    let report = run_queue(
        &cfg(&repo, "stamp-plan-deadline", false),
        &queue,
        &agent,
        &tracker,
        &clock,
    )
    .unwrap();

    assert!(matches!(report.stop, Some(StopReason::Deadline)));
    assert_eq!(*clock.waited_for.borrow(), vec!["15:00".to_string()]);
    assert_eq!(*agent.plan_attempts.borrow(), 1, "planned once, then cut");
    assert!(agent.executed.borrow().is_empty(), "never executed");

    fs::remove_dir_all(&repo).ok();
}

#[test]
fn limit_no_reset_synthesizes_a_wait_and_auto_resumes() {
    // A limit with no parseable reset (Kimi's 403 account block) no longer stops:
    // it parks a synthesised ~30-min window and re-runs execute() when the wait
    // returns (ADR-0030). Two consecutive no-commit synthetic limits also prove the
    // progress cap is skipped for this path (B2) — a real reset would abandon at two,
    // but an account-wide pause is the human's call, so the loop resumes until Done.
    let repo = init_repo("limit-noreset-resume");
    let queue = vec![issue(11)];
    let agent = ScriptedAgent::scripted(vec![
        (Outcome::Limit(None), false),
        (Outcome::Limit(None), false),
        (Outcome::Done, true),
    ]);
    let tracker = RecordingTracker::default();
    let clock = ScriptedClock::never();

    let report = run_queue(
        &cfg(&repo, "stamp-limit-none", false),
        &queue,
        &agent,
        &tracker,
        &clock,
    )
    .unwrap();

    assert_eq!(
        *agent.executed.borrow(),
        vec![11, 11, 11],
        "execute re-ran after each synthetic wait; the cap did not abandon the issue"
    );
    assert!(
        report.stop.is_none(),
        "a no-reset limit auto-resumes, not a stop"
    );
    // A synthetic, parseable reset target was passed to wait_for_reset each cycle.
    let waited = clock.waited_for.borrow();
    assert_eq!(waited.len(), 2, "waited once per no-reset limit");
    assert!(
        waited.iter().all(|w| !w.is_empty()),
        "each wait carried a synthesised reset target, got {waited:?}"
    );

    fs::remove_dir_all(&repo).ok();
}

#[test]
fn limit_no_reset_stops_when_stop_on_limit() {
    // `--stop-on-limit` is the opt-out (e.g. CI that must not hang): a no-reset limit
    // stops and reports with a None reset instead of parking a synthetic wait.
    let repo = init_repo("limit-noreset-stop");
    let queue = vec![issue(11)];
    let agent = ScriptedAgent::new(vec![Outcome::Limit(None)]);
    let tracker = RecordingTracker::default();

    let report = run_queue(
        &cfg_stop_on_limit(&repo, "stamp-limit-none-stop"),
        &queue,
        &agent,
        &tracker,
        &ScriptedClock::never(),
    )
    .unwrap();

    match report.stop {
        Some(StopReason::Limit { number, reset }) => {
            assert_eq!(number, 11);
            assert_eq!(reset, None);
        }
        other => panic!("expected Limit stop with None reset, got {other:?}"),
    }

    fs::remove_dir_all(&repo).ok();
}

#[test]
fn limit_resume_reexecutes_same_issue_only() {
    // A scripted `Limit(Some(reset))` then `Done` must re-run execute() for the
    // SAME issue (executed [n, n]) without advancing the queue or re-planning,
    // and the issue closes green. The clock records the parsed reset it waited on.
    let repo = init_repo("limit-resume");
    let queue = vec![issue(20)];
    let agent = ScriptedAgent::scripted(vec![
        (Outcome::Limit(Some("15:00".into())), true),
        (Outcome::Done, true),
    ]);
    let tracker = RecordingTracker::default();
    let clock = ScriptedClock::never();

    let report = run_queue(
        &cfg(&repo, "stamp-resume", false),
        &queue,
        &agent,
        &tracker,
        &clock,
    )
    .unwrap();

    // execute() ran twice for #20; plan() ran once (never re-planned).
    assert_eq!(*agent.executed.borrow(), vec![20, 20], "execute-only retry");
    assert_eq!(*agent.planned.borrow(), vec![20], "plan() never re-run");
    // The queue was not advanced and the issue closed green.
    assert!(report.stop.is_none(), "resume → green, no stop");
    let closes: Vec<u64> = tracker.closes.borrow().iter().map(|(n, _)| *n).collect();
    assert_eq!(closes, vec![20], "same issue closed green after resume");
    // wait_for_reset was called once, with the parsed reset string.
    assert_eq!(*clock.waited_for.borrow(), vec!["15:00".to_string()]);

    fs::remove_dir_all(&repo).ok();
}

#[test]
fn two_no_commit_limit_resumes_abandon_the_issue() {
    // Two consecutive limit-resumes that commit nothing abandon the issue with
    // StopReason::Limit; a commit between resumes resets the counter.
    {
        let repo = init_repo("limit-cap");
        let queue = vec![issue(30)];
        // Both resumes make no commit (HEAD unchanged) → cap fires.
        let agent = ScriptedAgent::scripted(vec![
            (Outcome::Limit(Some("09:00".into())), false),
            (Outcome::Limit(Some("09:00".into())), false),
        ]);
        let tracker = RecordingTracker::default();
        let clock = ScriptedClock::never();

        let report = run_queue(
            &cfg(&repo, "stamp-cap", false),
            &queue,
            &agent,
            &tracker,
            &clock,
        )
        .unwrap();

        assert_eq!(
            *agent.executed.borrow(),
            vec![30, 30],
            "two resumes then cap"
        );
        // The cap is reached via the resume path, not by skipping the wait.
        assert_eq!(
            *clock.waited_for.borrow(),
            vec!["09:00".to_string()],
            "the first limit waited for its reset before the cap fired"
        );
        match report.stop {
            Some(StopReason::Limit { number, reset }) => {
                assert_eq!(number, 30);
                assert_eq!(reset, Some("09:00".into()));
            }
            other => panic!("expected Limit stop from the progress cap, got {other:?}"),
        }
        assert!(tracker.closes.borrow().is_empty(), "abandoned, not closed");

        fs::remove_dir_all(&repo).ok();
    }

    // A commit between the two no-commit resumes resets the counter, so the cap
    // does not fire at the second resume; the run continues to green.
    {
        let repo = init_repo("limit-cap-reset");
        let queue = vec![issue(31)];
        let agent = ScriptedAgent::scripted(vec![
            (Outcome::Limit(Some("09:00".into())), false), // streak 1
            (Outcome::Limit(Some("09:00".into())), true),  // commit → streak 0
            (Outcome::Limit(Some("09:00".into())), false), // streak 1
            (Outcome::Done, true),                         // green before cap
        ]);
        let tracker = RecordingTracker::default();
        let clock = ScriptedClock::never();

        let report = run_queue(
            &cfg(&repo, "stamp-cap-reset", false),
            &queue,
            &agent,
            &tracker,
            &clock,
        )
        .unwrap();

        assert_eq!(
            *agent.executed.borrow(),
            vec![31, 31, 31, 31],
            "commit between resumes lets the run continue"
        );
        assert!(report.stop.is_none(), "reached green, no abandon");
        let closes: Vec<u64> = tracker.closes.borrow().iter().map(|(n, _)| *n).collect();
        assert_eq!(closes, vec![31], "issue closed green");

        fs::remove_dir_all(&repo).ok();
    }
}

#[test]
fn deadline_during_wait_short_circuits_with_deadline() {
    // A reset that lands beyond the run deadline (clock reports DeadlinePassed
    // from wait_for_reset) stops the run with StopReason::Deadline, without a
    // further execute.
    let repo = init_repo("limit-deadline");
    let queue = vec![issue(40)];
    let agent = ScriptedAgent::scripted(vec![(Outcome::Limit(Some("15:00".into())), true)]);
    let tracker = RecordingTracker::default();
    let clock = ScriptedClock::deadline_on_wait();

    let report = run_queue(
        &cfg(&repo, "stamp-deadline-wait", false),
        &queue,
        &agent,
        &tracker,
        &clock,
    )
    .unwrap();

    assert_eq!(
        *agent.executed.borrow(),
        vec![40],
        "no resume past the deadline"
    );
    assert_eq!(*clock.waited_for.borrow(), vec!["15:00".to_string()]);
    assert!(
        matches!(report.stop, Some(StopReason::Deadline)),
        "deadline beats resume: {:?}",
        report.stop
    );

    fs::remove_dir_all(&repo).ok();
}
