//! The blocked-by gate end to end: an open blocker skips, a closed one runs,
//! the human gate, and a blocker named in the consolidated comment.

use std::collections::{HashMap, HashSet};

use super::*;

#[test]
fn open_blocker_skips_then_closed_blocker_runs() {
    // Pass 1: #5 declares "## Blocked by\n- #2" and #2 is NOT in the closed set.
    // Expected: #5 skipped (blocked_by == [2]), not closed, no stop; #2 runs normally.
    {
        let root = test_dir("q-blocker-open");
        let blocked_body = "## Blocked by\n- #2\n";
        let queue = vec![issue_with_body(5, blocked_body), test_issue(2)];
        // Only #2 executes: it is NOT in closed_issues, so is_closed(2) is false and #5 is skipped.
        let agent = ScriptedAgent::new(vec![Outcome::Done]);
        let tracker = RecordingTracker::default();

        let (report, _) = run_on_fake(
            &test_cfg(&root, "stamp-blocker-open"),
            &queue,
            &agent,
            &tracker,
        );

        // #5 must have been skipped with blocked_by == [2].
        let r5 = report
            .worked
            .iter()
            .find(|r| r.number == 5)
            .expect("#5 in worked");
        assert!(r5.outcome.is_none(), "#5 outcome must be None (skipped)");
        assert!(!r5.closed, "#5 must not be closed");
        assert_eq!(r5.blocked_by, vec![2], "#5 blocked_by must be [2]");
        assert_eq!(r5.skip, Some(SkipReason::BlockedBy));

        // #2 must have been planned, executed, and closed.
        assert!(
            agent.planned.borrow().contains(&2),
            "#2 must have been planned"
        );
        assert!(
            agent.executed.borrow().contains(&2),
            "#2 must have been executed"
        );
        assert_eq!(closed(&tracker), vec![2], "only #2 closed");

        assert!(report.stop.is_none(), "no stop — later issues continue");

        remove_scratch(&root);
    }

    // Pass 2: same queue but #2 IS in the closed set.
    // Expected: #5 is no longer blocked and runs normally (planned, executed, closed).
    {
        let root = test_dir("q-blocker-closed");
        let blocked_body = "## Blocked by\n- #2\n";
        let queue = vec![issue_with_body(5, blocked_body), test_issue(2)];
        let agent = ScriptedAgent::new(vec![Outcome::Done, Outcome::Done]);
        let tracker = RecordingTracker {
            closed_issues: HashSet::from([2]),
            ..Default::default()
        };

        let (report, _) = run_on_fake(
            &test_cfg(&root, "stamp-blocker-closed"),
            &queue,
            &agent,
            &tracker,
        );

        // #5 must have run (not blocked).
        let r5 = report
            .worked
            .iter()
            .find(|r| r.number == 5)
            .expect("#5 in worked");
        assert!(
            r5.blocked_by.is_empty(),
            "#5 must not be blocked when #2 is closed"
        );
        assert!(r5.closed, "#5 must be closed green");

        assert!(report.stop.is_none());

        remove_scratch(&root);
    }
}

#[test]
fn human_gate_blocker_is_classified_and_run_continues() {
    // #5 is blocked by #2, an OPEN issue carrying `ready-for-human` (a human
    // gate, ADR-0014). #7 is independent and runnable. Expected: #5 is skipped
    // with #2 recorded in BOTH blocked_by and human_blockers; the run does NOT
    // stop — #7 still runs to a green close. Only #5's chain stalls.
    let root = test_dir("q-human-gate");
    let queue = vec![issue_with_body(5, "## Blocked by\n- #2\n"), test_issue(7)];
    let agent = ScriptedAgent::new(vec![Outcome::Done]); // only #7 executes
    let tracker = RecordingTracker {
        // #2 is open (absent from closed_issues) and carries the human gate.
        issue_labels: HashMap::from([(2u64, vec!["ready-for-human".to_owned()])]),
        ..Default::default()
    };

    let (report, _) = run_on_fake(
        &test_cfg(&root, "stamp-human-gate"),
        &queue,
        &agent,
        &tracker,
    );

    let r5 = report
        .worked
        .iter()
        .find(|r| r.number == 5)
        .expect("#5 in worked");
    assert!(r5.outcome.is_none(), "#5 skipped, never planned");
    assert_eq!(
        r5.status,
        ResultStatus::Hitl,
        "a human-gate blocker parks the issue as HITL"
    );
    assert!(!r5.closed, "#5 not closed");
    assert_eq!(r5.blocked_by, vec![2], "#5 still records its open blocker");
    assert_eq!(
        r5.human_blockers,
        vec![2],
        "#2 is classified as a human gate"
    );

    // The run continued: #7 ran and closed green; no stop.
    assert!(agent.executed.borrow().contains(&7), "#7 must have run");
    assert_eq!(closed(&tracker), vec![7], "only #7 closed");
    assert!(
        report.stop.is_none(),
        "a human gate never stops the whole run"
    );

    remove_scratch(&root);
}

#[test]
fn blocked_by_in_consolidated_comment_gates_issue() {
    // #5's body has NO `## Blocked by`; its marked consolidated-spec comment does
    // (ADR-0017). The open blocker #9 named there must gate the issue exactly
    // like one in the body.
    let root = test_dir("q-blocked-in-comment");
    let queue = vec![issue_with_body(
        5,
        "Just a prose spec, no blocked-by section.",
    )];
    let agent = ScriptedAgent::new(vec![]); // #5 never planned
    let marker = "<!-- ralphy:consolidated-spec -->";
    let tracker = RecordingTracker {
        comment_threads: HashMap::from([(
            5u64,
            vec![format!(
                "{marker}\n## Consolidated spec\n\n## Blocked by\n- #9\n"
            )],
        )]),
        // #9 is open (absent from closed_issues).
        ..Default::default()
    };

    let (report, _) = run_on_fake(
        &test_cfg(&root, "stamp-blocked-comment"),
        &queue,
        &agent,
        &tracker,
    );

    let r5 = report
        .worked
        .iter()
        .find(|r| r.number == 5)
        .expect("#5 in worked");
    assert!(r5.outcome.is_none(), "#5 skipped, never planned");
    assert_eq!(
        r5.blocked_by,
        vec![9],
        "the marked comment's blocker gates the queue"
    );
    assert!(agent.planned.borrow().is_empty(), "#5 never planned");

    remove_scratch(&root);
}
