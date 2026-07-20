//! Test helper child driven by `tests/headless.rs` via `CARGO_BIN_EXE_*`. Its
//! single argv arg selects a deterministic behavior so the shared `run_headless`
//! loop can be exercised against a real process, portably on Windows and Unix:
//!
//! - `clean` — write a known line to stdout and a *different* known line to
//!   stderr, then exit 0.
//! - `sleep` — sleep ~60s (the timeout-and-kill case); never exits on its own
//!   within a test's patience.
//! - `sleep-with-grandchild` — spawn a copy of itself (in `sleep` mode) that
//!   inherits this process's stdout, then sleep ~60s. The grandchild keeps the
//!   stdout pipe write-end open after the direct child dies, so it exercises
//!   `run_headless`'s process-tree kill: a plain `child.kill()` would leave the
//!   reader blocked on the still-open pipe.
//! - `large` — emit a large (>64KB) stream to stdout, then exit 0 (the
//!   no-truncation case).
//! - `stderr-then-sleep` — write a newline-terminated marker to stderr (so the
//!   reader's `read_until` returns it at once), then sleep ~60s. Exercises the
//!   early-kill switch: a watcher matching the marker must reap the child on the
//!   line, not wait out the sleep.
//! - `chatty` — emit a line every [`CHATTY_TICK`] for ~60s. The counterpart to
//!   `sleep` for the idle watchdog: a child that keeps talking must **survive** a
//!   window far shorter than its total runtime, proving the watchdog measures
//!   silence and not elapsed time.
//! - `echo-stdin` — read ALL of stdin and write it back verbatim, then exit 0.
//!   The charter channel every headless adapter depends on (ADR-0041 D2): the
//!   only way to prove a >24 KB prompt survives the write end to end is to have a
//!   real child read it and hand it back.
//! - `degraded-chatty` — emit a [`DEGRADED_MARKER`] line every [`CHATTY_TICK`] for
//!   ~60s: a child talking at the chatty cadence but only ever printing degraded
//!   banners. Exercises the API-degraded path — a matched degraded line must NOT
//!   rearm the idle beacon, so this child is idle-reaped **despite** talking,
//!   unlike `chatty`.
//!
//! The stdout/stderr marker lines and the large-output byte count are kept in sync
//! with the assertions in `tests/headless.rs` via the shared constants below.

use std::io::Write;
use std::time::Duration;

/// The exact line the `clean` child writes to stdout.
pub const CLEAN_STDOUT: &str = "hello-from-stdout";
/// The exact line the `clean` child writes to stderr (distinct from stdout).
pub const CLEAN_STDERR: &str = "hello-from-stderr";
/// The byte count the `large` child emits to stdout — comfortably past the
/// ~64KB pipe buffer so a truncating loop would be caught.
pub const LARGE_LEN: usize = 200_000;
/// The newline-terminated stderr line the `stderr-then-sleep` child emits before
/// sleeping — the early-kill watcher matches on it.
pub const STDERR_MARKER: &str = "quota-marker: usage limit reached";
/// How often the `chatty` child emits a line — short enough that several land
/// inside any idle window a test can afford to wait out.
pub const CHATTY_TICK: Duration = Duration::from_millis(100);
/// The line the `degraded-chatty` child emits every tick — a representative
/// degraded/retry banner the caller's `degraded_line` predicate matches on.
pub const DEGRADED_MARKER: &str = "Waiting for API response";

fn main() {
    let mode = std::env::args().nth(1).unwrap_or_default();
    match mode.as_str() {
        "clean" => {
            print!("{CLEAN_STDOUT}");
            let _ = std::io::stdout().flush();
            eprint!("{CLEAN_STDERR}");
            let _ = std::io::stderr().flush();
        }
        "sleep" => {
            std::thread::sleep(Duration::from_secs(60));
        }
        "sleep-with-grandchild" => {
            // Spawn a grandchild that inherits our stdout, so the pipe write-end
            // stays open even after we are killed. Then sleep. Only a process-tree
            // kill closes the pipe and lets the reader reach EOF.
            if let Ok(exe) = std::env::current_exe() {
                let _ = std::process::Command::new(exe).arg("sleep").spawn();
            }
            std::thread::sleep(Duration::from_secs(60));
        }
        "stderr-then-sleep" => {
            // Emit the marker as a full LINE (newline + flush) so the reader's
            // `read_until` returns it immediately, then sleep past any test's
            // patience. The early-kill switch must reap us on the marker line.
            eprintln!("{STDERR_MARKER}");
            let _ = std::io::stderr().flush();
            std::thread::sleep(Duration::from_secs(60));
        }
        "chatty" => {
            // Newline + flush per tick so each one reaches the reader (and thus the
            // idle beacon) immediately, rather than sitting in a block buffer.
            for i in 0..600 {
                println!("tick {i}");
                let _ = std::io::stdout().flush();
                std::thread::sleep(CHATTY_TICK);
            }
        }
        "degraded-chatty" => {
            // Only degraded banners, at the chatty cadence: the child never goes
            // silent, yet every line matches the degraded predicate, so none of
            // them rearm the idle beacon and the watchdog reaps it anyway.
            for i in 0..600 {
                println!("{DEGRADED_MARKER} (attempt {i})");
                let _ = std::io::stdout().flush();
                std::thread::sleep(CHATTY_TICK);
            }
        }
        "echo-stdin" => {
            // Read to EOF before writing a byte: a partial read would silently
            // truncate exactly the way this mode exists to detect.
            let mut buf = String::new();
            let _ = std::io::Read::read_to_string(&mut std::io::stdin(), &mut buf);
            let _ = std::io::stdout().write_all(buf.as_bytes());
            let _ = std::io::stdout().flush();
        }
        "large" => {
            // A repeating byte pattern, written in one shot, so the test can assert
            // the captured length exactly.
            let blob = vec![b'x'; LARGE_LEN];
            let _ = std::io::stdout().write_all(&blob);
            let _ = std::io::stdout().flush();
        }
        other => {
            eprintln!("unknown mode: {other:?}");
            std::process::exit(2);
        }
    }
}
