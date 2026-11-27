//! Usage limits: the stop-on-limit stance, plan-limit resumes, the no-reset
//! wait and the deadline during a wait.

use std::path::Path;

use super::*;

mod plan;

/// A config that stops on a usage limit in both phases.
fn cfg_stop_on_limit(root: &Path, stamp: &str) -> QueueConfig {
    QueueConfig {
        stop_on_limit_plan: true,
        stop_on_limit_exec: true,
        ..test_cfg(root, stamp)
    }
}

#[test]
fn stop_on_limit_opt_out_stops_as_limit() {
    // With `--stop-on-limit` (the opt-out, e.g. CI that must not hang), a usage
    // limit stops and reports the reset (the pre-auto-resume behaviour) instead
    // of waiting. A no-reset limit reports a `None` reset instead of parking a
    // synthetic wait.
    // (case, issue, the limit's reset)
    let rows: [(&str, u64, Option<&str>); 2] = [
        ("limit with a reset", 10, Some("15:00")),
        ("limit without a reset", 11, None),
    ];
    for (case, n, reset_at) in rows {
        let root = test_dir(&format!("q-limit-{n}"));
        let queue = vec![test_issue(n)];
        let agent = ScriptedAgent::new(vec![Outcome::Limit(reset_at.map(reset_text))]);
        let tracker = RecordingTracker::default();
        let clock = RecordingClock::never();

        let (report, _) = run_on_fake_with(
            &cfg_stop_on_limit(&root, &format!("stamp-limit-{n}")),
            &queue,
            &agent,
            &tracker,
            &clock,
        );

        match report.stop {
            Some(StopReason::Limit { number, reset }) => {
                assert_eq!(number, n, "{case}");
                assert_eq!(reset.map(|r| r.text).as_deref(), reset_at, "{case}");
            }
            other => panic!("{case}: expected Limit stop, got {other:?}"),
        }
        // The opt-out never waits.
        assert_eq!(
            *agent.executed.borrow(),
            vec![n],
            "{case}: executed once, no resume"
        );
        assert!(
            clock.waited_for.borrow().is_empty(),
            "{case}: stop-on-limit never calls wait_for_reset"
        );

        remove_scratch(&root);
    }
}

#[test]
fn plan_limit_with_stop_on_limit_stops_and_reports() {
    // A usage limit during *planning* (before any plan is written) under
    // `--stop-on-limit` (always the case for Codex) stops the run and reports the
    // reset — it never waits and never executes.
    let root = test_dir("q-plan-limit-stop");
    let queue = vec![test_issue(10)];
    let agent = ScriptedAgent::new(vec![Outcome::Done])
        .with_plan_scripts(vec![PlanScript::Limit(Some(reset_text("12:23 AM")))]);
    let tracker = RecordingTracker::default();
    let clock = RecordingClock::never();

    let (report, _) = run_on_fake_with(
        &cfg_stop_on_limit(&root, "stamp-plan-limit"),
        &queue,
        &agent,
        &tracker,
        &clock,
    );

    match report.stop {
        Some(StopReason::Limit { number, reset }) => {
            assert_eq!(number, 10);
            assert_eq!(reset, Some(reset_text("12:23 AM")));
        }
        other => panic!("expected Limit stop, got {other:?}"),
    }
    assert_eq!(agent.plan_attempts.get(), 1, "planned once, no resume");
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
        Some(Outcome::Limit(Some(reset_text("12:23 AM"))))
    );
    assert!(!worked[0].closed);

    remove_scratch(&root);
}

#[test]
fn limit_no_reset_synthesizes_a_wait_and_auto_resumes() {
    // A limit with no parseable reset (Kimi's 403 account block) no longer stops:
    // it parks a synthesised ~30-min window and re-runs execute() when the wait
    // returns (ADR-0030). Two consecutive no-commit synthetic limits also prove the
    // progress cap is skipped for this path (B2) — a real reset would abandon at two,
    // but an account-wide pause is the human's call, so the loop resumes until Done.
    let root = test_dir("q-limit-noreset-resume");
    let queue = vec![test_issue(11)];
    let agent = ScriptedAgent::scripted(vec![
        (Outcome::Limit(None), false),
        (Outcome::Limit(None), false),
        (Outcome::Done, true),
    ]);
    let tracker = RecordingTracker::default();
    let clock = RecordingClock::never();

    let (report, _) = run_on_fake_with(
        &test_cfg(&root, "stamp-limit-none"),
        &queue,
        &agent,
        &tracker,
        &clock,
    );

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

    remove_scratch(&root);
}

#[test]
fn limit_resume_reexecutes_same_issue_only() {
    // A scripted `Limit(Some(reset))` then `Done` must re-run execute() for the
    // SAME issue (executed [n, n]) without advancing the queue or re-planning,
    // and the issue closes green. The clock records the parsed reset it waited on.
    let root = test_dir("q-limit-resume");
    let queue = vec![test_issue(20)];
    let agent = ScriptedAgent::scripted(vec![
        (Outcome::Limit(Some(reset_text("15:00"))), true),
        (Outcome::Done, true),
    ]);
    let tracker = RecordingTracker::default();
    let clock = RecordingClock::never();

    let (report, _) = run_on_fake_with(
        &test_cfg(&root, "stamp-resume"),
        &queue,
        &agent,
        &tracker,
        &clock,
    );

    // execute() ran twice for #20; plan() ran once (never re-planned).
    assert_eq!(*agent.executed.borrow(), vec![20, 20], "execute-only retry");
    assert_eq!(*agent.planned.borrow(), vec![20], "plan() never re-run");
    // The queue was not advanced and the issue closed green.
    assert!(report.stop.is_none(), "resume → green, no stop");
    assert_eq!(
        closed(&tracker),
        vec![20],
        "same issue closed green after resume"
    );
    // wait_for_reset was called once, with the parsed reset string.
    assert_eq!(*clock.waited_for.borrow(), vec!["15:00".to_owned()]);

    remove_scratch(&root);
}

#[test]
fn two_no_commit_limit_resumes_abandon_the_issue() {
    // Two consecutive limit-resumes that commit nothing abandon the issue with
    // StopReason::Limit; a commit between resumes resets the counter.
    {
        let root = test_dir("q-limit-cap");
        let queue = vec![test_issue(30)];
        // Both resumes make no commit (HEAD unchanged) → cap fires.
        let agent = ScriptedAgent::scripted(vec![
            (Outcome::Limit(Some(reset_text("09:00"))), false),
            (Outcome::Limit(Some(reset_text("09:00"))), false),
        ]);
        let tracker = RecordingTracker::default();
        let clock = RecordingClock::never();

        let (report, _) = run_on_fake_with(
            &test_cfg(&root, "stamp-cap"),
            &queue,
            &agent,
            &tracker,
            &clock,
        );

        assert_eq!(
            *agent.executed.borrow(),
            vec![30, 30],
            "two resumes then cap"
        );
        // The cap is reached via the resume path, not by skipping the wait.
        assert_eq!(
            *clock.waited_for.borrow(),
            vec!["09:00".to_owned()],
            "the first limit waited for its reset before the cap fired"
        );
        match report.stop {
            Some(StopReason::Limit { number, reset }) => {
                assert_eq!(number, 30);
                assert_eq!(reset, Some(reset_text("09:00")));
            }
            other => panic!("expected Limit stop from the progress cap, got {other:?}"),
        }
        assert!(tracker.closes.borrow().is_empty(), "abandoned, not closed");

        remove_scratch(&root);
    }

    // A commit between the two no-commit resumes resets the counter, so the cap
    // does not fire at the second resume; the run continues to green.
    {
        let root = test_dir("q-limit-cap-reset");
        let queue = vec![test_issue(31)];
        let agent = ScriptedAgent::scripted(vec![
            (Outcome::Limit(Some(reset_text("09:00"))), false), // streak 1
            (Outcome::Limit(Some(reset_text("09:00"))), true),  // commit → streak 0
            (Outcome::Limit(Some(reset_text("09:00"))), false), // streak 1
            (Outcome::Done, true),                              // green before cap
        ]);
        let tracker = RecordingTracker::default();
        let clock = RecordingClock::never();

        let (report, _) = run_on_fake_with(
            &test_cfg(&root, "stamp-cap-reset"),
            &queue,
            &agent,
            &tracker,
            &clock,
        );

        assert_eq!(
            *agent.executed.borrow(),
            vec![31, 31, 31, 31],
            "commit between resumes lets the run continue"
        );
        assert!(report.stop.is_none(), "reached green, no abandon");
        assert_eq!(closed(&tracker), vec![31], "issue closed green");

        remove_scratch(&root);
    }
}

#[test]
fn deadline_during_wait_short_circuits_with_deadline() {
    // A reset that lands beyond the run deadline (clock reports DeadlinePassed
    // from wait_for_reset) stops the run with StopReason::Deadline, without a
    // further execute.
    let root = test_dir("q-limit-deadline");
    let queue = vec![test_issue(40)];
    let agent = ScriptedAgent::scripted(vec![(Outcome::Limit(Some(reset_text("15:00"))), true)]);
    let tracker = RecordingTracker::default();
    let clock = RecordingClock::deadline_on_wait();

    let (report, _) = run_on_fake_with(
        &test_cfg(&root, "stamp-deadline-wait"),
        &queue,
        &agent,
        &tracker,
        &clock,
    );

    assert_eq!(
        *agent.executed.borrow(),
        vec![40],
        "no resume past the deadline"
    );
    assert_eq!(*clock.waited_for.borrow(), vec!["15:00".to_owned()]);
    assert!(
        matches!(report.stop, Some(StopReason::Deadline)),
        "deadline beats resume: {:?}",
        report.stop
    );

    remove_scratch(&root);
}

/// A run where the first execute is `Done` and the next one (the protocol
/// bounce, or the verify repair) hits a usage limit with a reset. The run
/// stops on that limit, the issue is recorded as non-green and stays open.
fn assert_second_execute_limit_stops_the_run(
    root: &Path,
    agent: &ScriptedAgent,
    stamp: &str,
    number: u64,
) {
    let tracker = RecordingTracker::default();

    let (report, _) = run_on_fake(
        &cfg_stop_on_limit(root, stamp),
        &[test_issue(number)],
        agent,
        &tracker,
    );

    assert_eq!(
        *agent.executed.borrow(),
        vec![number, number],
        "the first execute, then the one that hit the limit"
    );
    let stop = report.stop.as_ref();
    assert!(
        matches!(stop, Some(StopReason::Limit { .. })),
        "expected a Limit stop, got {stop:?}"
    );
    if let Some(StopReason::Limit { number: n, reset }) = stop {
        assert_eq!(*n, number);
        assert_eq!(*reset, Some(reset_text("15:00")));
    }
    assert_eq!(report.worked.len(), 1);
    let worked = &report.worked[0];
    assert_eq!(worked.status, ResultStatus::NonGreen);
    assert!(!worked.closed, "a limit never closes the issue");
    assert!(
        tracker.closes.borrow().is_empty(),
        "an issue stopped on a limit stays open"
    );

    remove_scratch(root);
}

#[test]
fn a_limit_during_the_protocol_bounce_stops_the_run() {
    let root = test_dir("q-limit-protocol");
    // A protocol-dirty plan makes the runner bounce the issue once; that
    // second execute hits the limit.
    let agent = ScriptedAgent::new(vec![
        Outcome::Done,
        Outcome::Limit(Some(reset_text("15:00"))),
    ])
    .lint_dirty();

    assert_second_execute_limit_stops_the_run(&root, &agent, "stamp-limit-protocol", 50);
}

#[test]
fn a_limit_during_a_verify_repair_stops_the_run() {
    let root = test_dir("q-limit-verify-repair");
    // The gate fails after the first execute; the repair hits the limit.
    let extra = format!("## Verify\n\n{}\n", verify_fail_line());
    let agent = ScriptedAgent::new(vec![
        Outcome::Done,
        Outcome::Limit(Some(reset_text("15:00"))),
    ])
    .with_plan_extra(extra);

    assert_second_execute_limit_stops_the_run(&root, &agent, "stamp-limit-verify", 51);
}
