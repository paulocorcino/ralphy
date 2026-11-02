//! The protocol lint: one bounce, then a clean close or a report.

use super::*;

#[test]
fn protocol_lint_bounces_once_and_repaired_plan_closes_clean() {
    // ADR-0015: a protocol-dirty plan (unticked steps, missing sections) makes
    // the runner write `protocol-failure.md` and re-run the executor ONCE; the
    // well-behaved executor repairs the plan, the re-lint passes, and the issue
    // closes with an all-✓ lint block in the close comment.
    let repo = init_repo("lint-bounce");
    let queue = vec![issue(1)];
    let agent = ScriptedAgent::new(vec![Outcome::Done])
        .lint_dirty()
        .with_protocol_fix();
    let tracker = RecordingTracker::default();

    let report = run_queue(
        &cfg(&repo, "stamp-lint-bounce", false),
        &queue,
        &agent,
        &tracker,
        &ScriptedClock::never(),
    )
    .unwrap();

    assert_eq!(
        *agent.executed.borrow(),
        vec![1, 1],
        "exactly one bounce back to the executor"
    );
    let closes = tracker.closes.borrow();
    assert_eq!(closes.len(), 1, "repaired issue closed");
    let (_, comment) = &closes[0];
    assert!(
        comment.contains("## Protocol lint"),
        "lint result published: {comment}"
    );
    assert!(
        !comment.contains('\u{2717}'),
        "all checks green after the repair: {comment}"
    );
    // The bounce brief never leaks into the next issue.
    assert!(
        !repo.join(".ralphy").join("protocol-failure.md").exists(),
        "protocol-failure.md cleared after the lint settled"
    );
    assert!(report.stop.is_none());

    remove_scratch(&repo);
}

#[test]
fn protocol_lint_second_violation_closes_with_report() {
    // ADR-0015: an executor that ignores the bounce brief gets no second one —
    // the issue closes anyway (today's behavior) with the ✗ report and a
    // warning in the close comment for the human reviewer.
    let repo = init_repo("lint-unrepaired");
    let queue = vec![issue(1)];
    let agent = ScriptedAgent::new(vec![Outcome::Done]).lint_dirty(); // never repairs
    let tracker = RecordingTracker::default();

    let report = run_queue(
        &cfg(&repo, "stamp-lint-unrepaired", false),
        &queue,
        &agent,
        &tracker,
        &ScriptedClock::never(),
    )
    .unwrap();

    assert_eq!(
        *agent.executed.borrow(),
        vec![1, 1],
        "one bounce only — never a second"
    );
    let closes = tracker.closes.borrow();
    assert_eq!(closes.len(), 1, "second violation still closes");
    let (_, comment) = &closes[0];
    assert!(
        comment.contains('\u{2717}') && comment.contains('\u{26a0}'),
        "close comment carries the failed checks and the warning: {comment}"
    );
    assert!(report.stop.is_none());

    remove_scratch(&repo);
}
