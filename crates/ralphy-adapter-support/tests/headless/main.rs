//! Direct unit tests for `run_headless`, driven against the bundled
//! `headless_test_child` helper binary (located via `CARGO_BIN_EXE_*`, which Cargo
//! sets for integration tests). A real child process makes the spawn/drain/kill
//! plumbing observable, portably across Windows and Unix.
#![expect(
    clippy::tests_outside_test_module,
    reason = "an integration test crate is the test module"
)]

use std::fmt::Write as _;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use ralphy_adapter_support::{
    run_headless, run_headless_logged, run_headless_logged_watched, HeadlessCall,
};

mod reaping;
mod session;

// These mirror the constants in `src/bin/headless_test_child.rs`. Kept in sync by
// hand — a drift would fail the assertions below immediately.
const CLEAN_STDOUT: &str = "hello-from-stdout";
const CLEAN_STDERR: &str = "hello-from-stderr";
const LARGE_LEN: usize = 200_000;
const STDERR_MARKER: &str = "quota-marker: usage limit reached";
const DEGRADED_MARKER: &str = "Waiting for API response";
const LEAK_MARKER: &str = "output-before-the-leak";

/// Remove a scratch file a test made. It may be gone already, and a leftover
/// file in the temp dir fails nothing.
fn remove_scratch(file: &std::path::Path) {
    // cleanup of a test file that may be gone, and no test reads it after
    match std::fs::remove_file(file) {
        Ok(()) | Err(_) => {}
    }
}

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
fn natural_exit_with_leaked_grandchild_still_captures_the_output() {
    // The cursor #244 shape: the agent CLI exits ON ITS OWN, but an orphan it
    // spawned inherits stdout and holds the write-end open. Before the
    // pre-collect tree-kill, the reader never reached EOF, the collect grace
    // expired, and the capture came back EMPTY — silently dropping the very
    // output the limit/auth detectors scan. The reader delivers its buffer only
    // at EOF, so capturing the marker proves the kill closed the leaked pipe.
    let started = Instant::now();
    let r = run_headless(
        child_cmd("exit-leaking-grandchild"),
        "ignored prompt",
        Duration::from_secs(60),
    )
    .expect("run_headless should not error on a natural exit with a leaked orphan");
    let elapsed = started.elapsed();

    assert!(!r.timed_out, "the direct child exited on its own");
    let status = r.exit.expect("a natural exit yields Some(status)");
    assert!(status.success(), "the direct child exited 0");
    assert!(
        r.stdout.contains(LEAK_MARKER),
        "the child's output is captured despite the leaked pipe holder — got {:?}",
        r.stdout
    );
    assert!(
        elapsed < Duration::from_secs(30),
        "returned promptly, not after the orphan's 60s lifetime (took {elapsed:?})"
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

/// A unique temp path for a per-test log file (no `tempfile` dev-dep in this
/// crate — mirror the manual temp-dir pattern used elsewhere).
fn temp_log(tag: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "ralphy-headless-log-{tag}-{}.log",
        std::process::id()
    ))
}

#[test]
fn run_headless_logged_captures_flags_and_persists_the_log() {
    let log_path = temp_log("clean");
    remove_scratch(&log_path);

    let r = run_headless_logged(
        child_cmd("clean"),
        "ignored prompt",
        Duration::from_secs(30),
        &log_path,
    )
    .expect("run_headless_logged should not error on a clean child");

    // stdout is kept apart; the log carries BOTH streams.
    assert!(r.stdout.contains(CLEAN_STDOUT), "stdout carries the marker");
    assert!(
        !r.stdout.contains(CLEAN_STDERR),
        "stdout must not carry the stderr marker"
    );
    assert!(r.log.contains(CLEAN_STDOUT), "log carries stdout");
    assert!(r.log.contains(CLEAN_STDERR), "log carries stderr");
    assert!(r.exited_cleanly, "a clean child exited cleanly");
    assert!(!r.timed_out, "a clean child did not time out");

    // The persisted file equals the in-memory log.
    let on_disk = std::fs::read_to_string(&log_path).expect("log file was written");
    assert_eq!(on_disk, r.log, "the persisted log equals the returned log");
    remove_scratch(&log_path);
}

#[test]
fn run_headless_logged_reports_timeout_and_not_clean() {
    let log_path = temp_log("sleep");
    remove_scratch(&log_path);

    let r = run_headless_logged(
        child_cmd("sleep"),
        "ignored prompt",
        Duration::from_millis(300),
        &log_path,
    )
    .expect("run_headless_logged should not error when killing on timeout");

    assert!(r.timed_out, "a child outliving the timeout sets timed_out");
    assert!(
        !r.exited_cleanly,
        "a killed child did not exit cleanly (exit == None)"
    );
    remove_scratch(&log_path);
}

#[test]
fn run_headless_logged_watched_early_kills_on_matching_stderr_line() {
    let log_path = temp_log("early-kill");
    remove_scratch(&log_path);

    let started = Instant::now();
    let r = run_headless_logged_watched(
        child_cmd("stderr-then-sleep"),
        "ignored prompt",
        // A generous wall timeout the child would otherwise sleep out entirely, so a
        // prompt return can only mean the early-kill switch fired on the marker.
        Duration::from_secs(60),
        &log_path,
        |line| line.contains(STDERR_MARKER),
    )
    .expect("a watched run should not error");
    let elapsed = started.elapsed();

    assert!(
        elapsed < Duration::from_secs(30),
        "the switch reaped the child on the stderr marker, not on the 60s timeout \
         (took {elapsed:?})"
    );
    assert!(
        !r.timed_out,
        "an early-kill is an explicit signal, not a timeout"
    );
    assert!(!r.exited_cleanly, "a killed child did not exit cleanly");
    assert!(
        r.log.contains(STDERR_MARKER),
        "the matched stderr line is captured in the log"
    );
    remove_scratch(&log_path);
}

#[test]
fn run_headless_logged_watched_without_a_match_times_out_normally() {
    // A non-matching predicate leaves the switch inert: the sleep child runs to the
    // wall timeout exactly like the unwatched path, proving the switch only ever
    // shortens the run when it actually fires.
    let log_path = temp_log("watched-timeout");
    remove_scratch(&log_path);

    let r = run_headless_logged_watched(
        child_cmd("sleep"),
        "ignored prompt",
        Duration::from_millis(300),
        &log_path,
        |_line| false,
    )
    .expect("a watched run should not error when killing on timeout");

    assert!(r.timed_out, "no match → the wall timeout still fires");
    assert!(!r.exited_cleanly, "a killed child did not exit cleanly");
    remove_scratch(&log_path);
}

// ── idle watchdog (docs/adr/0038) ───────────────────────────────────────────

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

/// The charter channel, proved end to end (#229 / ADR-0041 D2). Every headless
/// adapter pipes a >23 KB charter on stdin because argv cannot carry it on
/// Windows; a write that stops short would truncate the agent's instructions with
/// no error anywhere. Markers on BOTH the first and last line, so a truncation at
/// either end fails: a head-only assertion passes on a payload cut in half.
#[test]
fn stdin_payload_over_24kb_round_trips() {
    const HEAD: &str = "RALPHY-STDIN-HEAD";
    const TAIL: &str = "RALPHY-STDIN-TAIL";

    let mut payload = String::from(HEAD);
    payload.push('\n');
    let mut i = 0usize;
    while payload.len() < 24_576 {
        writeln!(payload, "filler line {i} — padding the charter to size")
            .expect("a write to a String does not fail");
        i += 1;
    }
    payload.push_str(TAIL);
    payload.push('\n');
    assert!(payload.len() > 24_576, "payload is {} bytes", payload.len());

    let log_path = temp_log("stdin-24kb");
    remove_scratch(&log_path);

    let r = HeadlessCall::new(
        child_cmd("echo-stdin"),
        &payload,
        Duration::from_secs(60),
        &log_path,
    )
    .run()
    .expect("the echo child should not error");

    assert!(r.exited_cleanly, "child exited cleanly: {:?}", r.exit_code);
    assert_eq!(
        r.stdout.lines().next(),
        Some(HEAD),
        "the FIRST line survived the write"
    );
    assert_eq!(
        r.stdout.lines().rfind(|l| !l.trim().is_empty()),
        Some(TAIL),
        "the LAST line survived the write"
    );

    remove_scratch(&log_path);
}
