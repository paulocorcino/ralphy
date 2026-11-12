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

    assert_codex_hooks_inline(&mut ws, &sessions_dir, &status).await;

    // "The hook" appends a permission line; the pump's tick folds it.
    append_status(
        &status,
        concat!(
            r#"{"event":"PermissionRequest","session_id":"s1","tool_name":"Bash","#,
            r#""ts":"2026-10-28T10:00:00-03:00"}"#
        ),
    );
    let row = wait_for_agent_state(port).await;
    let rows: serde_json::Value = serde_json::from_str(&row).unwrap();
    assert_eq!(rows[0]["agent_state"]["state"], "waiting", "{row}");
    assert_eq!(
        rows[0]["agent_state"]["detail"], "permission: Bash",
        "{row}"
    );

    ws.send(terminal(b"quit\r")).await.unwrap();
    wait_for_no_session(port).await;
    assert!(!status.exists(), "status file removed with the session");
}

/// The live child's env names the status file, its argv carries the trust
/// flag and one inline hook per status event, and no settings file exists.
async fn assert_codex_hooks_inline(
    ws: &mut page::Channel,
    sessions_dir: &std::path::Path,
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
    let events = ralphy_proc_util::hook_form::CODEX_STATUS_EVENTS;
    let want_hook = |event: &str| format!("-chooks.{event}=[{{hooks=[{{type=");
    let got = read_echo(ws, |flat| {
        flat.contains(&want_env) && flat.contains(&want_hook("Interrupt"))
    })
    .await;
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
}
