//! The cooperative stop reaching a live headless child (docs/adr/0054).
//!
//! **Its own test binary, and nothing else may join it.** `ralphy_core::stop` is
//! a process-global flag; `cargo test` shares one process per test binary (and
//! runs its tests on several threads), so a stop test living in `headless.rs`
//! would reap the children of every test that ran alongside it — as an
//! unreproducible timing flake, not as an error. [`StopFlagGuard`] serializes
//! these and restores the flag even through a panicking assertion.
//!
//! Driven against the same bundled `headless_test_child` the sibling suite uses,
//! because the thing under test is a real process being reaped.
#![expect(
    clippy::tests_outside_test_module,
    reason = "an integration test crate is the test module"
)]

use std::process::{Command, Stdio};
use std::sync::{Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

use ralphy_adapter_support::{run_headless, HeadlessCall};

static STOP_FLAG_LOCK: Mutex<()> = Mutex::new(());

struct StopFlagGuard(
    #[expect(
        dead_code,
        reason = "the field only holds the lock until the guard drops"
    )]
    MutexGuard<'static, ()>,
);

impl StopFlagGuard {
    fn acquire() -> Self {
        let lock = STOP_FLAG_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        ralphy_core::stop::clear();
        Self(lock)
    }
}

impl Drop for StopFlagGuard {
    fn drop(&mut self) {
        ralphy_core::stop::clear();
    }
}

/// Remove a scratch file a test made. It may be gone already, and a leftover
/// file in the temp dir fails nothing.
fn remove_scratch(file: &std::path::Path) {
    // cleanup of a test file that may be gone, and no test reads it after
    match std::fs::remove_file(file) {
        Ok(()) | Err(_) => {}
    }
}

fn child_cmd(mode: &str) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_headless_test_child"));
    cmd.arg(mode)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    cmd
}

fn temp_log(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("ralphy-stop-{}-{}.log", std::process::id(), name))
}

/// The flag raised MID-CALL, which is the real shape: the setter is the snapshot
/// delivery worker on another thread, and the run is already deep inside a child
/// that would otherwise sleep out its whole budget.
#[test]
fn a_stop_request_reaps_a_live_child_promptly() {
    let _flag = StopFlagGuard::acquire();

    let waker = thread::spawn(|| {
        thread::sleep(Duration::from_millis(300));
        ralphy_core::stop::request();
    });

    let started = Instant::now();
    let r = run_headless(
        child_cmd("sleep"),
        "ignored prompt",
        // A wall timeout the child would otherwise sleep out entirely, so a
        // prompt return can only mean the stop fired.
        Duration::from_secs(60),
    )
    .expect("a stopped run returns its output, it does not error");
    let elapsed = started.elapsed();
    waker.join().unwrap();

    assert!(
        elapsed < Duration::from_secs(30),
        "the stop reaped the child rather than waiting out the 60s timeout (took {elapsed:?})"
    );
    assert!(r.stopped, "the discriminator says it was a stop");
    assert!(
        !r.timed_out,
        "a stop is an explicit act, not a timeout — collapsing the two would \
         report `Outcome::Timeout` for a button press"
    );
    assert!(r.exit.is_none(), "a killed child has no exit status");
}

/// The inert half, mirroring `run_headless_logged_watched_without_a_match_times_out_normally`.
/// Without it, a check accidentally inverted into `if !requested()` would reap
/// every child in the workspace and the test above would still be green.
#[test]
fn without_a_stop_request_the_wall_timeout_still_fires() {
    let _flag = StopFlagGuard::acquire();

    let r = run_headless(
        child_cmd("sleep"),
        "ignored prompt",
        Duration::from_millis(300),
    )
    .expect("an unstopped run still returns");

    assert!(r.timed_out, "no stop → the wall timeout owns the kill");
    assert!(!r.stopped, "…and it is not reported as a stop");
}

/// A stop must not be laundered into a timeout anywhere along the builder path
/// either — `HeadlessRun` is what every adapter actually reads, and the two
/// flags must stay distinguishable all the way out to it.
#[test]
fn the_builder_path_reports_a_stop_as_a_stop_not_a_timeout() {
    let _flag = StopFlagGuard::acquire();
    let log_path = temp_log("builder");
    remove_scratch(&log_path);

    // Pre-set: nothing about the discrimination depends on WHEN the flag rises,
    // and a pre-set flag makes this test cost milliseconds instead of seconds.
    ralphy_core::stop::request();
    let r = HeadlessCall::new(
        child_cmd("sleep"),
        "ignored prompt",
        Duration::from_secs(60),
        &log_path,
    )
    .run()
    .expect("a stopped builder run returns");

    assert!(r.stopped, "HeadlessRun carries the stop through");
    assert!(!r.timed_out, "and never as a timeout");
    assert!(!r.exited_cleanly, "a reaped child did not exit cleanly");
    remove_scratch(&log_path);
}

/// Run the test child's `drive-stop` mode with extra Windows creation `flags`
/// and return the one result line it prints.
fn drive_stop(mode: &str, flags: u32) -> String {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_headless_test_child"));
    cmd.args(["drive-stop", mode])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    std::os::windows::process::CommandExt::creation_flags(&mut cmd, flags);
    #[cfg(not(windows))]
    let _: u32 = flags;
    let out = cmd.output().expect("the driver child starts");
    assert!(
        out.status.success(),
        "the driver failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

/// The ways a test spawns the driver: on Windows with a hidden console of its
/// own, and with none; elsewhere as is.
fn driver_shapes() -> &'static [(&'static str, u32)] {
    if cfg!(windows) {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        &[
            ("console", CREATE_NO_WINDOW),
            ("no console", DETACHED_PROCESS),
        ]
    } else {
        &[("as is", 0)]
    }
}

/// The exit code of the test child on the stop signal: its own SIGTERM handler
/// on Unix, `STATUS_CONTROL_C_EXIT` of the default `CTRL_BREAK_EVENT` handler on Windows.
const SIGNALLED_EXIT: i32 = if cfg!(windows) {
    0xC000_013A_u32.cast_signed()
} else {
    7
};

#[test]
fn a_cooperative_stop_reaps_a_child_that_exits_on_the_signal() {
    for &(shape, flags) in driver_shapes() {
        let line = drive_stop("cooperative", flags);
        assert_eq!(
            line,
            format!("exit_code=Some({SIGNALLED_EXIT}) stopped=true kept=true"),
            "{shape}: the child exited on the signal, was reported as a stop, and its line \
             was kept"
        );
    }
}

#[test]
fn a_plain_stop_still_kills_the_tree_at_once() {
    for &(shape, flags) in driver_shapes() {
        let line = drive_stop("plain", flags);
        assert_eq!(
            line, "exit_code=None stopped=true kept=true",
            "{shape}: a call without cooperative stop is killed, with no exit status"
        );
    }
}
