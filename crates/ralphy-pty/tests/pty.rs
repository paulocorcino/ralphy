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

/// Pump until the shell answers its start-up cursor-position query — its prompt
/// is then drawn and it is listening for input — or `cap` elapses. This replaces
/// a blind fixed sleep with a readiness signal, so input is never written into a
/// shell that hasn't started reading yet (the source of the flakiness). Falling
/// through on `cap` is a safe fallback: `pump_until_exit` also answers DSR.
fn pump_until_ready(session: &mut PtySession, rx: &Receiver<Vec<u8>>, cap: Duration) {
    let start = Instant::now();
    while start.elapsed() < cap {
        while let Ok(chunk) = rx.try_recv() {
            if find_subslice(&chunk, CURSOR_POSITION_REQUEST).is_some() {
                session
                    .write_all(CURSOR_POSITION_REPLY)
                    .expect("answer DSR");
                return;
            }
        }
        thread::sleep(Duration::from_millis(10));
    }
}

/// Pump (answering DSR) until `needle` appears in the accumulated output or
/// `timeout` elapses. Returns the captured output and whether the needle was
/// seen. Used to confirm a write reached the child *before* triggering teardown,
/// so ConPTY can't close the output between the echo and the assertion.
fn pump_until_contains(
    session: &mut PtySession,
    rx: &Receiver<Vec<u8>>,
    needle: &str,
    timeout: Duration,
) -> (String, bool) {
    let mut out = String::new();
    let start = Instant::now();
    while start.elapsed() < timeout {
        while let Ok(chunk) = rx.try_recv() {
            if find_subslice(&chunk, CURSOR_POSITION_REQUEST).is_some() {
                session
                    .write_all(CURSOR_POSITION_REPLY)
                    .expect("answer DSR");
            }
            out.push_str(&String::from_utf8_lossy(&chunk));
        }
        if out.contains(needle) {
            return (out, true);
        }
        thread::sleep(Duration::from_millis(20));
    }
    (out, false)
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

    // Wait until the shell has started up and is listening before sending input.
    pump_until_ready(&mut session, &rx, Duration::from_secs(5));
    session
        .write_all(b"echo ralphy-pty-marker\r\n")
        .expect("write echo");

    // Confirm the marker round-tripped through the PTY *before* asking the shell
    // to exit — sending `exit` in the same breath races ConPTY closing the
    // output before the echoed marker is rendered and drained.
    let (rendered, saw_marker) = pump_until_contains(
        &mut session,
        &rx,
        "ralphy-pty-marker",
        Duration::from_secs(10),
    );
    assert!(
        saw_marker,
        "expected the echoed marker in captured output, got:\n{rendered}"
    );

    session.write_all(b"exit\r\n").expect("write exit");
    let (_tail, exited) = pump_until_exit(&mut session, &rx, Duration::from_secs(15));
    assert!(exited, "shell should exit after the `exit` command");

    let exit = session.wait().expect("wait on child");
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
