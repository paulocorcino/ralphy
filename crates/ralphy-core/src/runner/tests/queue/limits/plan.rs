//! Plan-phase usage limits: resumes, the cap and the deadline.

use super::*;

#[test]
fn per_phase_limit_plans_resume_but_execute_stops() {
    // The split-run asymmetry (ADR-0009): with `stop_on_limit_plan = false` and
    // `stop_on_limit_exec = true`, a plan-time limit auto-resumes (the planner
    // waits and re-plans) while an execute-time limit stops the run and reports
    // the reset. A single `stop_on_limit` field could not express this split — so
    // this test FAILS before the field was split and PASSES after.
    let root = test_dir("q-split-limit");
    let queue = vec![test_issue(8)];
    // One plan-time limit (resolves via resume), then an execute-time limit (stops).
    let agent = ScriptedAgent::new(vec![Outcome::Limit(Some(reset_text("15:00")))])
        .with_plan_scripts(vec![PlanScript::Limit(Some(reset_text("12:00")))]);
    let tracker = RecordingTracker::default();
    let clock = RecordingClock::never();
    let cfg = QueueConfig {
        stop_on_limit_exec: true,
        ..test_cfg(&root, "stamp-split-limit")
    };

    let (report, _) = run_on_fake_with(&cfg, &queue, &agent, &tracker, &clock);

    // Plan auto-resumed: two plan attempts, and the only wait was for the plan
    // reset — the execute-time limit stopped immediately without waiting.
    assert_eq!(
        agent.plan_attempts.get(),
        2,
        "plan auto-resumed once through the plan-time limit"
    );
    assert_eq!(
        *clock.waited_for.borrow(),
        vec!["12:00".to_owned()],
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
            assert_eq!(reset, Some(reset_text("15:00")));
        }
        other => panic!("expected Limit stop from the execute phase, got {other:?}"),
    }
    assert!(
        tracker.closes.borrow().is_empty(),
        "a limit-stopped issue is never closed"
    );

    remove_scratch(&root);
}

#[test]
fn plan_limit_auto_resumes_by_replanning() {
    // With auto-resume (the default), a plan-time limit waits for the reset and
    // re-plans the SAME issue, then proceeds to execute and close it green.
    let root = test_dir("q-plan-limit-resume");
    let queue = vec![test_issue(7)];
    let agent = ScriptedAgent::new(vec![Outcome::Done])
        .with_plan_scripts(vec![PlanScript::Limit(Some(reset_text("15:00")))]);
    let tracker = RecordingTracker::default();
    let clock = RecordingClock::never();

    let (report, _) = run_on_fake_with(
        &test_cfg(&root, "stamp-plan-resume"),
        &queue,
        &agent,
        &tracker,
        &clock,
    );

    assert!(report.stop.is_none(), "resume then green leaves no stop");
    assert_eq!(
        agent.plan_attempts.get(),
        2,
        "planned twice: limit, then success after the reset"
    );
    assert_eq!(*clock.waited_for.borrow(), vec!["15:00".to_owned()]);
    assert_eq!(*agent.executed.borrow(), vec![7], "executed after re-plan");
    assert_eq!(
        closed(&tracker),
        vec![7],
        "green issue closed after the resume"
    );

    remove_scratch(&root);
}

#[test]
fn repeated_plan_limits_hit_the_cap_and_stop() {
    // A reset that never actually clears (e.g. a past/garbage hint) must not spin
    // the resume loop forever: after MAX_PLAN_LIMIT_RESUMES no-progress waits the
    // runner stops and reports the limit.
    let root = test_dir("q-plan-limit-cap");
    let queue = vec![test_issue(3)];
    let agent = ScriptedAgent::new(vec![Outcome::Done]).with_plan_scripts(vec![
        PlanScript::Limit(Some(reset_text("09:00"))),
        PlanScript::Limit(Some(reset_text("09:00"))),
        PlanScript::Limit(Some(reset_text("09:00"))),
    ]);
    let tracker = RecordingTracker::default();
    let clock = RecordingClock::never();

    let (report, _) = run_on_fake_with(
        &test_cfg(&root, "stamp-plan-cap"),
        &queue,
        &agent,
        &tracker,
        &clock,
    );

    assert_eq!(
        agent.plan_attempts.get(),
        3,
        "two resumes then the third limit hits the cap"
    );
    assert_eq!(
        *clock.waited_for.borrow(),
        vec!["09:00".to_owned(), "09:00".to_owned()],
        "waited twice before the cap stopped it"
    );
    assert!(agent.executed.borrow().is_empty(), "never executed");
    match report.stop {
        Some(StopReason::Limit { number, reset }) => {
            assert_eq!(number, 3);
            assert_eq!(reset, Some(reset_text("09:00")));
        }
        other => panic!("expected Limit stop, got {other:?}"),
    }

    remove_scratch(&root);
}

#[test]
fn plan_limit_deadline_beats_resume() {
    // A plan-time reset landing past the run deadline stops the run (deadline beats
    // resume) instead of waiting, just like an execute-time limit.
    let root = test_dir("q-plan-limit-deadline");
    let queue = vec![test_issue(4)];
    let agent = ScriptedAgent::new(vec![Outcome::Done])
        .with_plan_scripts(vec![PlanScript::Limit(Some(reset_text("15:00")))]);
    let tracker = RecordingTracker::default();
    let clock = RecordingClock::deadline_on_wait();

    let (report, _) = run_on_fake_with(
        &test_cfg(&root, "stamp-plan-deadline"),
        &queue,
        &agent,
        &tracker,
        &clock,
    );

    assert!(matches!(report.stop, Some(StopReason::Deadline)));
    assert_eq!(*clock.waited_for.borrow(), vec!["15:00".to_owned()]);
    assert_eq!(agent.plan_attempts.get(), 1, "planned once, then cut");
    assert!(agent.executed.borrow().is_empty(), "never executed");

    remove_scratch(&root);
}
