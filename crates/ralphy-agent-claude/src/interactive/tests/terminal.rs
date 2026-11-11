use super::*;
use crate::cleanup::remove_scratch;

#[test]
fn scan_dsr_request_detects_split_sequence() {
    // Sequence split across two chunks: first call must return false, second true.
    let mut carry = Vec::new();
    assert!(
        !scan_dsr_request(&mut carry, b"\x1b["),
        "partial prefix should not fire"
    );
    assert!(
        scan_dsr_request(&mut carry, b"6n"),
        "completing the sequence should fire"
    );

    // Unsplit: a single chunk containing the full sequence fires immediately.
    let mut carry2 = Vec::new();
    assert!(
        scan_dsr_request(&mut carry2, CURSOR_POSITION_REQUEST),
        "full sequence in one chunk should fire"
    );

    // No sequence at all: never fires.
    let mut carry3 = Vec::new();
    assert!(
        !scan_dsr_request(&mut carry3, b"hello world"),
        "unrelated bytes should not fire"
    );
}

/// Raw PTY bytes of a logged-out interactive session (CLI v2.1.198),
/// captured on Windows `ConPTY`: the REPL renders with a
/// `Not logged in · Run /login` status line whose words are separated by
/// cursor-forward escapes, not spaces.
const LOGIN_TUI_FIXTURE: &[u8] = include_bytes!("../../../tests/fixtures/login_tui_exec.log");

#[test]
fn login_tui_fixture_detected() {
    assert!(is_login_tui_output(LOGIN_TUI_FIXTURE));
}

#[test]
fn normal_pty_output_not_detected() {
    // ANSI-heavy healthy-session shapes: a working REPL status line and
    // agent prose that mentions login without the logged-out signature.
    let healthy = b"\x1b[38;2;153;153;153m\x1b[17;3H?\x1b[1Cfor\x1b[1Cshortcuts\
            \x1b[18;83H\x1b[1Chigh\x1b[1C\xc2\xb7\x1b[1C/effort\x1b[m\r\n\
            Running\x1b[1Ccargo\x1b[1Ctest...\r\n";
    assert!(!is_login_tui_output(healthy));
    assert!(!is_login_tui_output(
        b"the user is logged in \xc2\xb7 no action"
    ));
}

#[test]
fn strip_pty_escapes_turns_cursor_moves_into_spaces() {
    // The fixture's exact word-separation shape: `ESC[1C` between words.
    let raw = b"\x1b[38;2;255;107;128mNot\x1b[1Clogged\x1b[1Cin\x1b[1C\xc2\xb7\
        \x1b[1CRun\x1b[1C/login\x1b[38;2;153;153;153m";
    assert_eq!(strip_pty_escapes(raw), "Not logged in \u{b7} Run /login");
}

#[test]
fn strip_pty_escapes_drops_osc_and_csi() {
    let raw = b"\x1b]0;claude\x07plain\x1b[2mtext\x1b[m";
    assert_eq!(strip_pty_escapes(raw), "plaintext");
}

#[test]
fn strip_pty_escapes_ends_an_osc_at_the_string_terminator() {
    let raw = b"a\x1b]0;window title\x1b\\b";
    assert_eq!(strip_pty_escapes(raw), "ab");
}

#[test]
fn strip_pty_escapes_turns_cursor_position_into_a_space() {
    assert_eq!(strip_pty_escapes(b"a\x1b[3;4Hb"), "a b");
    assert_eq!(strip_pty_escapes(b"a\x1b[3;4fb"), "a b");
}

#[test]
fn strip_pty_escapes_drops_a_two_byte_escape() {
    assert_eq!(strip_pty_escapes(b"a\x1bcb\x1b=c"), "abc");
}

#[test]
fn strip_pty_escapes_ignores_a_trailing_lone_escape() {
    assert_eq!(strip_pty_escapes(b"ab\x1b"), "ab");
}

#[test]
fn strip_pty_escapes_turns_each_cr_and_lf_into_a_space() {
    assert_eq!(strip_pty_escapes(b"a\r\nb\nc"), "a  b c");
}

/// Live end-to-end proof for issue #72: spawn a real logged-out `claude`
/// in a PTY (isolated `CLAUDE_CONFIG_DIR`, onboarding pre-completed,
/// workspace pre-trusted, no credentials) and assert the watch flags it on
/// the same poll cadence `drive_session` uses. Needs the `claude` binary
/// and ~15s, so it is opt-in: `cargo test -p ralphy-agent-claude -- --ignored`.
#[test]
#[ignore = "spawns the real claude CLI; run manually with -- --ignored"]
fn live_logged_out_interactive_session_is_detected() {
    use std::io::Read as _;
    use std::sync::mpsc;

    let base = std::env::temp_dir().join(format!("ralphy-login-e2e-{}", std::process::id()));
    let cfg_dir = base.join("cfg");
    let work_dir = base.join("ws");
    std::fs::create_dir_all(&cfg_dir).unwrap();
    std::fs::create_dir_all(&work_dir).unwrap();
    let key = work_dir.to_string_lossy().replace('\\', "/");
    std::fs::write(
        cfg_dir.join(".claude.json"),
        serde_json::json!({
            "hasCompletedOnboarding": true,
            "theme": "dark",
            "projects": { key: { "hasTrustDialogAccepted": true } },
        })
        .to_string(),
    )
    .unwrap();

    let cmd = PtyCommand::new(resolve_claude_binary())
        .cwd(&work_dir)
        .env("CLAUDE_CONFIG_DIR", cfg_dir.as_os_str())
        .size(30, 100);
    let mut session = PtySession::spawn(cmd).expect("spawning claude");
    let mut reader = session.reader().unwrap();
    let (tx, rx) = mpsc::channel::<Vec<u8>>();
    thread::spawn(move || {
        let mut buf = [0u8; 4096];
        while let Ok(n) = reader.read(&mut buf) {
            if n == 0 || tx.send(buf[..n].to_vec()).is_err() {
                break;
            }
        }
    });

    let mut watch = LoginTuiWatch::new();
    let mut dsr_carry: Vec<u8> = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(20);
    let detected = loop {
        while let Ok(chunk) = rx.try_recv() {
            if scan_dsr_request(&mut dsr_carry, &chunk) {
                // A closed terminal ends the wait at the deadline.
                if let Err(e) = session.write_all(CURSOR_POSITION_REPLY) {
                    tracing::debug!("cursor reply not written: {e}");
                }
            }
            watch.feed(&chunk);
        }
        if watch.detected() {
            break true;
        }
        if Instant::now() >= deadline {
            break false;
        }
        thread::sleep(Duration::from_millis(500));
    };
    // The child may have exited already.
    if let Err(e) = session.kill() {
        tracing::debug!("kill of the login child failed: {e}");
    }
    remove_scratch(&base);
    assert!(
        detected,
        "a logged-out interactive claude session must be flagged as an auth failure"
    );
}

#[test]
fn login_watch_detects_across_chunks_and_disarms_on_transcript() {
    // The signature arrives split across PTY chunks.
    let mut watch = LoginTuiWatch::new();
    let mid = LOGIN_TUI_FIXTURE.len() / 2;
    watch.feed(&LOGIN_TUI_FIXTURE[..mid]);
    watch.feed(&LOGIN_TUI_FIXTURE[mid..]);
    assert!(watch.detected());

    // Once the transcript shows activity the watch must stay quiet even if
    // the signature bytes appear again (agent echoing this source).
    let mut watch = LoginTuiWatch::new();
    watch.disarm();
    watch.feed(LOGIN_TUI_FIXTURE);
    assert!(!watch.detected());
}
