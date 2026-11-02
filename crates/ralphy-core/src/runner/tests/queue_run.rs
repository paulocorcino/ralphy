use super::*;

#[test]
fn green_close_runs_through_fakes_only() {
    let root = test_dir("green");
    let cfg = test_cfg(&root, "ut-green");
    let repo = FakeRepo::new();
    let sink = FakeLedger::default();
    let agent = MiniAgent::new(vec![Outcome::Done])
        .with_extra(format!("## Verify\n\n{}\n", verify_ok_line()));
    let tracker = FakeTracker::default();

    let report = run_queue_with(
        &cfg,
        &[test_issue(7)],
        &agent,
        &tracker,
        &FakeClock,
        &repo,
        &sink,
    )
    .expect("run succeeds");

    assert_eq!(report.worked.len(), 1);
    assert!(report.worked[0].closed, "green issue is closed");
    assert!(report.stop.is_none());
    assert_eq!(report.commits, 1, "commit count read through the fake");
    assert_eq!(*tracker.closes.borrow(), vec![7]);

    // One plan + one execute ledger line, folded into the run totals.
    let phases: Vec<String> = sink
        .records
        .borrow()
        .iter()
        .map(|r| r.phase.clone())
        .collect();
    assert_eq!(phases, vec!["plan", "execute"]);
    assert_eq!(report.run_usage.total(), 8, "3 plan + 5 execute tokens");
    // Both phases carry the same model: the plan usage straight through,
    // and the execute usage via `Usage::fold_usage` over the resume loop's
    // attempts (#225) — the exec phase no longer drops model to `unknown`.
    assert_eq!(report.run_usage_by_model["fake-model"].total(), 8);
    assert!(!report.run_usage_by_model.contains_key("unknown"));

    // Branch lifecycle: the run branch was cut, and the clean run
    // returned to the original branch.
    let checkouts = repo.checkouts.borrow();
    assert_eq!(checkouts[0], "new:afk/run-ut-green:main");
    assert_eq!(checkouts.last().unwrap(), "force:main");

    remove_scratch(&root);
}

#[test]
fn non_green_outcome_stops_the_run() {
    let root = test_dir("nongreen");
    let cfg = test_cfg(&root, "ut-nongreen");
    let repo = FakeRepo::new();
    let sink = FakeLedger::default();
    let agent = MiniAgent::new(vec![Outcome::Stuck]);
    let tracker = FakeTracker::default();

    let report = run_queue_with(
        &cfg,
        &[test_issue(1), test_issue(2)],
        &agent,
        &tracker,
        &FakeClock,
        &repo,
        &sink,
    )
    .expect("run succeeds");

    assert!(matches!(
        report.stop,
        Some(StopReason::NonGreen {
            number: 1,
            outcome: Outcome::Stuck
        })
    ));
    assert_eq!(report.worked.len(), 1, "issue 2 never started");
    assert_eq!(*agent.planned.borrow(), vec![1], "issue 2 never planned");
    assert!(tracker.closes.borrow().is_empty());

    // The execute ledger line carries the terminal outcome.
    let records = sink.records.borrow();
    let exec = records.iter().find(|r| r.phase == "execute").unwrap();
    assert_eq!(exec.outcome, "stuck");

    // A stopped run leaves the repo on the run branch for inspection.
    assert!(
        !repo.checkouts.borrow().iter().any(|c| c == "force:main"),
        "no return to the original branch on a stop"
    );

    remove_scratch(&root);
}

#[test]
fn verify_gate_failure_leaves_issue_open_and_run_continues() {
    let root = test_dir("verify-fail");
    let cfg = test_cfg(&root, "ut-vfail");
    let repo = FakeRepo::new();
    let sink = FakeLedger::default();
    // Initial execute + two repair attempts, all `Done`; the gate itself
    // keeps failing, so the repair budget is spent and the issue is left
    // open while the run marches on.
    let agent = MiniAgent::new(vec![Outcome::Done, Outcome::Done, Outcome::Done])
        .with_extra(format!("## Verify\n\n{}\n", verify_fail_line()));
    let tracker = FakeTracker::default();

    let report = run_queue_with(
        &cfg,
        &[test_issue(3)],
        &agent,
        &tracker,
        &FakeClock,
        &repo,
        &sink,
    )
    .expect("run succeeds");

    assert!(
        report.stop.is_none(),
        "verify failure does not stop the run"
    );
    assert_eq!(report.worked.len(), 1);
    assert!(!report.worked[0].closed);
    assert!(
        report.worked[0].outcome.is_none(),
        "reported skipped-on-verify"
    );
    assert!(tracker.closes.borrow().is_empty());

    // The repair phase is on the ledger with the failed-gate outcome.
    let records = sink.records.borrow();
    let repair = records.iter().find(|r| r.phase == "repair").unwrap();
    assert_eq!(repair.outcome, "verify-failed");
    assert_eq!(repair.tokens.total(), 10, "two repair executes accumulated");
    // …and the accumulation keeps the model: a repair folded with
    // `add_tokens` dropped it, so the line landed in the `unknown` bucket
    // and its cost never priced (the run figure showed a bare `+?`).
    assert_eq!(
        repair.model, "fake-model",
        "the folded repair line carries the attempts' model, not `unknown`"
    );

    // The honesty artifact was posted on each gate run.
    assert!(tracker
        .comments
        .borrow()
        .iter()
        .any(|(n, b)| *n == 3 && b.contains("## Verify (Ralphy run ut-vfail)")));

    // The run finished cleanly, so the repo returned to the original branch.
    assert_eq!(repo.checkouts.borrow().last().unwrap(), "force:main");

    remove_scratch(&root);
}

#[test]
fn verify_spawn_failure_short_circuits_without_repairs() {
    let root = test_dir("verify-spawn");
    let cfg = test_cfg(&root, "ut-vspawn");
    let repo = FakeRepo::new();
    let sink = FakeLedger::default();
    // A `## Verify` command that can't be spawned (a typo'd binary). The
    // MiniAgent is scripted with a SINGLE Done: if the gate handed the failure
    // back for repair, the repair execute would pop a second scripted outcome
    // and panic — so a clean run proves no repair attempt was ever spent (#182).
    let agent = MiniAgent::new(vec![Outcome::Done])
        .with_extra("## Verify\n\ndefinitely-not-a-real-binary-xyz\n");
    let tracker = FakeTracker::default();

    let report = run_queue_with(
        &cfg,
        &[test_issue(8)],
        &agent,
        &tracker,
        &FakeClock,
        &repo,
        &sink,
    )
    .expect("run succeeds");

    // Skipped, not stopped: the issue is left open, the run marches on, and
    // nothing was closed on a gate that could not run.
    assert!(report.stop.is_none(), "a spawn failure skips, never stops");
    assert_eq!(report.worked.len(), 1);
    assert!(!report.worked[0].closed);
    assert!(tracker.closes.borrow().is_empty());

    // No repair attempt was spent: only plan + the initial execute ran. A
    // repair phase would appear here (and would have panicked the MiniAgent).
    let phases: Vec<String> = sink
        .records
        .borrow()
        .iter()
        .map(|r| r.phase.clone())
        .collect();
    assert_eq!(
        phases,
        vec!["plan", "execute"],
        "no repair phase — the budget was untouched"
    );

    // The honesty artifact names it a spec/spawn problem, not a test failure.
    assert!(tracker.comments.borrow().iter().any(|(n, b)| *n == 8
        && b.contains("## Verify (Ralphy run ut-vspawn)")
        && b.contains("spec/spawn problem")));

    // The run finished cleanly, so the repo returned to the original branch.
    assert_eq!(repo.checkouts.borrow().last().unwrap(), "force:main");

    remove_scratch(&root);
}

#[test]
fn protocol_bounce_repairs_then_closes() {
    let root = test_dir("protocol");
    let cfg = test_cfg(&root, "ut-protocol");
    let repo = FakeRepo::new();
    let sink = FakeLedger::default();
    // First execute claims Done over a protocol-dirty plan; the lint
    // bounces the session back once, the executor repairs, the re-lint
    // passes, and the issue closes (no `## Verify` → warn-and-close).
    let agent = MiniAgent::new(vec![Outcome::Done, Outcome::Done]).lint_dirty_with_fix();
    let tracker = FakeTracker::default();

    let report = run_queue_with(
        &cfg,
        &[test_issue(4)],
        &agent,
        &tracker,
        &FakeClock,
        &repo,
        &sink,
    )
    .expect("run succeeds");

    assert_eq!(report.worked.len(), 1);
    assert!(report.worked[0].closed, "closed after the repaired bounce");
    assert_eq!(*tracker.closes.borrow(), vec![4]);

    // The bounce is its own ledger phase, settled green.
    let phases: Vec<String> = sink
        .records
        .borrow()
        .iter()
        .map(|r| r.phase.clone())
        .collect();
    assert_eq!(phases, vec!["plan", "execute", "protocol-repair"]);
    let records = sink.records.borrow();
    let bounce = records
        .iter()
        .find(|r| r.phase == "protocol-repair")
        .unwrap();
    assert_eq!(bounce.outcome, "done");
    // The bounce's usage is taken whole, model included — summing it into a
    // default `Usage` dropped the model and left the line unpriced.
    assert_eq!(
        bounce.model, "fake-model",
        "the protocol-bounce line carries the bounce's model, not `unknown`"
    );

    remove_scratch(&root);
}
