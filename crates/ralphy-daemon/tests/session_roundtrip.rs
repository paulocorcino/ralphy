//! The session manager's verifying spine (docs/adr/0032 §2): drive the
//! `session_test_child` helper through a real PTY and the #161 codec, asserting
//! the four behaviors the issue's Done-when names — spawn-in-cwd, stdin echo,
//! resize propagation, and process-tree teardown on close. No socket: the WS
//! bridge is a later step; this proves the transport-free core first.

use std::path::PathBuf;
use std::time::Duration;

use ralphy_daemon::protocol::{self, Frame};
use ralphy_daemon::session::{Session, SessionSpec};
use ralphy_pty::{CURSOR_POSITION_REPLY, CURSOR_POSITION_REQUEST};
use tokio::sync::mpsc::UnboundedReceiver;

/// Round-trip `data` through `encode`/`decode` as a `Terminal` frame — the same
/// codec the WS bridge uses — and return the payload. Proves the bytes survive
/// the wire shape in both directions.
fn through_codec(data: Vec<u8>) -> Vec<u8> {
    let framed = protocol::encode(&Frame::Terminal { session: 1, data });
    match protocol::decode(&framed).expect("terminal frame decodes") {
        Frame::Terminal { data, .. } => data,
        other => panic!("expected a terminal frame, got {other:?}"),
    }
}

/// Accumulate decoded output until it contains `needle`, bounded to 5s so a
/// regression fails instead of hanging. Returns the accumulated text.
///
/// Plays the terminal emulator (the role xterm.js fills in production): the
/// session manager is a transparent byte pipe by design, so this loop answers
/// the ConPTY startup cursor-position request (`ESC[6n`) — otherwise the child
/// blocks before it runs on Windows.
async fn read_until(
    session: &mut Session,
    rx: &mut UnboundedReceiver<Vec<u8>>,
    needle: &str,
) -> String {
    let mut acc = String::new();
    let res = tokio::time::timeout(Duration::from_secs(5), async {
        // Loop ends on EOF (`None`) or once the needle appears.
        while let Some(chunk) = rx.recv().await {
            let data = through_codec(chunk);
            if data
                .windows(CURSOR_POSITION_REQUEST.len())
                .any(|w| w == CURSOR_POSITION_REQUEST)
            {
                let _ = session.write(CURSOR_POSITION_REPLY);
            }
            acc.push_str(&String::from_utf8_lossy(&data));
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

fn spec_at(cwd: PathBuf) -> SessionSpec {
    SessionSpec {
        program: env!("CARGO_BIN_EXE_session_test_child").into(),
        args: Vec::new(),
        cwd,
        rows: 24,
        cols: 80,
    }
}

#[tokio::test]
async fn stdin_stdout_and_resize_round_trip_through_codec_and_pty() {
    let dir = tempfile::tempdir().unwrap();
    let unique = dir
        .path()
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();

    let mut session = Session::spawn(spec_at(dir.path().to_path_buf())).unwrap();
    let mut rx = session.take_output();

    // (a) launch-in-repo-dir: the startup CWD line names the spawn directory.
    let startup = read_until(&mut session, &mut rx, "CWD:").await;
    let cwd_line = startup
        .lines()
        .find(|l| l.contains("CWD:"))
        .expect("a CWD: line");
    assert!(
        cwd_line.contains(&unique),
        "child cwd must be the spawn dir (unique {unique:?}); got {cwd_line:?}"
    );

    // (b) stdin: a written line echoes back through the PTY as GOT:<line>.
    session
        .write(&through_codec(b"hello-stdin\r".to_vec()))
        .unwrap();
    read_until(&mut session, &mut rx, "GOT:hello-stdin").await;

    // (c) resize: SIZE reports cols x rows; after resize(40, 100) → SIZE 100x40.
    session.resize(40, 100).unwrap();
    read_until(&mut session, &mut rx, "SIZE 100x40").await;
}

#[tokio::test]
async fn close_terminates_the_child_process_tree() {
    let dir = tempfile::tempdir().unwrap();
    let mut session = Session::spawn(spec_at(dir.path().to_path_buf())).unwrap();
    let mut rx = session.take_output();

    // Wait until the child is running, then spawn a grandchild that inherits the
    // PTY slave. The `ping` after it is processed in stdin order, so GOT:ping
    // proves the grandchild already spawned — no timing guess.
    read_until(&mut session, &mut rx, "READY").await;
    session
        .write(&through_codec(b"spawn-grandchild\r".to_vec()))
        .unwrap();
    session.write(&through_codec(b"ping\r".to_vec())).unwrap();
    read_until(&mut session, &mut rx, "GOT:ping").await;

    // A plain direct-child kill would leave the grandchild holding stdout open,
    // so the reader would never reach EOF; close() tree-kills, so rx yields None.
    session.close();
    let reached_eof = tokio::time::timeout(Duration::from_secs(5), async {
        while rx.recv().await.is_some() {}
    })
    .await;
    assert!(
        reached_eof.is_ok(),
        "output channel must reach EOF within 5s after close() (process-tree kill)"
    );
}
