//! Direct unit tests for `run_headless`, driven against the bundled
//! `headless_test_child` helper binary (located via `CARGO_BIN_EXE_*`, which Cargo
//! sets for integration tests). A real child process makes the spawn/drain/kill
//! plumbing observable, portably across Windows and Unix.

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use ralphy_adapter_support::{
    run_headless, run_headless_logged, run_headless_logged_watched, run_init_session,
    run_text_session, HeadlessCall, JsonSession, TextSession,
};

// These mirror the constants in `src/bin/headless_test_child.rs`. Kept in sync by
// hand — a drift would fail the assertions below immediately.
const CLEAN_STDOUT: &str = "hello-from-stdout";
const CLEAN_STDERR: &str = "hello-from-stderr";
const LARGE_LEN: usize = 200_000;
const STDERR_MARKER: &str = "quota-marker: usage limit reached";
const DEGRADED_MARKER: &str = "Waiting for API response";

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
    let _ = std::fs::remove_file(&log_path);

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
    let _ = std::fs::remove_file(&log_path);
}

#[test]
fn run_headless_logged_reports_timeout_and_not_clean() {
    let log_path = temp_log("sleep");
    let _ = std::fs::remove_file(&log_path);

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
    let _ = std::fs::remove_file(&log_path);
}

#[test]
fn run_headless_logged_watched_early_kills_on_matching_stderr_line() {
    let log_path = temp_log("early-kill");
    let _ = std::fs::remove_file(&log_path);

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
        "the switch reaped the child on the stderr marker, not on the 60s timeout (took {elapsed:?})"
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
    let _ = std::fs::remove_file(&log_path);
}

#[test]
fn run_headless_logged_watched_without_a_match_times_out_normally() {
    // A non-matching predicate leaves the switch inert: the sleep child runs to the
    // wall timeout exactly like the unwatched path, proving the switch only ever
    // shortens the run when it actually fires.
    let log_path = temp_log("watched-timeout");
    let _ = std::fs::remove_file(&log_path);

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
    let _ = std::fs::remove_file(&log_path);
}

// ── idle watchdog (docs/adr/0038) ───────────────────────────────────────────

#[test]
fn the_idle_watchdog_reaps_a_silent_child_long_before_the_wall_timeout() {
    // This is the OpenCode failure mode in miniature: a child that hit a provider
    // quota block, swallowed it into a silent retry, and now prints nothing at all.
    // No stderr matcher can see a failure that is never printed — but the silence
    // itself is unmistakable, and it is the only signal that catches this.
    let log_path = temp_log("idle-kill");
    let _ = std::fs::remove_file(&log_path);

    let started = Instant::now();
    let r = HeadlessCall::new(
        child_cmd("sleep"),
        "ignored prompt",
        // A wall timeout the child would happily sleep out: a prompt return can
        // only mean the idle watchdog fired.
        Duration::from_secs(60),
        &log_path,
    )
    .idle_window(Duration::from_secs(1))
    .run()
    .expect("an idle-watched run should not error");
    let elapsed = started.elapsed();

    assert!(
        elapsed < Duration::from_secs(30),
        "the watchdog reaped the silent child, it did not wait out the 60s wall timeout (took {elapsed:?})"
    );
    assert!(r.idle_killed, "the idle watchdog fired, not the wall clock");
    assert!(
        r.timed_out,
        "an idle kill still reports as a timeout so classification is unchanged"
    );
    assert!(!r.exited_cleanly, "a killed child did not exit cleanly");
    let _ = std::fs::remove_file(&log_path);
}

#[test]
fn the_idle_watchdog_spares_a_child_that_keeps_talking() {
    // The regression this whole change risks: killing healthy work. The chatty
    // child runs far longer than the idle window but never goes quiet for a full
    // one, so it must survive — proving the watchdog measures silence, not elapsed
    // time. (That distinction is the entire reason it replaced a wall-clock cap.)
    let log_path = temp_log("idle-chatty");
    let _ = std::fs::remove_file(&log_path);

    let r = HeadlessCall::new(
        child_cmd("chatty"),
        "ignored prompt",
        // Wall timeout well past the idle window: whatever ends this run, it is
        // not the wall clock racing the watchdog.
        Duration::from_secs(3),
        &log_path,
    )
    .idle_window(Duration::from_secs(1))
    .run()
    .expect("a chatty child should not error");

    assert!(
        !r.idle_killed,
        "a child emitting a line every 100ms is never idle for a 1s window"
    );
    assert!(
        r.log.contains("tick "),
        "the child's output was captured while it ran"
    );
    let _ = std::fs::remove_file(&log_path);
}

#[test]
fn degraded_only_child_is_idle_reaped_despite_talking() {
    // The headless counterpart to the PTY `Waiting for API response` wedge: a child
    // that keeps printing degraded banners at the chatty cadence but makes no real
    // progress. Because every line matches the degraded predicate, none of them
    // rearm the idle beacon, so the watchdog reaps it despite the constant chatter.
    // This FAILS before the degraded plumbing exists (the banners would beat).
    let log_path = temp_log("degraded-idle");
    let _ = std::fs::remove_file(&log_path);

    let started = Instant::now();
    let r = HeadlessCall::new(
        child_cmd("degraded-chatty"),
        "ignored prompt",
        Duration::from_secs(60),
        &log_path,
    )
    .degraded_line(|l| l.contains(DEGRADED_MARKER))
    .idle_window(Duration::from_secs(1))
    .run()
    .expect("a degraded-watched run should not error");
    let elapsed = started.elapsed();

    assert!(
        r.idle_killed,
        "a child emitting only degraded lines is reaped by the idle watchdog"
    );
    assert!(
        elapsed < Duration::from_secs(30),
        "reaped promptly despite talking every 100ms (took {elapsed:?})"
    );
    let _ = std::fs::remove_file(&log_path);
}

#[test]
fn healthy_child_with_a_degraded_matcher_survives() {
    // The non-regression: arming a degraded matcher must not endanger a healthy
    // child. The chatty child's lines never match the predicate, so they still beat
    // the idle beacon and the child survives — proving the degraded gate only ever
    // stops *matching* lines from counting as progress.
    let log_path = temp_log("degraded-healthy");
    let _ = std::fs::remove_file(&log_path);

    let r = HeadlessCall::new(
        child_cmd("chatty"),
        "ignored prompt",
        Duration::from_secs(3),
        &log_path,
    )
    .degraded_line(|l| l.contains(DEGRADED_MARKER))
    .idle_window(Duration::from_secs(1))
    .run()
    .expect("a healthy chatty child should not error");

    assert!(
        !r.idle_killed,
        "the matcher never matches, so healthy lines still rearm the beacon"
    );
    assert!(
        r.log.contains("tick "),
        "the child's healthy output was captured while it ran"
    );
    let _ = std::fs::remove_file(&log_path);
}

#[test]
fn a_disabled_idle_watchdog_leaves_the_run_exactly_as_it_was() {
    // `0` is the operator's opt-out: the silent child now runs to the wall timeout
    // exactly as it did before the watchdog existed.
    let log_path = temp_log("idle-off");
    let _ = std::fs::remove_file(&log_path);

    let r = HeadlessCall::new(
        child_cmd("sleep"),
        "ignored prompt",
        Duration::from_millis(300),
        &log_path,
    )
    .idle_minutes(0)
    .run()
    .expect("a run with the watchdog disabled should not error");

    assert!(r.timed_out, "the wall timeout still fires");
    assert!(!r.idle_killed, "the watchdog was off, so it did not fire");
    let _ = std::fs::remove_file(&log_path);
}

#[test]
fn run_text_session_returns_the_log_and_bails_on_auth_then_timeout() {
    // Clean child: no auth match, no timeout → returns the combined log.
    let log_path = temp_log("text-clean");
    let _ = std::fs::remove_file(&log_path);
    let log = run_text_session(
        TextSession {
            cmd: child_cmd("clean"),
            prompt: "ignored prompt",
            timeout: Duration::from_secs(30),
            log_path: &log_path,
            spawn_err: "failed to spawn the test child",
            auth_msg: "AUTH FAILED",
            timeout_msg: "TIMED OUT",
        },
        |_log| false,
    )
    .expect("a clean child with no auth match returns the log");
    assert!(log.contains(CLEAN_STDOUT) && log.contains(CLEAN_STDERR));
    assert!(log_path.is_file(), "the log file was written");
    let _ = std::fs::remove_file(&log_path);

    // Auth detector fires on the stdout marker → Err carries auth_msg + log path.
    let log_path = temp_log("text-auth");
    let _ = std::fs::remove_file(&log_path);
    let err = run_text_session(
        TextSession {
            cmd: child_cmd("clean"),
            prompt: "ignored prompt",
            timeout: Duration::from_secs(30),
            log_path: &log_path,
            spawn_err: "failed to spawn the test child",
            auth_msg: "AUTH FAILED",
            timeout_msg: "TIMED OUT",
        },
        |log| log.contains(CLEAN_STDOUT),
    )
    .expect_err("an auth match must bail");
    let msg = format!("{err}");
    assert!(
        msg.contains("AUTH FAILED"),
        "err names the auth message: {msg}"
    );
    assert!(
        msg.contains(&log_path.display().to_string()),
        "err names the log path: {msg}"
    );
    let _ = std::fs::remove_file(&log_path);

    // Sleep child (no auth match) → timeout bail with timeout_msg.
    let log_path = temp_log("text-timeout");
    let _ = std::fs::remove_file(&log_path);
    let err = run_text_session(
        TextSession {
            cmd: child_cmd("sleep"),
            prompt: "ignored prompt",
            timeout: Duration::from_millis(300),
            log_path: &log_path,
            spawn_err: "failed to spawn the test child",
            auth_msg: "AUTH FAILED",
            timeout_msg: "TIMED OUT",
        },
        |_log| false,
    )
    .expect_err("a timed-out session must bail");
    assert!(
        format!("{err}").contains("TIMED OUT"),
        "err names the timeout message: {err}"
    );
    let _ = std::fs::remove_file(&log_path);
}

#[test]
fn run_init_session_clears_a_stale_artifact_before_the_run() {
    // A stale artifact from a prior run must never survive into this session. Drive
    // a child that writes NO artifact, seed a stale `out_path`, and assert the run
    // bails with `missing_msg` (proving the file was cleared, not read back).
    let log_path = temp_log("init-log");
    let out_path = temp_log("init-out");
    let _ = std::fs::remove_file(&log_path);
    std::fs::write(&out_path, "stale contents from a prior run").unwrap();

    let err = run_init_session(
        JsonSession {
            cmd: child_cmd("clean"),
            prompt: "ignored prompt",
            timeout: Duration::from_secs(30),
            log_path: &log_path,
            out_path: &out_path,
            spawn_err: "failed to spawn the test child",
            auth_msg: "AUTH FAILED",
            timeout_msg: "TIMED OUT",
            missing_msg: "NO ARTIFACT",
        },
        |_log| false,
        |raw| Ok::<String, anyhow::Error>(raw.to_string()),
    )
    .expect_err("the stale artifact was cleared, so the read must fail with missing_msg");

    assert!(
        format!("{err}").contains("NO ARTIFACT"),
        "err names the missing-artifact message: {err}"
    );
    assert!(
        !out_path.exists(),
        "the stale artifact was removed before the run"
    );
    let _ = std::fs::remove_file(&log_path);
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
