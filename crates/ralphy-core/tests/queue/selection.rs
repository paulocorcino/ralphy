//! Which issues a run takes: stop-before, `--only`/forced lists, view/run
//! parity, and the human-return labels (ADR-0016).

use super::*;

#[test]
fn stop_before_halts_before_labeled_issue() {
    let repo = init_repo("stop-before");
    // #1 is a normal issue; #2 carries the stop-before label; #3 must never be touched.
    let queue = vec![issue(1), issue_labeled(2, &["stop-before"]), issue(3)];
    let agent = ScriptedAgent::new(vec![Outcome::Done]);
    let tracker = RecordingTracker::default();

    let report = run_queue(
        &cfg(&repo, "stamp-stopbefore", false),
        &queue,
        &agent,
        &tracker,
        &ScriptedClock::never(),
    )
    .unwrap();

    // #1 executed; #2 (labeled) and #3 never planned/executed.
    assert_eq!(
        *agent.executed.borrow(),
        vec![1],
        "#2 and #3 never executed"
    );
    assert_eq!(*agent.planned.borrow(), vec![1], "#2 and #3 never planned");

    match report.stop {
        Some(StopReason::StopBefore { number }) => assert_eq!(number, 2),
        other => panic!("expected StopBefore, got {other:?}"),
    }

    // Branch has work (from #1), so it is handed back on the run branch.
    assert_eq!(current_branch(&repo), report.branch);

    fs::remove_dir_all(&repo).ok();
}

#[test]
fn view_and_run_agree_issue_for_issue() {
    // ADR-0020 criterion #7: the read-only queue view (`resolve_queue_view`) and a
    // real `run_queue` over the SAME fixture + tracker must classify every issue
    // identically — stop-before, blocked, human-return, eligible — so the CLI
    // listing can never disagree with what the run does. FAILS if either the view
    // or the runner drifts from the shared precedence.
    let repo = init_repo("view-run-agree");
    let queue = vec![
        issue_with_body(5, "## Blocked by\n- #2\n"), // #2 open → Blocked
        issue_labeled(3, &["needs-info"]),           // human-return → Skipped
        issue(7),                                    // clean → Eligible
        issue_labeled(9, &["stop-before"]),          // first stop-before → halts
    ];
    let agent = ScriptedAgent::new(vec![Outcome::Done]); // only #7 executes
    let tracker = RecordingTracker::default(); // #2 open; no children/labels

    // Resolve the view over the SAME fixture + tracker the run consumes.
    let view = resolve_queue_view(&queue, &[], &default_human_return(), &tracker).unwrap();

    let report = run_queue(
        &cfg(&repo, "stamp-view-run-agree", false),
        &queue,
        &agent,
        &tracker,
        &ScriptedClock::never(),
    )
    .unwrap();

    let vs = |n: u64| view.issues.iter().find(|i| i.number == n).unwrap();
    let worked = |n: u64| report.worked.iter().find(|r| r.number == n);

    // Stop-before: the view marks #9 and the run halts there, producing no row.
    assert_eq!(vs(9).queue_status, QueueStatus::StopBefore);
    assert_eq!(view.stop_before, Some(9));
    match &report.stop {
        Some(StopReason::StopBefore { number }) => assert_eq!(*number, 9),
        other => panic!("expected StopBefore, got {other:?}"),
    }
    assert!(worked(9).is_none(), "the stop-before issue is never worked");

    // Blocked: the view's blocked_by equals the run's IssueResult.blocked_by.
    assert_eq!(vs(5).queue_status, QueueStatus::Blocked);
    let r5 = worked(5).expect("#5 produces a skip row");
    assert!(r5.outcome.is_none() && !r5.closed);
    assert_eq!(vs(5).blocked_by, r5.blocked_by);
    assert_eq!(vs(5).blocked_by, vec![2]);

    // Human-return: view Skipped ⇔ the run skips it (no outcome, empty blocked_by).
    assert_eq!(vs(3).queue_status, QueueStatus::Skipped);
    let r3 = worked(3).expect("#3 produces a skip row");
    assert!(r3.outcome.is_none() && !r3.closed && r3.blocked_by.is_empty());
    assert_eq!(vs(3).skip_reason.as_deref(), Some("needs-info"));

    // Eligible: view Eligible ⇔ the run actually worked it (an outcome present).
    assert_eq!(vs(7).queue_status, QueueStatus::Eligible);
    assert_eq!(vs(7).position, Some(1));
    let r7 = worked(7).expect("#7 is worked");
    assert!(r7.outcome.is_some(), "eligible issue was executed");
    assert!(agent.executed.borrow().contains(&7));

    fs::remove_dir_all(&repo).ok();
}

#[test]
fn view_and_run_agree_under_assignee_filter() {
    // ADR-0021 criterion #8: the `--assignee` filter is FETCH-ONLY — it narrows
    // which issues `list_queue` returns, and never touches judgment. So the runner
    // and the view both see only the surviving (filtered) subset, and blocked-by
    // must STILL consult the tracker (`is_closed`), so an issue blocked by an OPEN
    // issue OUTSIDE the filtered subset stays `Blocked`. This models the filtered
    // queue as the already-narrowed subset and asserts view/run parity over it.
    let repo = init_repo("view-run-agree-assignee");
    // The filtered subset the runner receives (say `@me` is assigned #7 and #9).
    // #4 is a colleague's OPEN issue — outside the subset, but #7 is blocked by it.
    let queue = vec![
        issue_with_body(7, "## Blocked by\n- #4\n"), // #4 open & out-of-subset → Blocked
        issue(9),                                    // clean → Eligible
    ];
    let agent = ScriptedAgent::new(vec![Outcome::Done]); // only #9 executes
    let tracker = RecordingTracker::default(); // #4 absent from closed_issues → open

    let view = resolve_queue_view(&queue, &[], &default_human_return(), &tracker).unwrap();

    let report = run_queue(
        &cfg(&repo, "stamp-view-run-assignee", false),
        &queue,
        &agent,
        &tracker,
        &ScriptedClock::never(),
    )
    .unwrap();

    let vs = |n: u64| view.issues.iter().find(|i| i.number == n).unwrap();
    let worked = |n: u64| report.worked.iter().find(|r| r.number == n);

    // Blocked-by still consults the tracker even though #4 is not in the subset:
    // the view marks #7 Blocked and the run records the same blocker.
    assert_eq!(vs(7).queue_status, QueueStatus::Blocked);
    let r7 = worked(7).expect("#7 produces a skip row");
    assert!(r7.outcome.is_none() && !r7.closed);
    assert_eq!(vs(7).blocked_by, r7.blocked_by);
    assert_eq!(
        vs(7).blocked_by,
        vec![4],
        "blocked by the out-of-subset open #4"
    );

    // Eligible: view Eligible ⇔ the run actually worked #9.
    assert_eq!(vs(9).queue_status, QueueStatus::Eligible);
    let r9 = worked(9).expect("#9 is worked");
    assert!(r9.outcome.is_some(), "eligible issue was executed");
    assert!(agent.executed.borrow().contains(&9));
    assert!(!agent.executed.borrow().contains(&7), "#7 stayed blocked");

    fs::remove_dir_all(&repo).ok();
}

#[test]
fn only_issue_ignores_stop_before() {
    let repo = init_repo("only-stop-before");
    // The queue is just the labeled issue; only_issue overrides the stop-before guard.
    let queue = vec![issue_labeled(7, &["stop-before"])];
    let agent = ScriptedAgent::new(vec![Outcome::Done]);
    let tracker = RecordingTracker::default();

    let report = run_queue(
        &cfg_only(&repo, "stamp-only", 7),
        &queue,
        &agent,
        &tracker,
        &ScriptedClock::never(),
    )
    .unwrap();

    // The issue was executed despite the label.
    assert_eq!(*agent.executed.borrow(), vec![7]);
    assert!(
        report.stop.is_none(),
        "no stop when only_issue overrides stop-before"
    );

    fs::remove_dir_all(&repo).ok();
}

#[test]
fn forced_issues_list_ignores_stop_before_across_the_list() {
    let repo = init_repo("forced-list-stop-before");
    // `--issues 1,2`: an explicit, ordered list. #2 carries `stop-before`, but both
    // are named, so the run works the whole list in order without halting — the
    // generalization of `--only-issue` to a set.
    let queue = vec![issue(1), issue_labeled(2, &["stop-before"])];
    let agent = ScriptedAgent::new(vec![Outcome::Done, Outcome::Done]);
    let tracker = RecordingTracker::default();

    let report = run_queue(
        &cfg_forced(&repo, "stamp-forced-list", vec![1, 2]),
        &queue,
        &agent,
        &tracker,
        &ScriptedClock::never(),
    )
    .unwrap();

    assert_eq!(
        *agent.executed.borrow(),
        vec![1, 2],
        "both listed issues run, in order, despite stop-before on #2"
    );
    assert!(
        report.stop.is_none(),
        "a listed issue's stop-before never halts a forced run"
    );

    fs::remove_dir_all(&repo).ok();
}

#[test]
fn human_return_label_skips_issue_and_continues() {
    let repo = init_repo("human-return-skip");
    // #1 carries a queue label PLUS a human-return label; #2 is a plain queue
    // issue. #1 must be skipped (not planned, not executed, not closed) and the
    // queue must continue to #2.
    let queue = vec![issue_labeled(1, &["AFK", "needs-info"]), issue(2)];
    let agent = ScriptedAgent::new(vec![Outcome::Done]);
    let tracker = RecordingTracker::default();

    let report = run_queue(
        &cfg(&repo, "stamp-hr-skip", false),
        &queue,
        &agent,
        &tracker,
        &ScriptedClock::never(),
    )
    .unwrap();

    assert_eq!(*agent.planned.borrow(), vec![2], "#1 never planned");
    assert_eq!(*agent.executed.borrow(), vec![2], "#1 never executed");
    // #1 recorded as a skip (outcome None, not closed); #2 worked. Run continues.
    assert_eq!(report.worked.len(), 2, "both issues produce a result row");
    let skipped = report.worked.iter().find(|r| r.number == 1).unwrap();
    assert!(skipped.outcome.is_none(), "#1 skipped, no outcome");
    assert!(!skipped.closed, "#1 not closed");
    assert!(
        !tracker.closes.borrow().iter().any(|(n, _)| *n == 1),
        "#1 never closed on the tracker"
    );
    assert!(report.stop.is_none(), "the run continues past the skip");

    fs::remove_dir_all(&repo).ok();
}

#[test]
fn reparked_issue_is_not_reworked_on_next_run() {
    // Regression for the ADR-0015 re-park bug (ADR-0016 amendment): a verify-gate
    // park leaves the issue labeled `ready-for-human` while its queue label stays.
    // On the next run that exact label state must NOT re-queue the issue.
    let repo = init_repo("reparked");
    let queue = vec![issue_labeled(1, &["AFK", "ready-for-human"])];
    let agent = ScriptedAgent::new(vec![Outcome::Done]);
    let tracker = RecordingTracker::default();

    let report = run_queue(
        &cfg(&repo, "stamp-reparked", false),
        &queue,
        &agent,
        &tracker,
        &ScriptedClock::never(),
    )
    .unwrap();

    assert!(
        agent.planned.borrow().is_empty(),
        "parked issue not planned"
    );
    assert!(
        agent.executed.borrow().is_empty(),
        "parked issue not executed"
    );
    assert!(
        tracker.closes.borrow().is_empty(),
        "parked issue not closed"
    );
    let row = report.worked.iter().find(|r| r.number == 1).unwrap();
    assert!(!row.closed, "parked issue stays open");

    fs::remove_dir_all(&repo).ok();
}

#[test]
fn only_issue_does_not_override_human_return() {
    // Unlike stop-before, `--only-issue` must NOT run a human-return-labelled
    // issue: the label may record someone else's state (ADR-0016).
    let repo = init_repo("only-human-return");
    let queue = vec![issue_labeled(1, &["AFK", "HITL"])];
    let agent = ScriptedAgent::new(vec![Outcome::Done]);
    let tracker = RecordingTracker::default();

    let report = run_queue(
        &cfg_only(&repo, "stamp-only-hr", 1),
        &queue,
        &agent,
        &tracker,
        &ScriptedClock::never(),
    )
    .unwrap();

    assert!(
        agent.planned.borrow().is_empty(),
        "only_issue does not override the human-return skip"
    );
    assert!(agent.executed.borrow().is_empty());
    assert!(tracker.closes.borrow().is_empty());
    assert!(report.stop.is_none(), "a skip continues, it does not stop");

    fs::remove_dir_all(&repo).ok();
}

#[test]
fn custom_mapped_human_return_label_skips() {
    // The core honours whatever resolved set the CLI passes: a repo that renames
    // `needs-info` to `waiting-reporter` still parks the issue.
    let repo = init_repo("custom-hr");
    let queue = vec![issue_labeled(1, &["AFK", "waiting-reporter"]), issue(2)];
    let agent = ScriptedAgent::new(vec![Outcome::Done]);
    let tracker = RecordingTracker::default();

    let mut config = cfg(&repo, "stamp-custom-hr", false);
    config.human_return_labels = vec!["waiting-reporter".into()];

    let report = run_queue(&config, &queue, &agent, &tracker, &ScriptedClock::never()).unwrap();

    assert_eq!(*agent.executed.borrow(), vec![2], "#1 skipped, #2 worked");
    assert!(report.stop.is_none());

    fs::remove_dir_all(&repo).ok();
}
