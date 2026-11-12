use super::*;

#[tokio::test]
async fn a_claude_console_gets_the_status_hooks_and_reports_its_agent_state() {
    let dir = tempfile::tempdir().unwrap();
    let (port, store) = start_daemon(dir.path()).await;

    let mut presence = page::connect(port).await;

    let mut ws = page::console(
        port,
        serde_json::json!({ "repo": "owner/statelab", "agent": "claude" }),
    )
    .await;
    assert!(
        sessions_dirty(&mut presence).await,
        "sessions.dirty when the console starts"
    );

    let sessions_dir = store.join("sessions");
    let settings = sessions_dir.join("1.settings.json");
    let status = sessions_dir.join("1.agent-status.jsonl");

    assert_claude_launch(&mut ws, &settings, &status).await;
    assert_hook_set(&settings);
    assert!(
        status.is_file(),
        "the status file exists, empty, before any hook"
    );

    // Before any hook: no `agent_state` on the row at all.
    let body = http_get(port, "/api/sessions").await;
    assert!(
        body.contains("\"agent\":\"claude\"") && !body.contains("agent_state"),
        "{body}"
    );

    // "The hook" appends a waiting line; the pump's tick folds it.
    append_status(
        &status,
        concat!(
            r#"{"event":"PreToolUse","tool_name":"AskUserQuestion","#,
            r#""question":"which port?","ts":"2026-09-15T10:00:00-03:00"}"#
        ),
    );
    let row = wait_for_agent_state(port).await;
    assert!(
        sessions_dirty(&mut presence).await,
        "sessions.dirty when the agent state changes"
    );
    let rows: serde_json::Value = serde_json::from_str(&row).unwrap();
    let state = &rows[0]["agent_state"];
    assert_eq!(state["state"], "waiting", "{row}");
    assert_eq!(state["detail"], "AskUserQuestion: which port?", "{row}");
    assert_eq!(state["since"], "2026-09-15T10:00:00-03:00", "{row}");

    // The child exits → the files go with the session.
    ws.send(terminal(b"quit\r")).await.unwrap();
    wait_for_no_session(port).await;
    assert!(
        sessions_dirty(&mut presence).await,
        "sessions.dirty when the console ends"
    );
    assert!(!settings.exists(), "settings removed with the session");
    assert!(!status.exists(), "status file removed with the session");
}

/// The live child's env names the status file and its argv the settings file.
async fn assert_claude_launch(
    ws: &mut page::Channel,
    settings: &std::path::Path,
    status: &std::path::Path,
) {
    ws.send(terminal(b"env RALPHY_STATUS_FILE\r"))
        .await
        .expect("send the env request");
    ws.send(terminal(b"argv\r"))
        .await
        .expect("send the argv request");
    let want_env = flatten(&format!(
        "ENV:RALPHY_STATUS_FILE={}",
        status.to_string_lossy()
    ));
    let want_argv = flatten(&format!("ARGV:--settings {}", settings.to_string_lossy()));
    let got = read_echo(ws, |flat| {
        flat.contains(&want_env) && flat.contains(&want_argv)
    })
    .await;
    let flat = flatten(&got);
    assert!(flat.contains(&want_env), "wanted {want_env}, got:\n{got}");
    assert!(flat.contains(&want_argv), "wanted {want_argv}, got:\n{got}");
}

/// The daemon wrote the hook set: the six events, `hook status` each.
fn assert_hook_set(settings: &std::path::Path) {
    let text = std::fs::read_to_string(settings).expect("read the settings file");
    let doc: serde_json::Value = serde_json::from_str(&text).expect("parse the settings file");
    for event in [
        "SessionStart",
        "UserPromptSubmit",
        "PreToolUse",
        "PermissionRequest",
        "Stop",
        "SubagentStop",
    ] {
        // The test child answers `--version` as Claude Code 2.1.312, so the
        // form is exec; an older Claude Code gets one shell string.
        let entry = doc
            .pointer(&format!("/hooks/{event}/0/hooks/0"))
            .unwrap_or(&serde_json::Value::Null);
        let runs_hook_status = match entry["args"].as_array() {
            Some(args) => args == &[serde_json::json!("hook"), serde_json::json!("status")],
            None => entry["command"]
                .as_str()
                .is_some_and(|c| c.ends_with("hook status")),
        };
        assert!(runs_hook_status, "{event}: {entry}");
    }
}
