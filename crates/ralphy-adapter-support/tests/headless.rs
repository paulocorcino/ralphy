//! Direct unit tests for `run_headless`, driven against the bundled
//! `headless_test_child` helper binary (located via `CARGO_BIN_EXE_*`, which Cargo
//! sets for integration tests). A real child process makes the spawn/drain/kill
//! plumbing observable, portably across Windows and Unix.

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use ralphy_adapter_support::run_headless;

// These mirror the constants in `src/bin/headless_test_child.rs`. Kept in sync by
// hand — a drift would fail the assertions below immediately.
const CLEAN_STDOUT: &str = "hello-from-stdout";
const CLEAN_STDERR: &str = "hello-from-stderr";
const LARGE_LEN: usize = 200_000;

/// Build a `Command` for the helper child in the given `mode`, with stdin/stdout/
/// stderr piped exactly as the adapters do before handing the command off.
fn child_cmd(mode: &str) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_headless_test_child"));
    cmd.arg(mode)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    cmd
}

#[test]
fn clean_exit_captures_stdout_and_stderr_separately() {
    let r = run_headless(
        child_cmd("clean"),
        "ignored prompt",
        Duration::from_secs(30),
    )
    .expect("run_headless should not error on a clean child");

    // stdout and stderr come back as distinct strings — not concatenated.
    assert_eq!(r.stdout, CLEAN_STDOUT, "stdout captured verbatim");
    assert_eq!(r.stderr, CLEAN_STDERR, "stderr captured verbatim");
    assert!(!r.timed_out, "a clean child did not time out");
    let status = r.exit.expect("a natural exit yields Some(status)");
    assert!(status.success(), "the clean child exits 0");
}

#[test]
fn timeout_kills_child_and_returns_promptly() {
    let started = Instant::now();
    let r = run_headless(
        child_cmd("sleep"),
        "ignored prompt",
        Duration::from_millis(300),
    )
    .expect("run_headless should not error when killing on timeout");
    let elapsed = started.elapsed();

    assert!(r.timed_out, "a child outliving the timeout sets timed_out");
    assert!(r.exit.is_none(), "a killed child reports exit == None");
    // The call returns rather than hanging for the child's full ~60s sleep. Allow
    // generous slack for the 500ms poll tick and the 5s collect grace.
    assert!(
        elapsed < Duration::from_secs(30),
        "run_headless returned promptly after the deadline (took {elapsed:?})"
    );
}

#[test]
fn timeout_with_surviving_grandchild_still_returns_promptly() {
    // A grandchild inherits the child's stdout pipe and outlives the direct child.
    // Only run_headless's process-tree kill closes that pipe; without it the reader
    // would block on the still-open write-end and the collect grace would hang.
    let started = Instant::now();
    let r = run_headless(
        child_cmd("sleep-with-grandchild"),
        "ignored prompt",
        Duration::from_millis(300),
    )
    .expect("run_headless should not error when killing a tree on timeout");
    let elapsed = started.elapsed();

    assert!(r.timed_out, "the child tree outlived the timeout");
    assert!(r.exit.is_none(), "a killed child reports exit == None");
    assert!(
        elapsed < Duration::from_secs(30),
        "tree-kill closes the inherited pipe so the reader doesn't hang (took {elapsed:?})"
    );
}

#[test]
fn large_output_is_captured_complete() {
    let r = run_headless(
        child_cmd("large"),
        "ignored prompt",
        Duration::from_secs(30),
    )
    .expect("run_headless should not error on a large-output child");

    assert!(
        !r.timed_out,
        "the large child exits cleanly within the timeout"
    );
    assert_eq!(
        r.stdout.len(),
        LARGE_LEN,
        "the full >64KB stream is captured with no truncation"
    );
}
