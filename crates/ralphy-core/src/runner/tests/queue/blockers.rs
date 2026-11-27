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

struct BlockerRow {
    name: &'static str,
    queue: Vec<Issue>,
    outcomes: Vec<Outcome>,
    tracker: RecordingTracker,
    blocked_by: Vec<u64>,
    human_blockers: Vec<u64>,
    status: Option<ResultStatus>,
    executed: Vec<u64>,
    closed: Vec<u64>,
}

#[test]
fn an_open_blocker_skips_the_issue_whatever_names_it_table() {
    // #5 is skipped without a plan in both rows; only where the blocker is
    // named, and whether it is a human gate, differ.
    let marker = "<!-- ralphy:consolidated-spec -->";
    let rows = [
        BlockerRow {
            name: "human gate label: #2 is also a human blocker",
            queue: vec![
                issue_with_body(
                    5,
                    "## Blocked by
- #2
",
                ),
                test_issue(7),
            ],
            outcomes: vec![Outcome::Done],
            tracker: RecordingTracker {
                issue_labels: HashMap::from([(2u64, vec!["ready-for-human".to_owned()])]),
                ..Default::default()
            },
            blocked_by: vec![2],
            human_blockers: vec![2],
            status: Some(ResultStatus::Hitl),
            executed: vec![7],
            closed: vec![7],
        },
        BlockerRow {
            name: "consolidated comment: the blocker is read from the comment",
            queue: vec![issue_with_body(
                5,
                "Just a prose spec, no blocked-by section.",
            )],
            outcomes: vec![],
            tracker: RecordingTracker {
                comment_threads: HashMap::from([(
                    5u64,
                    vec![format!(
                        "{marker}
## Consolidated spec

## Blocked by
- #9
"
                    )],
                )]),
                ..Default::default()
            },
            blocked_by: vec![9],
            human_blockers: vec![],
            status: None,
            executed: vec![],
            closed: vec![],
        },
    ];
    for row in rows {
        let root = test_dir("q-blocker-table");
        let agent = ScriptedAgent::new(row.outcomes);

        let (report, _) = run_on_fake(
            &test_cfg(&root, "stamp-blocker-table"),
            &row.queue,
            &agent,
            &row.tracker,
        );

        let name = row.name;
        let r5 = report
            .worked
            .iter()
            .find(|r| r.number == 5)
            .unwrap_or_else(|| panic!("{name}: #5 in worked"));
        assert!(r5.outcome.is_none(), "{name}: #5 skipped, never planned");
        if let Some(status) = row.status {
            assert_eq!(r5.status, status, "{name}: status");
        }
        assert!(!r5.closed, "{name}: #5 not closed");
        assert_eq!(r5.blocked_by, row.blocked_by, "{name}: blocked_by");
        assert_eq!(
            r5.human_blockers, row.human_blockers,
            "{name}: human_blockers"
        );
        assert!(
            !agent.planned.borrow().contains(&5),
            "{name}: #5 never planned"
        );
        assert_eq!(*agent.executed.borrow(), row.executed, "{name}: executed");
        assert_eq!(closed(&row.tracker), row.closed, "{name}: closed");
        assert!(
            report.stop.is_none(),
            "{name}: a blocked issue never stops the run"
        );

        remove_scratch(&root);
    }
}
