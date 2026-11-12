use super::*;

mod charter;
mod terminal;

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
        None,
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
        None,
    ));
    assert!(!unset.iter().any(|arg| arg == "--effort"));
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
        Some("{\"type\":\"system\"}".to_owned())
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
