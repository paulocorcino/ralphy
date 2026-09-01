//! The terminal a session gives its child is the DAEMON's declaration, not the
//! launching shell's leftovers.
//!
//! This is a real bug's regression test: a daemon started from an agent's shell
//! tool inherits `NO_COLOR=1`, `portable-pty` copies the whole parent
//! environment into the child, and every workbench console came out monochrome —
//! measured on claude, codex, gemini and copilot, which all honour it.
//!
//! Its own test binary on purpose: it mutates the process environment (the only
//! way to have something to inherit), so it must not race a sibling test that
//! reads it.

use std::path::PathBuf;
use std::time::Duration;

use ralphy_daemon::session::{Session, SessionSpec};
use ralphy_pty::{CURSOR_POSITION_REPLY, CURSOR_POSITION_REQUEST};
use tokio::sync::mpsc::UnboundedReceiver;

/// Accumulate output until it contains `needle`, bounded to 5s. Plays the
/// terminal emulator, answering the ConPTY startup cursor-position request —
/// without it the child blocks before running on Windows.
async fn read_until(
    session: &mut Session,
    rx: &mut UnboundedReceiver<Vec<u8>>,
    needle: &str,
) -> String {
    let mut acc = String::new();
    let res = tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(chunk) = rx.recv().await {
            if chunk
                .windows(CURSOR_POSITION_REQUEST.len())
                .any(|w| w == CURSOR_POSITION_REQUEST)
            {
                let _ = session.write(CURSOR_POSITION_REPLY);
            }
            acc.push_str(&String::from_utf8_lossy(&chunk));
            if acc.contains(needle) {
                break;
            }
        }
    })
    .await;
    assert!(
        res.is_ok(),
        "timed out (5s) waiting for {needle:?}; got so far:\n{acc}"
    );
    acc
}

/// Ask the helper child to report one variable and return the reported value.
async fn child_env(
    session: &mut Session,
    rx: &mut UnboundedReceiver<Vec<u8>>,
    name: &str,
) -> String {
    session
        .write(format!("env {name}\r").as_bytes())
        .expect("writing to the PTY");
    let out = read_until(session, rx, &format!("ENV:{name}=")).await;
    let marker = format!("ENV:{name}=");
    let tail = out.rsplit(&marker).next().unwrap_or_default();
    tail.lines().next().unwrap_or_default().trim().to_string()
}

fn spec_at(cwd: PathBuf, env: Vec<(std::ffi::OsString, std::ffi::OsString)>) -> SessionSpec {
    SessionSpec {
        program: env!("CARGO_BIN_EXE_session_test_child").into(),
        args: Vec::new(),
        cwd,
        rows: 24,
        cols: 80,
        env,
        name: None,
    }
}

#[tokio::test]
async fn the_child_gets_the_daemons_terminal_not_the_launching_shells_suppressors() {
    // What a daemon launched from an agent's shell tool actually carries.
    std::env::set_var("NO_COLOR", "1");
    std::env::set_var("FORCE_COLOR", "0");
    std::env::set_var("TERM", "dumb");

    let dir = tempfile::tempdir().unwrap();
    // A vendor's own env still wins: it is applied after the terminal defaults.
    let mut session = Session::spawn(spec_at(
        dir.path().to_path_buf(),
        vec![("TERM".into(), "vendor-term".into())],
    ))
    .unwrap();
    let mut rx = session.take_output();
    read_until(&mut session, &mut rx, "READY").await;

    let no_color = child_env(&mut session, &mut rx, "NO_COLOR").await;
    assert!(
        no_color.is_empty(),
        "NO_COLOR must not reach the child; got {no_color:?}"
    );
    let force_color = child_env(&mut session, &mut rx, "FORCE_COLOR").await;
    assert!(
        force_color.is_empty(),
        "FORCE_COLOR must not reach the child; got {force_color:?}"
    );
    let colorterm = child_env(&mut session, &mut rx, "COLORTERM").await;
    assert_eq!(
        colorterm, "truecolor",
        "the daemon declares its own terminal"
    );
    let term = child_env(&mut session, &mut rx, "TERM").await;
    assert_eq!(
        term, "vendor-term",
        "a spec's own env overrides the terminal default"
    );

    session.close();
}
