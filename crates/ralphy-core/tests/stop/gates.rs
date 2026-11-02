//! A stop and the verify gate, the reset wait and a usage limit.

use super::*;

/// The verify gate is the run's OTHER long child — a real suite here is minutes.
/// Without a stop check in its 50 ms poll loop the operator's stop would sit
/// unheard until `verify_timeout` elapsed, which is exactly the wait a Stop
/// button exists to remove.
#[test]
fn a_stop_cuts_a_running_verify_gate() {
    let _flag = StopFlagGuard::acquire();
    let repo = init_repo("verify");
    let cmd = vec![
        env!("CARGO_BIN_EXE_verify_test_child").to_string(),
        "sleep".to_string(),
    ];

    // Pre-set, so the assertion is about the check existing rather than about
    // racing a 60s child with a timer.
    ralphy_core::stop::request();
    let started = std::time::Instant::now();
    let report = ralphy_core::verify::run(&[cmd], &repo, Duration::from_secs(120));
    let elapsed = started.elapsed();

    assert!(
        elapsed < Duration::from_secs(30),
        "the gate must reap on the stop, not wait out its 120s budget (took {elapsed:?})"
    );
    assert!(!report.passed, "a gate that never finished did not pass");
    remove_scratch(&repo);
}

/// `wait_for_reset` is the one wait with no ceiling — a reset days out is
/// honoured on purpose (ADR-0030). Before docs/adr/0054 its own doc said it was
/// "bounded only by the run deadline and Ctrl-C", which made it the single place
/// a run could ignore a stop indefinitely.
///
/// A far-future reset with the flag already up must return AT ONCE: a sleeping
/// loop would take a second per tick, so the elapsed-time assertion is what
/// proves the check sits at the TOP of the loop rather than after the sleep.
#[test]
fn wait_for_reset_returns_at_once_on_a_stop_request() {
    let _flag = StopFlagGuard::acquire();
    let clock = ralphy_core::WallClock { deadline: None };
    let far_future = ResetAt {
        text: "in three days".to_string(),
        at: Some((chrono::Local::now() + chrono::Duration::days(3)).fixed_offset()),
    };

    ralphy_core::stop::request();
    let started = std::time::Instant::now();
    let outcome = clock.wait_for_reset(&far_future);
    let elapsed = started.elapsed();

    assert_eq!(
        outcome,
        WaitOutcome::DeadlinePassed,
        "a stopped wait ends the run rather than resuming it"
    );
    assert!(
        elapsed < Duration::from_millis(900),
        "the check must precede the 1s sleep, not follow it (took {elapsed:?})"
    );
}

/// The inert half. An inverted check would cut every verify gate in the
/// workspace short and still leave the test above green.
#[test]
fn without_a_stop_the_verify_gate_runs_its_command_to_completion() {
    let _flag = StopFlagGuard::acquire();
    let repo = init_repo("verify-clean");
    let cmd = vec![
        env!("CARGO_BIN_EXE_verify_test_child").to_string(),
        "exit-leaking-grandchild".to_string(),
    ];

    let report = ralphy_core::verify::run(&[cmd], &repo, Duration::from_secs(120));
    assert!(
        report.passed,
        "an unstopped gate runs its command normally: {:?}",
        report.commands
    );
    remove_scratch(&repo);
}

/// A stop read after a `Limit` outcome comes BEFORE the resume decision: the
/// child the operator stopped ended with a limit line in its transcript, and
/// resuming would wait for the reset and then run `execute` a second time.
#[test]
fn a_stop_while_the_outcome_is_a_limit_executes_once_and_never_resumes() {
    let _flag = StopFlagGuard::acquire();
    let repo = init_repo("stop-limit");
    let agent = StoppingAgent::new(Some(1)).limiting_first();

    let report = run_queue(
        &cfg(&repo, "20260729-000000"),
        &[issue(1)],
        &agent,
        &SilentTracker,
        &FreeClock,
    )
    .expect("the run completes its unwind");

    assert_eq!(*agent.executed.borrow(), vec![1], "no resume after a stop");
    assert!(
        matches!(report.stop, Some(StopReason::Stopped { number: Some(1) })),
        "expected a stop naming #1, got {:?}",
        report.stop
    );
    remove_scratch(&repo);
}
