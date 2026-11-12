//! Test helper child driven by `tests/verify_gate.rs` via `CARGO_BIN_EXE_*`. Its
//! single argv arg selects a deterministic behavior so the verify gate's
//! spawn/drain/kill loop can be exercised against a real process, portably on
//! Windows and Unix (no shell-script children — CONTEXT.md testing conventions).
//!
//! - `exit-leaking-grandchild` — write a marker line to stdout, spawn a copy of
//!   itself (in `sleep` mode) that inherits this process's stdout pipe, then exit
//!   0 *immediately*. This is the `FinCal` #29 shape (#156): the foreground command
//!   exits clean in moments, but the orphaned grandchild keeps the stdout
//!   write-end open, so an unbounded output drain would block forever even though
//!   the exit status is already in hand. The gate must tree-kill before
//!   collecting, capture the marker (EOF was reached), and report exit 0.
//! - `sleep-with-grandchild` — spawn a `sleep` grandchild that inherits stdout,
//!   then sleep ~60s. The direct child outlives the deadline, so the gate's
//!   timeout path must kill the whole tree (a plain child-kill would leave the
//!   grandchild holding the pipe).
//! - `sleep` — sleep ~60s; used both directly and as the spawned grandchild.

#![expect(
    clippy::print_stdout,
    clippy::print_stderr,
    reason = "a test helper child: the parent test reads what it prints"
)]

use std::time::Duration;

fn main() {
    let mode = std::env::args().nth(1).unwrap_or_default();
    match mode.as_str() {
        "exit-leaking-grandchild" => {
            // The marker the gate must capture: it only arrives if the reader
            // reaches EOF, which only the pre-collect tree-kill can force while
            // the orphan below holds the pipe.
            println!("foreground-marker-before-leak");
            // A test helper child has nowhere to report a failed flush.
            match std::io::Write::flush(&mut std::io::stdout()) {
                Ok(()) | Err(_) => {}
            }
            // Grandchild inherits our stdout (Stdio::inherit is the default), then
            // we exit 0 straight away — the orphan holds the pipe open.
            if let Ok(exe) = std::env::current_exe() {
                // The grandchild only holds the pipe, and a failed spawn fails the test.
                match std::process::Command::new(exe).arg("sleep").spawn() {
                    Ok(_) | Err(_) => {}
                }
            }
        }
        "sleep-with-grandchild" => {
            if let Ok(exe) = std::env::current_exe() {
                // The grandchild only holds the pipe, and a failed spawn fails the test.
                match std::process::Command::new(exe).arg("sleep").spawn() {
                    Ok(_) | Err(_) => {}
                }
            }
            std::thread::sleep(Duration::from_secs(60));
        }
        "sleep" => {
            std::thread::sleep(Duration::from_secs(60));
        }
        other => {
            eprintln!("unknown mode: {other:?}");
            #[expect(
                clippy::disallowed_methods,
                reason = "a test helper child reports an unknown mode by its exit code"
            )]
            std::process::exit(2);
        }
    }
}
