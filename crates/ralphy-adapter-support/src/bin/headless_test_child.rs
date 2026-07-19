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
