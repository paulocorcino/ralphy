//! The queue's basic lifecycle: order, close, the first non-green issue, dry
//! runs, the deadline and the run report.

use super::*;

#[test]
fn works_issues_in_order_and_closes_each_green() {
    let repo = init_repo("green");
    let queue = vec![issue(2), issue(5), issue(9)];
    let agent = ScriptedAgent::new(vec![Outcome::Done, Outcome::Done, Outcome::Done]);
    let tracker = RecordingTracker::default();

    let report = run_queue(
        &cfg(&repo, "stamp-green", false),
        &queue,
        &agent,
        &tracker,
        &ScriptedClock::never(),
    )
    .unwrap();

    // Worked ascending, all three executed.
    assert_eq!(*agent.executed.borrow(), vec![2, 5, 9]);
    assert!(report.stop.is_none(), "no stop on an all-green run");

    // Each green issue closed exactly once, comment names the run branch, and no
    // label mutation is even possible (the tracker has no label method).
    let closes = tracker.closes.borrow();
    let numbers: Vec<u64> = closes.iter().map(|(n, _)| *n).collect();
    assert_eq!(numbers, vec![2, 5, 9], "every green issue closed once");
    for (_, comment) in closes.iter() {
        assert!(
            comment.contains(&report.branch),
            "comment must point at the run branch: {comment}"
        );
    }

    // Clean New-mode run: the repo is returned to the original branch and the run
    // branch is kept (not deleted) for the human to review and merge by hand.
    assert_eq!(current_branch(&repo), "main", "returned to original branch");
    assert!(
        branch_exists(&repo, &report.branch),
        "run branch kept after a clean run"
    );

    fs::remove_dir_all(&repo).ok();
}

#[test]
fn first_non_green_stops_run_and_leaves_later_issues_untouched() {
    let repo = init_repo("stop");
    let queue = vec![issue(1), issue(2), issue(3)];
    // #1 green, #2 blocked → #3 must never be touched.
    let agent = ScriptedAgent::new(vec![Outcome::Done, Outcome::Blocked("nope".into())]);
    let tracker = RecordingTracker::default();

    let report = run_queue(
        &cfg(&repo, "stamp-stop", false),
        &queue,
        &agent,
        &tracker,
        &ScriptedClock::never(),
    )
    .unwrap();

    assert_eq!(*agent.executed.borrow(), vec![1, 2], "#3 never executed");
    assert_eq!(*agent.planned.borrow(), vec![1, 2], "#3 never planned");

    // Earlier green issue stays closed; the blocked one is not closed.
    let numbers: Vec<u64> = tracker.closes.borrow().iter().map(|(n, _)| *n).collect();
    assert_eq!(numbers, vec![1], "only the green issue closed");

    match report.stop {
        Some(StopReason::NonGreen { number, outcome }) => {
            assert_eq!(number, 2);
            assert_eq!(outcome, Outcome::Blocked("nope".into()));
        }
        other => panic!("expected NonGreen stop, got {other:?}"),
    }

    // Branch handed back with the green commit on it.
    assert_eq!(current_branch(&repo), report.branch);

    fs::remove_dir_all(&repo).ok();
}

#[test]
fn dry_run_closes_nothing_and_restores() {
    let repo = init_repo("dry");
    let queue = vec![issue(1), issue(2)];
    let agent = ScriptedAgent::new(vec![Outcome::Done, Outcome::Done]);
    let tracker = RecordingTracker::default();

    let report = run_queue(
        &cfg(&repo, "stamp-dry", true),
        &queue,
        &agent,
        &tracker,
        &ScriptedClock::never(),
    )
    .unwrap();

    assert!(
        tracker.closes.borrow().is_empty(),
        "dry run must never close an issue"
    );
    assert!(
        agent.executed.borrow().is_empty(),
        "dry run plans only — never executes"
    );
    // Empty branch dropped, repo restored.
    assert_eq!(current_branch(&repo), "main", "restored to original branch");
    assert!(
        !branch_exists(&repo, &report.branch),
        "empty branch dropped"
    );

    fs::remove_dir_all(&repo).ok();
}

#[test]
fn deadline_blocks_starting_the_next_issue() {
    let repo = init_repo("deadline");
    let queue = vec![issue(1), issue(2)];
    let agent = ScriptedAgent::new(vec![Outcome::Done, Outcome::Done]);
    let tracker = RecordingTracker::default();
    // Clock: first poll (before #1) OK, second poll (before #2) reports passed.
    let clock = ScriptedClock::passes_after(1);

    let report = run_queue(
        &cfg(&repo, "stamp-deadline", false),
        &queue,
        &agent,
        &tracker,
        &clock,
    )
    .unwrap();

    assert_eq!(*agent.planned.borrow(), vec![1], "#2 never planned");
    assert_eq!(*agent.executed.borrow(), vec![1], "#2 never executed");
    assert!(matches!(report.stop, Some(StopReason::Deadline)));

    fs::remove_dir_all(&repo).ok();
}

#[test]
fn report_carries_commit_count_and_oneline() {
    let repo = init_repo("report");
    // Three green issues → three commits over the base.
    let queue = vec![issue(1), issue(2), issue(3)];
    let agent = ScriptedAgent::new(vec![Outcome::Done, Outcome::Done, Outcome::Done]);
    let tracker = RecordingTracker::default();

    let report = run_queue(
        &cfg(&repo, "stamp-report", false),
        &queue,
        &agent,
        &tracker,
        &ScriptedClock::never(),
    )
    .unwrap();

    assert_eq!(report.commits, 3, "one commit per green issue");
    assert_eq!(
        report.oneline.len(),
        report.commits,
        "one oneline entry per counted commit"
    );

    fs::remove_dir_all(&repo).ok();
}
