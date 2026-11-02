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

    ws.send(terminal(b"env RALPHY_STATUS_FILE\r"))
        .await
        .unwrap();
    ws.send(terminal(b"argv\r")).await.unwrap();
    let want_env = flatten(&format!(
        "ENV:RALPHY_STATUS_FILE={}",
        status.to_string_lossy()
    ));
    let want_argv = flatten(&format!("ARGV:--settings {}", settings.to_string_lossy()));

    let got = tokio::time::timeout(Duration::from_secs(10), async {
        let mut acc = String::new();
        while let Some(msg) = ws.next().await {
            let bytes = match msg.unwrap() {
                Message::Binary(b) => b,
                Message::Text(_)
                | Message::Ping(_)
                | Message::Pong(_)
                | Message::Close(_)
                | Message::Frame(_) => continue,
            };
            if let Ok(Frame::Terminal { data, .. }) = protocol::decode(&bytes) {
                if data
                    .windows(CURSOR_POSITION_REQUEST.len())
                    .any(|w| w == CURSOR_POSITION_REQUEST)
                {
                    ws.send(terminal(CURSOR_POSITION_REPLY)).await.unwrap();
                }
                acc.push_str(&String::from_utf8_lossy(&data));
                let flat = flatten(&acc);
                if flat.contains(&want_env) && flat.contains(&want_argv) {
                    return acc;
                }
            }
        }
        acc
    })
    .await
    .expect("the env + argv round-trip must complete within 10s");
    let flat = flatten(&got);
    assert!(flat.contains(&want_env), "wanted {want_env}, got:\n{got}");
    assert!(flat.contains(&want_argv), "wanted {want_argv}, got:\n{got}");

    // The daemon wrote the hook set — the six events, `hook status` each.
    let doc: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&settings).unwrap()).unwrap();
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
        let entry = &doc["hooks"][event][0]["hooks"][0];
        let runs_hook_status = match entry["args"].as_array() {
            Some(args) => args == &[serde_json::json!("hook"), serde_json::json!("status")],
            None => entry["command"]
                .as_str()
                .is_some_and(|c| c.ends_with("hook status")),
        };
        assert!(runs_hook_status, "{event}: {entry}");
    }
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
    {
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&status)
            .unwrap();
        writeln!(
            f,
            concat!(
                r#"{{"event":"PreToolUse","tool_name":"AskUserQuestion","#,
                r#""question":"which port?","ts":"2026-09-15T10:00:00-03:00"}}"#
            )
        )
        .unwrap();
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    let row = loop {
        let body = http_get(port, "/api/sessions").await;
        if body.contains("agent_state") {
            break body;
        }
        assert!(
            Instant::now() < deadline,
            "no agent_state within 5s: {body}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
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
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if http_get(port, "/api/sessions").await == "[]" {
            break;
        }
        assert!(Instant::now() < deadline, "the session did not end");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(
        sessions_dirty(&mut presence).await,
        "sessions.dirty when the console ends"
    );
    assert!(!settings.exists(), "settings removed with the session");
    assert!(!status.exists(), "status file removed with the session");
}
