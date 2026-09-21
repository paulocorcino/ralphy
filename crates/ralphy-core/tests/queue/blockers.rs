//! The blocked-by graph: open/closed blockers, split children, self
//! references and the human gate.

use super::*;

#[test]
fn open_blocker_skips_then_closed_blocker_runs() {
    // Pass 1: #5 declares "## Blocked by\n- #2" and #2 is NOT in the closed set.
    // Expected: #5 skipped (blocked_by == [2]), not closed, no stop; #2 runs normally.
    {
        let repo = init_repo("blocker-open");
        let blocked_body = "## Blocked by\n- #2\n";
        let queue = vec![issue_with_body(5, blocked_body), issue(2)];
        let agent = ScriptedAgent::new(vec![Outcome::Done]); // only #2 executes
                                                             // #2 is NOT in closed_issues → is_closed(2) returns false → #5 is skipped.
        let tracker = RecordingTracker::default();

        let report = run_queue(
            &cfg(&repo, "stamp-blocker-open", false),
            &queue,
            &agent,
            &tracker,
            &ScriptedClock::never(),
        )
        .unwrap();

        // #5 must have been skipped with blocked_by == [2].
        let r5 = report
            .worked
            .iter()
            .find(|r| r.number == 5)
            .expect("#5 in worked");
        assert!(r5.outcome.is_none(), "#5 outcome must be None (skipped)");
        assert!(!r5.closed, "#5 must not be closed");
        assert_eq!(r5.blocked_by, vec![2], "#5 blocked_by must be [2]");

        // #2 must have been planned, executed, and closed.
        assert!(
            agent.planned.borrow().contains(&2),
            "#2 must have been planned"
        );
        assert!(
            agent.executed.borrow().contains(&2),
            "#2 must have been executed"
        );
        let closes: Vec<u64> = tracker.closes.borrow().iter().map(|(n, _)| *n).collect();
        assert_eq!(closes, vec![2], "only #2 closed");

        assert!(report.stop.is_none(), "no stop — later issues continue");

        fs::remove_dir_all(&repo).ok();
    }

    // Pass 2: same queue but #2 IS in the closed set.
    // Expected: #5 is no longer blocked and runs normally (planned, executed, closed).
    {
        let repo = init_repo("blocker-closed");
        let blocked_body = "## Blocked by\n- #2\n";
        let queue = vec![issue_with_body(5, blocked_body), issue(2)];
        let agent = ScriptedAgent::new(vec![Outcome::Done, Outcome::Done]);
        let tracker = RecordingTracker {
            closed_issues: HashSet::from([2]),
            ..Default::default()
        };

        let report = run_queue(
            &cfg(&repo, "stamp-blocker-closed", false),
            &queue,
            &agent,
            &tracker,
            &ScriptedClock::never(),
        )
        .unwrap();

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

        fs::remove_dir_all(&repo).ok();
    }
}

#[test]
fn closed_blocker_with_open_children_still_blocks() {
    // #4 declares "Blocked by #3"; #3 is closed but was a retired bundle whose
    // work moved into open children #16/#17 (their `## Parent` references #3).
    // Expected: #4 is skipped, blocked on the children — not on the closed #3.
    {
        let repo = init_repo("split-blocks");
        let queue = vec![issue_with_body(4, "## Blocked by\n- #3\n")];
        let agent = ScriptedAgent::new(vec![]);
        let tracker = RecordingTracker {
            closed_issues: HashSet::from([3]),
            children: HashMap::from([(3u64, vec![16, 17])]),
            ..Default::default()
        };

        let report = run_queue(
            &cfg(&repo, "stamp-split", false),
            &queue,
            &agent,
            &tracker,
            &ScriptedClock::never(),
        )
        .unwrap();

        let r4 = report
            .worked
            .iter()
            .find(|r| r.number == 4)
            .expect("#4 in worked");
        assert!(r4.outcome.is_none(), "#4 skipped, never planned");
        assert!(!r4.closed);
        assert_eq!(r4.blocked_by, vec![16, 17], "blocked on the open children");
        assert!(
            agent.planned.borrow().is_empty(),
            "#4 never reached the planner"
        );
        assert!(report.stop.is_none(), "a skip never stops the run");

        fs::remove_dir_all(&repo).ok();
    }

    // Same shape but the children are all closed (none open): the closed
    // blocker counts as done and #4 runs normally.
    {
        let repo = init_repo("split-drained");
        let queue = vec![issue_with_body(4, "## Blocked by\n- #3\n")];
        let agent = ScriptedAgent::new(vec![Outcome::Done]);
        let tracker = RecordingTracker {
            closed_issues: HashSet::from([3]),
            children: HashMap::new(), // no OPEN children remain
            ..Default::default()
        };

        let report = run_queue(
            &cfg(&repo, "stamp-drained", false),
            &queue,
            &agent,
            &tracker,
            &ScriptedClock::never(),
        )
        .unwrap();

        let r4 = report
            .worked
            .iter()
            .find(|r| r.number == 4)
            .expect("#4 in worked");
        assert!(r4.blocked_by.is_empty(), "no open children → unblocked");
        assert!(r4.closed, "#4 closed green");

        fs::remove_dir_all(&repo).ok();
    }
}

#[test]
fn split_children_never_block_the_issue_on_itself() {
    // The #299/#300 shape: #4 declares "Blocked by #3", #3 is closed, and #4 is
    // itself among #3's open children (its `## Parent` section named #3 in prose).
    // Substituting the children verbatim would hand #4 back to itself as a blocker
    // — a gate only #4 could clear, so it could never run. Expected: the self-ref
    // is dropped, the remaining child #16 blocks, and with no other child #4 runs.
    {
        let repo = init_repo("split-self-among-children");
        let queue = vec![issue_with_body(4, "## Blocked by\n- #3\n")];
        let agent = ScriptedAgent::new(vec![]);
        let tracker = RecordingTracker {
            closed_issues: HashSet::from([3]),
            children: HashMap::from([(3u64, vec![4, 16])]),
            ..Default::default()
        };

        let report = run_queue(
            &cfg(&repo, "stamp-split-self", false),
            &queue,
            &agent,
            &tracker,
            &ScriptedClock::never(),
        )
        .unwrap();

        let r4 = report
            .worked
            .iter()
            .find(|r| r.number == 4)
            .expect("#4 in worked");
        assert_eq!(
            r4.blocked_by,
            vec![16],
            "self dropped, sibling still blocks"
        );

        fs::remove_dir_all(&repo).ok();
    }

    // #4 is the ONLY open child of the closed #3: nothing else is pending, so the
    // blocker counts as done and #4 runs instead of parking forever on itself.
    {
        let repo = init_repo("split-self-only-child");
        let queue = vec![issue_with_body(4, "## Blocked by\n- #3\n")];
        let agent = ScriptedAgent::new(vec![Outcome::Done]);
        let tracker = RecordingTracker {
            closed_issues: HashSet::from([3]),
            children: HashMap::from([(3u64, vec![4])]),
            ..Default::default()
        };

        let report = run_queue(
            &cfg(&repo, "stamp-split-self-only", false),
            &queue,
            &agent,
            &tracker,
            &ScriptedClock::never(),
        )
        .unwrap();

        let r4 = report
            .worked
            .iter()
            .find(|r| r.number == 4)
            .expect("#4 in worked");
        assert!(r4.blocked_by.is_empty(), "an issue never blocks itself");
        assert!(r4.closed, "#4 closed green");

        fs::remove_dir_all(&repo).ok();
    }
}

#[test]
fn self_ref_in_blocked_by_is_ignored() {
    // A malformed `## Blocked by - #5` on issue #5: the gate must drop it rather
    // than park the issue on a blocker only itself could close.
    let repo = init_repo("self-ref-blocker");
    let queue = vec![issue_with_body(5, "## Blocked by\n- #5\n")];
    let agent = ScriptedAgent::new(vec![Outcome::Done]);
    let tracker = RecordingTracker::default();

    let report = run_queue(
        &cfg(&repo, "stamp-self-ref", false),
        &queue,
        &agent,
        &tracker,
        &ScriptedClock::never(),
    )
    .unwrap();

    let r5 = report
        .worked
        .iter()
        .find(|r| r.number == 5)
        .expect("#5 in worked");
    assert!(r5.blocked_by.is_empty(), "self-ref dropped");
    assert!(r5.closed, "#5 ran and closed green");

    fs::remove_dir_all(&repo).ok();
}

#[test]
fn human_gate_blocker_is_classified_and_run_continues() {
    // #5 is blocked by #2, an OPEN issue carrying `ready-for-human` (a human
    // gate, ADR-0014). #7 is independent and runnable. Expected: #5 is skipped
    // with #2 recorded in BOTH blocked_by and human_blockers; the run does NOT
    // stop — #7 still runs to a green close. Only #5's chain stalls.
    let repo = init_repo("human-gate");
    let queue = vec![issue_with_body(5, "## Blocked by\n- #2\n"), issue(7)];
    let agent = ScriptedAgent::new(vec![Outcome::Done]); // only #7 executes
    let tracker = RecordingTracker {
        // #2 is open (absent from closed_issues) and carries the human gate.
        issue_labels: HashMap::from([(2u64, vec!["ready-for-human".to_string()])]),
        ..Default::default()
    };

    let report = run_queue(
        &cfg(&repo, "stamp-human-gate", false),
        &queue,
        &agent,
        &tracker,
        &ScriptedClock::never(),
    )
    .unwrap();

    let r5 = report
        .worked
        .iter()
        .find(|r| r.number == 5)
        .expect("#5 in worked");
    assert!(r5.outcome.is_none(), "#5 skipped, never planned");
    assert!(!r5.closed, "#5 not closed");
    assert_eq!(r5.blocked_by, vec![2], "#5 still records its open blocker");
    assert_eq!(
        r5.human_blockers,
        vec![2],
        "#2 is classified as a human gate"
    );

    // The run continued: #7 ran and closed green; no stop.
    assert!(agent.executed.borrow().contains(&7), "#7 must have run");
    let closes: Vec<u64> = tracker.closes.borrow().iter().map(|(n, _)| *n).collect();
    assert_eq!(closes, vec![7], "only #7 closed");
    assert!(
        report.stop.is_none(),
        "a human gate never stops the whole run"
    );

    fs::remove_dir_all(&repo).ok();
}

#[test]
fn ordinary_open_blocker_is_not_a_human_gate() {
    // #5 is blocked by open #2 carrying only `ready-for-agent` (ordinary agent
    // work the queue will clear). Expected: skipped, blocked_by == [2], but
    // human_blockers empty — it is NOT a human gate.
    let repo = init_repo("agent-blocker");
    let queue = vec![issue_with_body(5, "## Blocked by\n- #2\n")];
    let agent = ScriptedAgent::new(vec![]);
    let tracker = RecordingTracker {
        issue_labels: HashMap::from([(2u64, vec!["ready-for-agent".to_string()])]),
        ..Default::default()
    };

    let report = run_queue(
        &cfg(&repo, "stamp-agent-blocker", false),
        &queue,
        &agent,
        &tracker,
        &ScriptedClock::never(),
    )
    .unwrap();

    let r5 = report
        .worked
        .iter()
        .find(|r| r.number == 5)
        .expect("#5 in worked");
    assert_eq!(r5.blocked_by, vec![2]);
    assert!(
        r5.human_blockers.is_empty(),
        "an agent-work blocker is not a human gate"
    );

    fs::remove_dir_all(&repo).ok();
}

#[test]
fn blocked_by_in_consolidated_comment_gates_issue() {
    // #5's body has NO `## Blocked by`; its marked consolidated-spec comment does
    // (ADR-0017). The open blocker #9 named there must gate the issue exactly
    // like one in the body.
    let repo = init_repo("blocked-in-comment");
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

    let report = run_queue(
        &cfg(&repo, "stamp-blocked-comment", false),
        &queue,
        &agent,
        &tracker,
        &ScriptedClock::never(),
    )
    .unwrap();

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

    fs::remove_dir_all(&repo).ok();
}
