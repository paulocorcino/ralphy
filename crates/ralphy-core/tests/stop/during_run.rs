//! A stop raised while the run works an issue.

use ralphy_core::QueueReport;

use super::*;

/// A stop raised while #1 is executing is caught the moment the execute returns
/// — BEFORE the protocol lint and the verify gate, which is where the minutes
/// are. #2 is never planned and never executed, and the stop names the issue
/// that was in flight so the panel can say what happened to it.
#[test]
fn a_stop_during_an_issue_halts_before_the_gates_and_names_that_issue() {
    let _flag = StopFlagGuard::acquire();
    let repo = init_repo("halts");
    let agent = StoppingAgent::new(Some(1));

    let report = run_queue(
        &cfg(&repo, "20260728-000000"),
        &[issue(1), issue(2), issue(3)],
        &agent,
        &SilentTracker,
        &FreeClock,
    )
    .expect("the run completes its unwind");

    assert_eq!(
        *agent.executed.borrow(),
        vec![1],
        "issues past the stop must never be executed"
    );
    assert_eq!(*agent.planned.borrow(), vec![1], "…nor even planned");
    assert!(
        matches!(report.stop, Some(StopReason::Stopped { number: Some(1) })),
        "expected a stop naming #1, got {:?}",
        report.stop
    );
    // Worked but NOT closed: its commits are on the branch, but no gate ever
    // verified them, so the run must not vouch for the issue by closing it.
    assert_eq!(report.worked.len(), 1);
    assert!(!report.worked[0].closed, "a stopped issue stays open");
    remove_scratch(&repo);
}

/// A stop that lands while #1 is PLANNING. The reaped planner wrote no plan, so
/// `plan` returns an `Err` — which, without the stop check, would restore the
/// branch and fail the whole run with a planning error. It must end as the
/// button, name #1, and never start an executor.
#[test]
fn a_stop_that_reaps_the_planner_ends_the_run_as_stopped() {
    let _flag = StopFlagGuard::acquire();
    let repo = init_repo("plan-reaped");
    let agent = StoppingAgent::new(None).stopping_in_plan(1, PlannerEnd::Reaped);

    let report = run_queue(
        &cfg(&repo, "20260728-000004"),
        &[issue(1), issue(2)],
        &agent,
        &SilentTracker,
        &FreeClock,
    )
    .expect("a stopped planner is not a run error");

    assert_eq!(*agent.planned.borrow(), vec![1]);
    assert!(
        agent.executed.borrow().is_empty(),
        "no executor after a stop"
    );
    assert!(
        matches!(report.stop, Some(StopReason::Stopped { number: Some(1) })),
        "expected a stop naming #1, got {:?}",
        report.stop
    );
    assert_eq!(report.worked.len(), 1);
    assert!(!report.worked[0].closed, "a stopped issue stays open");
    assert_eq!(
        current_branch(&repo),
        report.branch,
        "a stopped run hands its branch back, as every other stop does"
    );
    remove_scratch(&repo);
}

/// Runs #1 and #2 with a planner that the stop reaps after it spent tokens:
/// `plan` returns a `PlanStopped` carrying that usage.
fn run_with_a_stopped_planner(name: &str, stamp: &str) -> (StoppingAgent, QueueReport, PathBuf) {
    let repo = init_repo(name);
    let agent = StoppingAgent::new(None).stopping_in_plan(1, PlannerEnd::ReapedWithUsage);
    let report = run_queue(
        &cfg(&repo, stamp),
        &[issue(1), issue(2)],
        &agent,
        &SilentTracker,
        &FreeClock,
    )
    .expect("a stopped planner is not a run error");
    (agent, report, repo)
}

/// The vendor billed the tokens of a reaped planner: the run's usage counts them.
#[test]
fn a_stopped_planner_with_usage_counts_in_the_run_usage() {
    let _flag = StopFlagGuard::acquire();
    let (_agent, report, repo) =
        run_with_a_stopped_planner("plan-stopped-usage", "20260728-000006");

    assert_eq!(report.run_usage.input, STOPPED_PLAN_INPUT);
    remove_scratch(&repo);
}

/// The reaped planner's spend is under its own model in the per-model split.
#[test]
fn a_stopped_planners_model_is_in_the_run_split() {
    let _flag = StopFlagGuard::acquire();
    let (_agent, report, repo) =
        run_with_a_stopped_planner("plan-stopped-model", "20260728-000007");

    let split = report.run_usage_by_model.get(STOPPED_PLAN_MODEL);
    assert_eq!(
        split.map(|u| u.input),
        Some(STOPPED_PLAN_INPUT),
        "split: {:?}",
        report.run_usage_by_model
    );
    remove_scratch(&repo);
}

/// The ledger keeps the reaped planner's spend as a `plan` row with outcome
/// `stopped`, which `ralphy usage` and the spend panel read.
#[test]
fn a_stopped_planner_writes_a_stopped_plan_row() {
    let _flag = StopFlagGuard::acquire();
    let (_agent, _report, repo) =
        run_with_a_stopped_planner("plan-stopped-ledger", "20260728-000009");

    let rows = ralphy_core::read_project_rows(&ralphy_core::git::project_slug(&repo));
    let plan: Vec<_> = rows.iter().filter(|r| r.phase == "plan").collect();
    assert_eq!(plan.len(), 1, "one plan row: {rows:?}");
    assert_eq!(plan[0].outcome, "stopped");
    assert_eq!(plan[0].issue, 1);
    assert_eq!(plan[0].tokens.input, STOPPED_PLAN_INPUT);
    assert_eq!(plan[0].session_id.as_deref(), Some("stopped-session"));
    remove_scratch(&repo);
}

/// Usage on the stop does not turn it into a planning failure: the run still
/// ends as the stop, names #1, and starts no executor.
#[test]
fn a_stopped_planner_with_usage_ends_the_run_as_stopped() {
    let _flag = StopFlagGuard::acquire();
    let (agent, report, repo) = run_with_a_stopped_planner("plan-stopped-ends", "20260728-000008");

    assert_eq!(*agent.planned.borrow(), vec![1]);
    assert!(
        agent.executed.borrow().is_empty(),
        "no executor after a stop"
    );
    assert!(
        matches!(report.stop, Some(StopReason::Stopped { number: Some(1) })),
        "expected a stop naming #1, got {:?}",
        report.stop
    );
    remove_scratch(&repo);
}

/// The planner finished its plan just as the stop landed. The plan is kept on
/// disk, but no executor starts: an executor spawned only to be reaped half a
/// second later is the wait the Stop button exists to remove.
#[test]
fn a_stop_during_planning_starts_no_executor() {
    let _flag = StopFlagGuard::acquire();
    let repo = init_repo("plan-finished");
    let agent = StoppingAgent::new(None).stopping_in_plan(1, PlannerEnd::Finished);

    let report = run_queue(
        &cfg(&repo, "20260728-000005"),
        &[issue(1), issue(2)],
        &agent,
        &SilentTracker,
        &FreeClock,
    )
    .expect("the run completes its unwind");

    assert!(
        agent.executed.borrow().is_empty(),
        "no executor after a stop"
    );
    assert!(
        matches!(report.stop, Some(StopReason::Stopped { number: Some(1) })),
        "expected a stop naming #1, got {:?}",
        report.stop
    );
    remove_scratch(&repo);
}

/// The other gate: a stop already standing when the loop begins is seen at the
/// TOP of the iteration, so nothing is planned at all and no issue is named.
#[test]
fn a_stop_standing_before_the_first_issue_names_no_issue() {
    let _flag = StopFlagGuard::acquire();
    let repo = init_repo("between");
    let agent = StoppingAgent::new(None);
    ralphy_core::stop::request();

    let report = run_queue(
        &cfg(&repo, "20260728-000003"),
        &[issue(1), issue(2)],
        &agent,
        &SilentTracker,
        &FreeClock,
    )
    .expect("the run completes its unwind");

    assert!(
        agent.planned.borrow().is_empty(),
        "the top-of-iteration gate runs before `plan`"
    );
    assert!(
        matches!(report.stop, Some(StopReason::Stopped { number: None })),
        "expected a between-issues stop, got {:?}",
        report.stop
    );
    assert!(report.worked.is_empty());
    remove_scratch(&repo);
}

/// The negative control. Without it, a gate accidentally inverted into
/// `if !stop::requested()` would stop every run at its first issue and the test
/// above would still be green.
#[test]
fn no_stop_request_works_the_whole_queue() {
    let _flag = StopFlagGuard::acquire();
    let repo = init_repo("nostop");
    let agent = StoppingAgent::new(None);

    let report = run_queue(
        &cfg(&repo, "20260728-000001"),
        &[issue(1), issue(2)],
        &agent,
        &SilentTracker,
        &FreeClock,
    )
    .expect("the run completes");

    assert_eq!(*agent.executed.borrow(), vec![1, 2]);
    assert!(
        report.stop.is_none(),
        "an unstopped queue must report no stop reason, got {:?}",
        report.stop
    );
    remove_scratch(&repo);
}

/// **The load-bearing test of docs/adr/0054.**
///
/// The runner's teardown hands the branch back for every `Some(_)` stop and
/// force-checks-out the ORIGINAL branch when `stop` is `None` — and
/// `git checkout -f` destroys uncommitted tracked changes silently. So a stop
/// modelled as anything other than a `StopReason` (an early return, a bool, a
/// "queue exhausted") would delete the work the run had in flight.
///
/// This asserts the consequence rather than the implementation: the repo is
/// still on the run branch, and an uncommitted file written during the stopped
/// issue is still there. Rewrite the stop as a non-`StopReason` and this reds.
#[test]
fn a_stop_leaves_the_run_branch_checked_out_with_uncommitted_work_intact() {
    let _flag = StopFlagGuard::acquire();
    let repo = init_repo("keeps");
    let agent = StoppingAgent::new(Some(1)).littering("scratch.txt");

    let report = run_queue(
        &cfg(&repo, "20260728-000002"),
        &[issue(1), issue(2)],
        &agent,
        &SilentTracker,
        &FreeClock,
    )
    .expect("the run completes its unwind");

    assert_eq!(
        current_branch(&repo),
        report.branch,
        "a stopped run stays on its own branch — it does not check `main` back out"
    );
    assert_ne!(
        report.branch, report.orig_branch,
        "sanity: they differ here"
    );
    let scratch = repo.join("scratch.txt");
    assert!(
        scratch.exists(),
        "the uncommitted file written during the stopped issue was destroyed"
    );
    assert_eq!(fs::read_to_string(&scratch).unwrap(), "uncommitted\n");
    remove_scratch(&repo);
}
