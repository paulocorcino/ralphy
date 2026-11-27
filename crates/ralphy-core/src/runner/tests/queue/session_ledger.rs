//! The execute ledger line carries the last non-empty vendor session of the
//! resume loop.

use super::*;

/// Run a two-attempt resume loop whose attempts report `sessions`, and return the
/// execute line the ledger recorded.
fn execute_line_after_resume(stamp: &str, sessions: [Option<String>; 2]) -> String {
    let root = test_dir(stamp);
    let reset = ResetAt {
        text: "09:00".to_owned(),
        at: None,
    };
    let agent = ScriptedAgent::new(vec![Outcome::Limit(Some(reset)), Outcome::Done])
        .with_sessions(sessions.into());

    let (_, sink) = run_on_fake(
        &test_cfg(&root, stamp),
        &[test_issue(78)],
        &agent,
        &RecordingTracker::default(),
    );

    let content = ledger_lines(&sink).join("\n");
    let line = content
        .lines()
        .find(|l| l.contains("\"phase\":\"execute\""))
        .ok_or_else(|| format!("no execute line: {content}"))
        .expect("this test needs the value")
        .to_owned();

    remove_scratch(&root);
    line
}

#[test]
fn the_last_non_empty_session_of_the_resume_loop_reaches_the_ledger() {
    // The first attempt hits a limit and names its session; the resumed one
    // reports none.
    let line = execute_line_after_resume(
        "stamp-session-ledger",
        [Some("sess-first".to_owned()), None],
    );
    assert!(
        line.contains("\"session_id\":\"sess-first\""),
        "the execute line keeps the earlier session: {line}"
    );
}

#[test]
fn an_empty_session_of_the_resume_loop_does_not_replace_the_earlier_one() {
    let line = execute_line_after_resume(
        "stamp-session-ledger-empty",
        [Some("sess-first".to_owned()), Some(String::new())],
    );
    assert!(
        line.contains("\"session_id\":\"sess-first\""),
        "the execute line keeps the earlier session: {line}"
    );
}
