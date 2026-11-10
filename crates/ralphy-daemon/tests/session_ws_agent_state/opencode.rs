use super::*;

/// An `OpenCode` console loads Ralphy's state plugin (ADR-0085 D2): the live
/// child's `OPENCODE_CONFIG_CONTENT` names the session's plugin file, and
/// only it, as a `file:///` URL; `RALPHY_STATUS_FILE` names the session's
/// status file; the plugin file holds the plugin; a line in the status file
/// shows up on `/api/sessions`; both files go with the session.
#[tokio::test]
async fn an_opencode_console_is_launched_with_the_state_plugin() {
    let dir = tempfile::tempdir().unwrap();
    let (port, store) = start_daemon(dir.path()).await;
    let mut ws = page::console(
        port,
        serde_json::json!({ "repo": "owner/statelab", "agent": "opencode" }),
    )
    .await;
    let sessions_dir = store.join("sessions");
    let status = sessions_dir.join("1.agent-status.jsonl");
    let plugin = sessions_dir.join("1.state-plugin.js");

    let ask = b"env RALPHY_STATUS_FILE\renv OPENCODE_CONFIG_CONTENT\r";
    ws.send(terminal(ask)).await.unwrap();
    let want_env = flatten(&format!(
        "ENV:RALPHY_STATUS_FILE={}",
        status.to_string_lossy()
    ));
    let entry = flatten(&plugin.to_string_lossy());
    let want_config = format!(
        r#"ENV:OPENCODE_CONFIG_CONTENT={{"plugin":["file:///{}"]}}"#,
        entry.trim_start_matches('/')
    );
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
                if flat.contains(&want_env) && flat.contains("ENV:OPENCODE_CONFIG_CONTENT=") {
                    return acc;
                }
            }
        }
        acc
    })
    .await
    .expect("the env round-trip must complete within 10s");
    let flat = flatten(&got);
    assert!(flat.contains(&want_env), "wanted {want_env}, got:\n{got}");
    assert!(
        flat.contains(&want_config),
        "wanted {want_config}, got:\n{got}"
    );
    let source = std::fs::read_to_string(&plugin).expect("the plugin file is written");
    assert!(source.contains("plugin.loaded"), "{source}");
    assert!(
        status.is_file(),
        "the status file exists before the plugin runs"
    );
    assert!(
        !sessions_dir.join("1.settings.json").exists(),
        "OpenCode takes a plugin: no settings file"
    );

    // "The plugin" writes its first line and a permission; the tick folds them.
    {
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&status)
            .unwrap();
        writeln!(
            f,
            r#"{{"event":"plugin.loaded","ts":"2026-10-28T13:00:00.000Z"}}"#
        )
        .unwrap();
        writeln!(
            f,
            concat!(
                r#"{{"event":"permission.asked","session_id":"ses_1","tool_name":"bash","#,
                r#""tool_use_id":"per_1","ts":"2026-10-28T13:00:01.000Z"}}"#
            )
        )
        .unwrap();
    }
    let block = wait_for_block(port, |b| b.get("state").is_some()).await;
    assert_eq!(block["state"], "waiting", "{block}");
    assert_eq!(block["detail"], "permission: bash", "{block}");

    ws.send(terminal(b"quit\r")).await.unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while http_get(port, "/api/sessions").await != "[]" {
        assert!(Instant::now() < deadline, "the session did not end");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(!status.exists(), "status file removed with the session");
    assert!(!plugin.exists(), "plugin file removed with the session");
}
