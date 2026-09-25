//! ADR-0059 §5 over the workbench's interactive launch: a Claude console is
//! launched with `--settings <store>/sessions/<id>.settings.json` registering
//! the status hooks, its environment carries `RALPHY_STATUS_FILE` naming
//! `<store>/sessions/<id>.agent-status.jsonl`, and a line appended to that
//! file by "the hook" (this test, standing in for `ralphy hook status`) shows
//! up as `agent_state` on `/api/sessions` — `waiting` with its detail. When
//! the child exits, both files are gone.
//!
//! Read off the LIVE child (`env`/`argv` echoed by `session_test_child`), not
//! off the spec: a regression that dropped `spec.env` would leave the file
//! written and the hook pointing nowhere.
//!
//! SOLE env-setter in its file: `RALPHY_DAEMON_DIR` and
//! `RALPHY_DAEMON_AGENT_OVERRIDE` are process-global.

use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use ralphy_daemon::protocol::{self, Frame};
use ralphy_daemon::{registry, router};
use ralphy_pty::{CURSOR_POSITION_REPLY, CURSOR_POSITION_REQUEST};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_tungstenite::tungstenite::Message;

fn terminal(data: &[u8]) -> Message {
    Message::Binary(
        protocol::encode(&Frame::Terminal {
            session: 1,
            data: data.to_vec(),
        })
        .into(),
    )
}

async fn http_get(port: u16, path: &str) -> String {
    let mut sock = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .unwrap();
    sock.write_all(
        format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n").as_bytes(),
    )
    .await
    .unwrap();
    let mut raw = String::new();
    sock.read_to_string(&mut raw).await.unwrap();
    raw.split_once("\r\n\r\n")
        .map(|(_, body)| body.to_string())
        .unwrap_or(raw)
}

fn flatten(s: &str) -> String {
    s.replace('\\', "/")
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect()
}

#[tokio::test]
async fn a_claude_console_gets_the_status_hooks_and_reports_its_agent_state() {
    let dir = tempfile::tempdir().unwrap();
    let store = dir.path().join("store");
    std::fs::create_dir_all(&store).unwrap();
    let registry_path = dir.path().join("repos.toml");
    let mut reg = registry::RegistryStore::default();
    let slug = "owner/statelab";
    reg.upsert(slug, &dir.path().to_string_lossy());
    registry::save_to(&reg, &registry_path).unwrap();

    std::env::set_var("RALPHY_DAEMON_DIR", &store);
    std::env::set_var(
        "RALPHY_DAEMON_AGENT_OVERRIDE",
        env!("CARGO_BIN_EXE_session_test_child"),
    );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let (_tx, rx) = tokio::sync::watch::channel(false);
    let app = router(
        None,
        registry_path,
        std::path::PathBuf::from("does-not-exist"),
        ralphy_daemon::StorePaths::default(),
        Instant::now(),
        rx,
        ralphy_daemon::auth::AuthState::localhost(),
    );
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let url = format!("ws://127.0.0.1:{port}/ws/session?repo=owner%2Fstatelab&agent=claude");
    let (mut ws, _resp) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("a claude console launches");

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
                _ => continue,
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
        let cmd = doc["hooks"][event][0]["hooks"][0]["command"]
            .as_str()
            .unwrap_or_else(|| panic!("{event} registered"));
        assert!(cmd.ends_with("hook status"), "{event}: {cmd}");
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
            r#"{{"event":"PreToolUse","tool_name":"AskUserQuestion","tool_input":{{"questions":[{{"question":"which port?"}}]}},"interrupted":false,"ts":"2026-09-15T10:00:00-03:00"}}"#
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
    assert!(!settings.exists(), "settings removed with the session");
    assert!(!status.exists(), "status file removed with the session");
}
