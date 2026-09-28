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
fn forced_issues_ignore_stop_before() {
    // `--only-issue 7` and `--issues 1,2` name their issues, so a named issue's
    // `stop-before` never halts the run: it works the whole list in order.
    // (case, queue, forced list)
    let rows: [(&str, Vec<Issue>, Vec<u64>); 2] = [
        (
            "one forced issue",
            vec![issue_labeled(7, &["stop-before"])],
            vec![7],
        ),
        (
            "a forced list",
            vec![issue(1), issue_labeled(2, &["stop-before"])],
            vec![1, 2],
        ),
    ];
    for (i, (case, queue, forced)) in rows.into_iter().enumerate() {
        let repo = init_repo(&format!("forced-stop-before-{i}"));
        let agent = ScriptedAgent::new(vec![Outcome::Done; forced.len()]);
        let tracker = RecordingTracker::default();

        let report = run_queue(
            &cfg_forced(&repo, &format!("stamp-forced-{i}"), forced.clone()),
            &queue,
            &agent,
            &tracker,
            &ScriptedClock::never(),
        )
        .unwrap();

        assert_eq!(
            *agent.executed.borrow(),
            forced,
            "{case}: every named issue runs, in order, despite stop-before"
        );
        assert!(
            report.stop.is_none(),
            "{case}: a named issue's stop-before never halts a forced run"
        );

        fs::remove_dir_all(&repo).ok();
    }
}

#[test]
fn human_return_labels_skip_their_issues_and_the_queue_continues() {
    // Each of #1..#3 carries a queue label PLUS a human-return label; #4 is a
    // plain queue issue. #1..#3 are skipped (not planned, not executed, not
    // closed) and the queue continues to #4.
    // - `needs-info`: a default human-return label.
    // - `ready-for-human`: the ADR-0015 re-park bug (ADR-0016 amendment). A
    //   verify-gate park leaves this label while the queue label stays, and the
    //   next run must NOT re-queue the issue.
    // - `waiting-reporter`: the core honours whatever resolved set the CLI
    //   passes, so a repo that maps its own label still parks the issue.
    let repo = init_repo("human-return-skip");
    let cases = [
        (1, "default label needs-info"),
        (2, "re-parked ready-for-human"),
        (3, "custom-mapped waiting-reporter"),
    ];
    let queue = vec![
        issue_labeled(1, &["AFK", "needs-info"]),
        issue_labeled(2, &["AFK", "ready-for-human"]),
        issue_labeled(3, &["AFK", "waiting-reporter"]),
        issue(4),
    ];
    let agent = ScriptedAgent::new(vec![Outcome::Done]);
    let tracker = RecordingTracker::default();

    let mut config = cfg(&repo, "stamp-hr-skip", false);
    config.human_return_labels.push("waiting-reporter".into());

    let report = run_queue(&config, &queue, &agent, &tracker, &ScriptedClock::never()).unwrap();

    assert_eq!(*agent.planned.borrow(), vec![4], "only #4 is planned");
    assert_eq!(*agent.executed.borrow(), vec![4], "only #4 is executed");
    assert_eq!(report.worked.len(), 4, "every issue produces a result row");
    for (n, case) in cases {
        let skipped = report.worked.iter().find(|r| r.number == n).unwrap();
        assert!(
            skipped.outcome.is_none(),
            "{case}: #{n} skipped, no outcome"
        );
        assert!(!skipped.closed, "{case}: #{n} stays open");
        assert!(
            !tracker.closes.borrow().iter().any(|(c, _)| *c == n),
            "{case}: #{n} never closed on the tracker"
        );
    }
    assert!(report.stop.is_none(), "the run continues past the skips");

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
