use super::*;
use crate::cleanup::remove_scratch;

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
                #[expect(
                    clippy::let_underscore_must_use,
                    reason = "a closed terminal ends the wait at the deadline"
                )]
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
    #[expect(
        clippy::let_underscore_must_use,
        reason = "the child may have exited already"
    )]
    let _ = session.kill();
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

#[test]
fn classify_exit_reads_the_flag_file_and_the_limit_transcript() {
    let dir = tempfile::tempdir().unwrap();
    let flag = dir.path().join("status.flag");
    let limit = r#"{"type":"rate_limit_event","rate_limit_info":{"status":"rejected"}}"#;
    let since = SystemTime::now();

    // No flag: the limit transcript of the live loop decides.
    let outcome = classify_exit(SessionExit::Limit(limit.into()), &flag, None, since).unwrap();
    assert_eq!(outcome, Outcome::Limit(None));

    // A flag from the Stop hook is read at the end of the session.
    std::fs::write(&flag, "BLOCKED needs a key").unwrap();
    let outcome = classify_exit(SessionExit::Flag, &flag, None, since).unwrap();
    assert_eq!(outcome, Outcome::Blocked("needs a key".into()));

    // With a flag, the transcript is not consulted.
    let outcome = classify_exit(SessionExit::Limit(limit.into()), &flag, None, since).unwrap();
    assert_eq!(outcome, Outcome::Blocked("needs a key".into()));
}

/// A transcript directory holding one fresh transcript with `text`.
fn transcript_with(text: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("session.jsonl"), text).unwrap();
    dir
}

#[test]
fn only_an_ended_session_has_a_transcript_to_read() {
    let dir = transcript_with("{\"type\":\"system\"}");
    let since = SystemTime::UNIX_EPOCH;
    assert_eq!(ended_transcript(false, Some(dir.path()), since), None);
    assert_eq!(
        ended_transcript(true, Some(dir.path()), since),
        Some("{\"type\":\"system\"}".to_string())
    );
}

#[test]
fn a_child_that_exits_on_a_logged_out_transcript_is_an_auth_error() {
    let logged_out = transcript_with(
        "Not logged in · Please run /login
",
    );
    let flag = logged_out.path().join("status.flag");
    let since = SystemTime::UNIX_EPOCH;
    let error = classify_exit(
        SessionExit::ChildExited,
        &flag,
        Some(logged_out.path()),
        since,
    )
    .unwrap_err();
    assert_eq!(error.to_string(), CLAUDE_AUTH_ERROR_MSG);

    // Control: the same child exit on a healthy transcript is no auth error.
    let healthy = transcript_with(
        "{\"type\":\"system\"}
",
    );
    let flag = healthy.path().join("status.flag");
    let outcome =
        classify_exit(SessionExit::ChildExited, &flag, Some(healthy.path()), since).unwrap();
    assert_eq!(outcome, Outcome::Stuck);
}

#[test]
fn a_timed_out_session_reads_its_transcript_for_a_limit() {
    let limit = r#"{"type":"rate_limit_event","rate_limit_info":{"status":"rejected"}}"#;
    let limited = transcript_with(limit);
    let flag = limited.path().join("status.flag");
    let since = SystemTime::UNIX_EPOCH;
    let outcome = classify_exit(SessionExit::TimedOut, &flag, Some(limited.path()), since).unwrap();
    assert_eq!(outcome, Outcome::Limit(None));

    // Control: without a limit in the transcript the same exit is a timeout.
    let quiet = transcript_with(
        "{\"type\":\"system\"}
",
    );
    let flag = quiet.path().join("status.flag");
    let outcome = classify_exit(SessionExit::TimedOut, &flag, Some(quiet.path()), since).unwrap();
    assert_eq!(outcome, Outcome::Timeout);
}

#[test]
fn the_interactive_argv_carries_no_prompt() {
    for remote_control in [false, true] {
        let args = interactive_args(
            Path::new("settings.json"),
            Path::new("plugin"),
            "sonnet",
            Some("high"),
            remote_control,
            "ralphy-1",
        );
        assert!(!args.iter().any(|arg| arg == EXEC_CHARTER));
    }
}

#[test]
fn the_charter_is_typed_as_measured() {
    let keys = charter_keys();
    assert_eq!(
        keys.text,
        b"Read .ralphy/exec.md and follow it exactly to implement .ralphy/plan.md for this issue. Emit RALPHY_DONE_EXIT when finished."
    );
    assert_eq!(keys.submit, b"\r");
    assert!(keys.pause >= Duration::from_millis(100));
    assert_eq!(keys.pause, Duration::from_millis(300));
}

#[test]
fn the_gate_types_the_charter_once_after_the_first_hook_event() {
    let start = Instant::now();
    let mut gate = ProofGate::new(start, Duration::from_secs(30));
    assert_eq!(gate.poll(false, start), GateAction::Wait);
    assert_eq!(
        gate.poll(false, start + Duration::from_secs(29)),
        GateAction::Wait
    );
    assert_eq!(
        gate.poll(true, start + Duration::from_secs(29)),
        GateAction::Type
    );
    for later in [29, 30, 60] {
        assert_eq!(
            gate.poll(true, start + Duration::from_secs(later)),
            GateAction::Wait
        );
        assert_eq!(
            gate.poll(false, start + Duration::from_secs(later)),
            GateAction::Wait
        );
    }
}

#[test]
fn the_gate_refuses_a_session_with_no_hook_event() {
    let start = Instant::now();
    let mut gate = ProofGate::new(start, Duration::from_secs(30));
    assert_eq!(
        gate.poll(false, start + Duration::from_secs(30)),
        GateAction::Refuse
    );
    for later in [30, 31, 90] {
        assert_ne!(
            gate.poll(true, start + Duration::from_secs(later)),
            GateAction::Type
        );
    }
}

#[test]
fn an_unproven_interactive_session_types_nothing() {
    let dir = tempfile::tempdir().expect("temp dir");
    let status = dir.path().join("agent-status.jsonl");
    std::fs::write(&status, "{\"event\":\"PreToolUse\"}\n").expect("status file");
    let proof = ProofWait::new(
        status,
        0,
        dir.path().join("settings.json"),
        dir.path().to_path_buf(),
        Duration::from_secs(1),
    );
    let agent = ClaudeAgent::new(None, None, dir.path().to_path_buf());
    let (_tx, rx) = mpsc::channel();
    let flag = dir.path().join("status.flag");
    let mut pump = SessionPump::new(&agent, rx, &flag, None, SystemTime::now(), proof);
    let mut typed: Vec<u8> = Vec::new();
    let give_up = Instant::now() + Duration::from_secs(10);
    let err = loop {
        let mut keys = |bytes: &[u8]| -> Result<()> {
            typed.extend_from_slice(bytes);
            Ok(())
        };
        if let Err(e) = pump.advance_proof(Instant::now(), &mut keys) {
            break e;
        }
        assert!(Instant::now() < give_up, "the pump never refused");
        thread::sleep(Duration::from_millis(100));
    };
    assert!(err.downcast_ref::<ralphy_core::HooksUnproven>().is_some());
    assert!(typed.is_empty(), "typed {} bytes", typed.len());
}

#[test]
fn the_waiver_types_the_charter_of_an_unproven_interactive_session() {
    let dir = tempfile::tempdir().expect("temp dir");
    let status = dir.path().join("agent-status.jsonl");
    std::fs::write(&status, "{\"event\":\"PreToolUse\"}\n").expect("status file");
    let mut proof = ProofWait::new(
        status,
        0,
        dir.path().join("settings.json"),
        dir.path().to_path_buf(),
        Duration::from_millis(200),
    );
    proof.waiver = true;
    let mut typed: Vec<u8> = Vec::new();
    let give_up = Instant::now() + Duration::from_secs(10);
    while typed.is_empty() {
        let mut keys = |bytes: &[u8]| -> Result<()> {
            typed.extend_from_slice(bytes);
            Ok(())
        };
        proof
            .advance(Instant::now(), &mut keys)
            .expect("the waiver lets the session go on");
        assert!(Instant::now() < give_up, "the charter was never typed");
        thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(typed, charter_keys().text);
}
