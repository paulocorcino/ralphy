//! Test helper child driven by `tests/session_roundtrip.rs` (and, via the WS
//! route override, `tests/session_ws.rs`) through `CARGO_BIN_EXE_*`. It stands in
//! for a real agent CLI so the session manager and its transport can be exercised
//! against a live PTY child with no installed/logged-in agent — portable on
//! Windows and Unix, no shell-script children (house convention).
//!
//! Behavior:
//! - On start: print `CWD:<current_dir>` then `READY` — the first proves the child
//!   was spawned in the session's working directory.
//! - A 50ms poll thread reads [`terminal_size::terminal_size`] and prints
//!   `SIZE <cols>x<rows>` on the first read and on every change, so a PTY resize
//!   is observable from the captured output.
//! - The main loop reads stdin lines: `quit` exits 0; `spawn-grandchild` spawns a
//!   copy of itself in `sleep` mode inheriting this stdout (the pipe write-end
//!   stays open after the direct child dies, so only a process-tree kill reaches
//!   EOF); `env <NAME>` prints `ENV:<NAME>=<value>` — the only way a test can
//!   observe the environment the launcher actually gave the child; any other line
//!   echoes as `GOT:<line>`.
//! - `sleep` mode sleeps ~60s — the grandchild that holds stdout open.

use std::io::{BufRead, Write};
use std::time::Duration;

/// Prefix of the startup line carrying the child's working directory.
pub const CWD_MARKER: &str = "CWD:";
/// Prefix of the line echoing a received stdin line.
pub const GOT_MARKER: &str = "GOT:";
/// Prefix of the line reporting one environment variable (`ENV:<NAME>=<value>`).
pub const ENV_MARKER: &str = "ENV:";
/// Prefix of the line reporting the current terminal size (`SIZE <cols>x<rows>`).
pub const SIZE_MARKER: &str = "SIZE";

fn main() {
    let mode = std::env::args().nth(1).unwrap_or_default();
    if mode == "sleep" {
        std::thread::sleep(Duration::from_secs(60));
        return;
    }

    let cwd = std::env::current_dir()
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    println!("{CWD_MARKER}{cwd}");
    println!("READY");
    let _ = std::io::stdout().flush();

    // Poll the terminal size every 50ms and print it on change, so a resize made
    // through the PTY master shows up in the captured stream.
    std::thread::spawn(|| {
        let mut last: Option<(u16, u16)> = None;
        loop {
            if let Some((terminal_size::Width(cols), terminal_size::Height(rows))) =
                terminal_size::terminal_size()
            {
                if last != Some((cols, rows)) {
                    last = Some((cols, rows));
                    println!("{SIZE_MARKER} {cols}x{rows}");
                    let _ = std::io::stdout().flush();
                }
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    });

    let stdin = std::io::stdin();
    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => break,
        };
        let line = line.trim_end_matches(['\r', '\n']);
        match line {
            "quit" => std::process::exit(0),
            "spawn-grandchild" => {
                // A grandchild inheriting our stdout keeps the PTY slave open after
                // we die, so a plain direct-child kill would leave the reader
                // blocked — only a tree kill reaches EOF.
                if let Ok(exe) = std::env::current_exe() {
                    let _ = std::process::Command::new(exe).arg("sleep").spawn();
                }
            }
            other if other.starts_with("env ") => {
                let name = other["env ".len()..].trim();
                // An unset variable prints an EMPTY value rather than nothing, so
                // a test can tell "not set" from "the child never answered".
                let value = std::env::var(name).unwrap_or_default();
                println!("{ENV_MARKER}{name}={value}");
                let _ = std::io::stdout().flush();
            }
            other => {
                println!("{GOT_MARKER}{other}");
                let _ = std::io::stdout().flush();
            }
        }
    }
}
