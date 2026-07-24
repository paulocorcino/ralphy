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
//! - `exit-leaking-grandchild` — write [`LEAK_MARKER`] to stdout, spawn a copy of
//!   itself (in `sleep` mode) that inherits this process's stdout, then exit 0
//!   *immediately*. The natural-exit counterpart of `sleep-with-grandchild` (the
//!   cursor #244 shape): the agent CLI exits on its own but its orphan holds the
//!   pipe, so only the pre-collect tree-kill lets the reader reach EOF — the
//!   marker arriving is the proof.
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
//! - `heartbeat <path>` — append `"leaf tick\n"` to `<path>` every [`CHATTY_TICK`]
//!   for ~60s. The leaf of the survival tree: it writes to a FILE, never to stdout,
//!   so its liveness is observable *after* the runner has closed the pipes — the
//!   file growing past a kill is the only evidence a descendant outlived it.
//!   Writing to a file rather than stdout is also what keeps the idle beacon
//!   un-rearmed, which is what makes the idle-kill path reachable at all.
//! - `heartbeat-tree <path>` / `heartbeat-tree-inner <path>` — spawn the next level
//!   down (`heartbeat-tree-inner`, then `heartbeat`) with stdout INHERITED, then
//!   beat as `L1` / `L2` into the same file. Three levels below the runner,
//!   mirroring the depth of the Node process tree the vendor CLIs run under
//!   (ADR-0043 D18) — and every level writes, so a teardown that orphaned an
//!   INTERMEDIATE node is caught too, not only one that orphaned the leaf.
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
/// The stdout line the `exit-leaking-grandchild` child writes before leaking its
/// orphan — capturing it proves the reader reached EOF past the leaked pipe.
pub const LEAK_MARKER: &str = "output-before-the-leak";

/// Append `<label> tick\n` to the heartbeat file every [`CHATTY_TICK`] for ~60s.
///
/// EVERY level of the tree runs this, not only the leaf: a teardown that reaped
/// the leaf but orphaned an intermediate node would otherwise leave nothing
/// observable, and both survival tests would pass while the tree they are named
/// after still had a live member.
fn beat(label: &str) {
    let path = std::env::args().nth(2).unwrap_or_default();
    let line = format!("{label} tick\n");
    for _ in 0..600 {
        // Reopen-append per tick and flush: the test reads the file's length from
        // another process while this one is still running, so a buffered write
        // would look like a frozen descendant.
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
        {
            let _ = f.write_all(line.as_bytes());
            let _ = f.flush();
        }
        std::thread::sleep(CHATTY_TICK);
    }
}

/// Spawn a copy of ourselves one level deeper in `next` mode, forwarding the
/// heartbeat path, then beat as `label`. stdout is INHERITED so the whole tree
/// holds the runner's pipe open — only a process-tree kill closes it.
fn spawn_next_level(next: &str, label: &str) {
    let path = std::env::args().nth(2).unwrap_or_default();
    match std::env::current_exe()
        .map(|exe| std::process::Command::new(exe).arg(next).arg(&path).spawn())
    {
        Ok(Ok(_)) => {}
        // Loud, because a silent spawn failure surfaces downstream as "the leaf
        // never wrote", which reads as a kill-logic regression instead of an
        // environment problem.
        other => eprintln!("failed to spawn {next}: {other:?}"),
    }
    beat(label);
}

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
        "exit-leaking-grandchild" => {
            // The output the runner must still capture: it only arrives if the
            // reader reaches EOF, which — with the orphan below holding the
            // write-end — only the pre-collect tree-kill can force.
            println!("{LEAK_MARKER}");
            let _ = std::io::stdout().flush();
            // Grandchild inherits our stdout, then we exit 0 straight away — the
            // orphan holds the pipe open past our natural exit.
            if let Ok(exe) = std::env::current_exe() {
                let _ = std::process::Command::new(exe).arg("sleep").spawn();
            }
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
        "heartbeat" => beat("leaf"),
        "heartbeat-tree" => spawn_next_level("heartbeat-tree-inner", "L1"),
        "heartbeat-tree-inner" => spawn_next_level("heartbeat", "L2"),
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
