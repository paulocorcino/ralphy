//! Integration tests for the verify gate's spawn/drain/kill loop (ADR-0011,
//! #156), driven against the bundled `verify_test_child` helper binary (located
//! via `CARGO_BIN_EXE_*`, which Cargo sets for integration tests). A real child —
//! and a real grandchild that inherits its stdout pipe — makes the "leaked
//! background process can't hang the gate" guarantee observable, portably across
//! Windows and Unix.

use std::time::{Duration, Instant};

use ralphy_core::verify::run;

/// The argv the gate runs: the helper child in `mode`. `run` spawns argv directly
/// (no shell), so this is exactly what a plan's `## Verify` line becomes.
fn child_argv(mode: &str) -> Vec<String> {
    vec![
        env!("CARGO_BIN_EXE_verify_test_child").to_string(),
        mode.to_string(),
    ]
}

#[test]
fn leaked_grandchild_holding_stdout_does_not_hang_the_gate() {
    // The load-bearing fix: the foreground command exits 0 in moments, but an
    // orphaned grandchild keeps the inherited stdout pipe open. An unbounded output
    // drain would block forever (the observed ~43 min hang); the pre-collect
    // tree-kill must close the pipe so the gate returns promptly WITH the
    // foreground's output captured.
    let started = Instant::now();
    let report = run(
        &[child_argv("exit-leaking-grandchild")],
        std::env::temp_dir().as_path(),
        Duration::from_secs(120),
    );
    let elapsed = started.elapsed();

    assert!(
        report.passed,
        "the foreground command exited 0 — the gate passes despite the leaked child"
    );
    let cmd = &report.commands[0];
    assert!(
        !cmd.timed_out,
        "the foreground command exited, it did not time out"
    );
    assert_eq!(cmd.exit_code, Some(0), "its exit code is captured");
    // The reader delivers its buffer only at EOF, so capturing the marker proves
    // the tree-kill closed the leaked write-end — before that fix, the collect
    // grace expired and this stream came back EMPTY (the repair brief a failing
    // gate hands the executor lost its whole output tail).
    assert!(
        cmd.output_tail.contains("foreground-marker-before-leak"),
        "the foreground's output is captured despite the leaked pipe holder — got {:?}",
        cmd.output_tail
    );
    // Returns promptly, NOT after the grandchild's 60s lifetime.
    assert!(
        elapsed < Duration::from_secs(30),
        "the gate returned promptly, not after the grandchild's lifetime (took {elapsed:?})"
    );
}

#[test]
fn timeout_kills_the_whole_tree_and_returns_promptly() {
    // The direct child sleeps past the deadline with a grandchild holding the pipe.
    // The timeout path must kill the whole tree so the reader reaches EOF and the
    // gate returns promptly, counting the command as a failure.
    let started = Instant::now();
    let report = run(
        &[child_argv("sleep-with-grandchild")],
        std::env::temp_dir().as_path(),
        Duration::from_millis(500),
    );
    let elapsed = started.elapsed();

    assert!(!report.passed, "a timed-out command fails the gate");
    let cmd = &report.commands[0];
    assert!(cmd.timed_out, "the command outlived the deadline");
    assert!(
        elapsed < Duration::from_secs(30),
        "tree-kill closes the inherited pipe so the drain doesn't hang (took {elapsed:?})"
    );
}
