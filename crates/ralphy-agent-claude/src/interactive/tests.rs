use super::*;

#[test]
fn interactive_command_maps_high_and_omits_unset() {
    let strings = |args: Vec<std::ffi::OsString>| {
        args.into_iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
    };
    let set = strings(interactive_args(
        Path::new("settings.json"),
        Path::new("plugin"),
        "sonnet",
        Some("high"),
        false,
        "ralphy-1",
    ));
    assert_eq!(
        set.windows(2)
            .filter(|pair| pair == &["--effort", "high"])
            .count(),
        1
    );
    let unset = strings(interactive_args(
        Path::new("settings.json"),
        Path::new("plugin"),
        "sonnet",
        None,
        false,
        "ralphy-1",
    ));
    assert!(!unset.iter().any(|arg| arg == "--effort"));
}

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

#[test]
fn respawn_budget_is_exactly_one() {
    // A terminal outcome settles immediately, untouched.
    let mut respawned = false;
    assert!(matches!(
        respawn_step(DriveEnd::Outcome(Outcome::Done), &mut respawned),
        RespawnStep::Done(Outcome::Done)
    ));
    assert!(!respawned);

    // First Respawn asks for one more child and spends the budget…
    let mut respawned = false;
    assert!(matches!(
        respawn_step(DriveEnd::Respawn, &mut respawned),
        RespawnStep::Again
    ));
    assert!(respawned);
    // …a second Respawn settles on Timeout (budget = 1).
    assert!(matches!(
        respawn_step(DriveEnd::Respawn, &mut respawned),
        RespawnStep::Done(Outcome::Timeout)
    ));
}

#[test]
fn workspace_trust_sets_flag_and_preserves_other_content() {
    use serde_json::json;

    // Existing config with an unrelated project and a top-level key.
    let root = json!({
        "numStartups": 7,
        "projects": { "C:/other": { "hasTrustDialogAccepted": false, "keep": 1 } }
    });
    let out = with_workspace_trusted(root, "C:/ws");

    // The new workspace is trusted...
    assert_eq!(out["projects"]["C:/ws"]["hasTrustDialogAccepted"], true);
    // ...and nothing else was disturbed.
    assert_eq!(out["numStartups"], 7);
    assert_eq!(out["projects"]["C:/other"]["hasTrustDialogAccepted"], false);
    assert_eq!(out["projects"]["C:/other"]["keep"], 1);
}

#[test]
fn workspace_trust_bootstraps_empty_config() {
    let out = with_workspace_trusted(serde_json::json!({}), "C:/ws");
    assert_eq!(out["projects"]["C:/ws"]["hasTrustDialogAccepted"], true);
}

#[test]
fn onboarding_completed_sets_flag_and_seeds_theme_once() {
    use serde_json::json;

    // No theme yet → flag set and a default theme seeded.
    let out = with_onboarding_completed(json!({ "numStartups": 7 }));
    assert_eq!(out["hasCompletedOnboarding"], true);
    assert_eq!(out["theme"], "dark");
    assert_eq!(out["numStartups"], 7);

    // An existing theme is never overwritten.
    let out = with_onboarding_completed(json!({ "theme": "light" }));
    assert_eq!(out["hasCompletedOnboarding"], true);
    assert_eq!(out["theme"], "light");
}

/// Raw PTY bytes of a logged-out interactive session (CLI v2.1.198),
/// captured on Windows ConPTY: the REPL renders with a
/// `Not logged in · Run /login` status line whose words are separated by
/// cursor-forward escapes, not spaces.
const LOGIN_TUI_FIXTURE: &[u8] = include_bytes!("../../tests/fixtures/login_tui_exec.log");

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
    let raw = b"\x1b[38;2;255;107;128mNot\x1b[1Clogged\x1b[1Cin\x1b[1C\xc2\xb7\x1b[1CRun\x1b[1C/login\x1b[38;2;153;153;153m";
    assert_eq!(strip_pty_escapes(raw), "Not logged in \u{b7} Run /login");
}

#[test]
fn strip_pty_escapes_drops_osc_and_csi() {
    let raw = b"\x1b]0;claude\x07plain\x1b[2mtext\x1b[m";
    assert_eq!(strip_pty_escapes(raw), "plaintext");
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
                let _ = session.write_all(CURSOR_POSITION_REPLY);
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
    let _ = session.kill();
    let _ = std::fs::remove_dir_all(&base);
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
