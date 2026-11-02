use super::*;

/// A Codex console takes its hooks inline (ADR-0085 D3): the live child's
/// argv has the trust flag and one `-c hooks.<Event>=` per status event,
/// each running `hook status codex`; its environment names the session's
/// status file; no settings file is written; and a line in that file shows
/// up on `/api/sessions`.
#[tokio::test]
async fn a_codex_console_is_launched_with_the_status_hooks() {
    let dir = tempfile::tempdir().unwrap();
    let (port, store) = start_daemon(dir.path()).await;
    let mut ws = page::console(
        port,
        serde_json::json!({ "repo": "owner/statelab", "agent": "codex" }),
    )
    .await;
    let sessions_dir = store.join("sessions");
    let status = sessions_dir.join("1.agent-status.jsonl");

    ws.send(terminal(b"env RALPHY_STATUS_FILE\r"))
        .await
        .unwrap();
    ws.send(terminal(b"argv\r")).await.unwrap();
    let want_env = flatten(&format!(
        "ENV:RALPHY_STATUS_FILE={}",
        status.to_string_lossy()
    ));
    let events = ralphy_proc_util::hook_form::CODEX_STATUS_EVENTS;
    let want_hook = |event: &str| format!("-chooks.{event}=[{{hooks=[{{type=");
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
                if flat.contains(&want_env) && flat.contains(&want_hook("Interrupt")) {
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
    assert!(
        flat.contains("ARGV:--dangerously-bypass-hook-trust"),
        "the trust flag comes first, got:\n{got}"
    );
    for event in events {
        assert_eq!(
            flat.matches(&want_hook(event)).count(),
            1,
            "{event}, got:\n{got}"
        );
    }
    assert_eq!(
        flat.matches("hookstatuscodex").count(),
        events.len(),
        "every hook runs `hook status codex`, got:\n{got}"
    );
    assert!(status.is_file(), "the status file exists before any hook");
    assert!(
        !sessions_dir.join("1.settings.json").exists(),
        "Codex takes its hooks inline: no settings file"
    );

    // "The hook" appends a permission line; the pump's tick folds it.
    {
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&status)
            .unwrap();
        writeln!(
            f,
            concat!(
                r#"{{"event":"PermissionRequest","session_id":"s1","tool_name":"Bash","#,
                r#""ts":"2026-10-28T10:00:00-03:00"}}"#
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
    let rows: serde_json::Value = serde_json::from_str(&row).unwrap();
    assert_eq!(rows[0]["agent_state"]["state"], "waiting", "{row}");
    assert_eq!(
        rows[0]["agent_state"]["detail"], "permission: Bash",
        "{row}"
    );

    ws.send(terminal(b"quit\r")).await.unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while http_get(port, "/api/sessions").await != "[]" {
        assert!(Instant::now() < deadline, "the session did not end");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(!status.exists(), "status file removed with the session");
}
