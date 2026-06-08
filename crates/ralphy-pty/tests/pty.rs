//! Acceptance tests for the shared PTY crate, exercised against an interactive
//! system shell (`cmd.exe` on Windows, `sh` elsewhere). They cover the three
//! capabilities the issue calls out: capture TTY output, write input to the
//! child, and kill+wait the process tree.
//!
//! The shared driver below plays the terminal: it drains the master on a
//! background thread and answers the shell's start-up cursor-position query
//! ([`CURSOR_POSITION_REQUEST`]), without which the child blocks before running.

use std::io::Read;
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

use ralphy_pty::{PtyCommand, PtySession, CURSOR_POSITION_REPLY, CURSOR_POSITION_REQUEST};

/// Spawn the platform's interactive shell inside a PTY.
fn shell() -> PtyCommand {
    #[cfg(windows)]
    {
        PtyCommand::new("cmd.exe")
    }
    #[cfg(not(windows))]
    {
        PtyCommand::new("sh")
    }
}

/// Index of the first occurrence of `needle` in `haystack`.
fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// Start draining a session's master output on a background thread, returning a
/// channel of chunks.
fn spawn_drain(session: &PtySession) -> Receiver<Vec<u8>> {
    let mut reader = session.reader().expect("clone reader");
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut buf = [0u8; 4096];
        while let Ok(n) = reader.read(&mut buf) {
            if n == 0 || tx.send(buf[..n].to_vec()).is_err() {
                break;
            }
        }
    });
    rx
}

/// Pump the session until the child exits (or `timeout` elapses), answering
/// cursor-position queries along the way. Returns the captured output and
/// whether the child exited on its own.
fn pump_until_exit(
    session: &mut PtySession,
    rx: &Receiver<Vec<u8>>,
    timeout: Duration,
) -> (String, bool) {
    let mut out = String::new();
    let start = Instant::now();
    let mut exited = false;
    while start.elapsed() < timeout {
        while let Ok(chunk) = rx.try_recv() {
            if find_subslice(&chunk, CURSOR_POSITION_REQUEST).is_some() {
                session
                    .write_all(CURSOR_POSITION_REPLY)
                    .expect("answer DSR");
            }
            out.push_str(&String::from_utf8_lossy(&chunk));
        }
        if session.try_wait().expect("poll child").is_some() {
            exited = true;
            break;
        }
        thread::sleep(Duration::from_millis(20));
    }
    while let Ok(chunk) = rx.try_recv() {
        out.push_str(&String::from_utf8_lossy(&chunk));
    }
    (out, exited)
}

#[test]
fn captures_output_from_input_written_to_the_child() {
    let mut session = PtySession::spawn(shell()).expect("spawn shell in PTY");
    let rx = spawn_drain(&session);

    // Type a line the shell will echo back to its terminal, then exit.
    session
        .write_all(b"echo ralphy-pty-marker\r\n")
        .expect("write echo");
    session.write_all(b"exit\r\n").expect("write exit");

    let (rendered, exited) = pump_until_exit(&mut session, &rx, Duration::from_secs(15));
    assert!(exited, "shell should exit after the `exit` command");

    let exit = session.wait().expect("wait on child");
    assert!(
        rendered.contains("ralphy-pty-marker"),
        "expected the echoed marker in captured output, got:\n{rendered}"
    );
    assert!(
        exit.success,
        "interactive shell should exit cleanly: {exit:?}"
    );
}

#[test]
fn kills_and_waits_the_process_tree() {
    // An interactive shell with no `exit` sent runs until we kill it.
    let mut session = PtySession::spawn(shell()).expect("spawn shell in PTY");
    let rx = spawn_drain(&session);

    // Let the shell start up (answering its cursor query), then confirm it is
    // still running.
    let (_warmup, exited) = pump_until_exit(&mut session, &rx, Duration::from_millis(500));
    assert!(!exited, "shell should still be running before kill");

    session.kill().expect("kill child");
    let exit = session.wait().expect("wait after kill");
    assert!(
        !exit.success,
        "a killed shell should not report success: {exit:?}"
    );
}
