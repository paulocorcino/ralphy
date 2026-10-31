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
//! The presence frames of the page socket `/ws` include `sessions.dirty`, pushed when the console
//! starts, when its agent state changes, and when it ends (ADR-0070 D2).
//!
//! The environment and the start check are process-global, so each test
//! needs a process of its own (nextest runs each test in its own process).

#![expect(
    clippy::disallowed_methods,
    reason = "nextest runs each test in its own process, so a test may change its environment"
)]

use std::time::{Duration, Instant};

use futures_util::StreamExt;
use ralphy_daemon::protocol::{self, Frame};
use ralphy_daemon::{registry, router};
use ralphy_pty::{CURSOR_POSITION_REPLY, CURSOR_POSITION_REQUEST};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_tungstenite::tungstenite::Message;

#[path = "../support/page.rs"]
pub mod page;

mod silent_console;

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
        .expect("connect to the test server");
    sock.write_all(
        format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n").as_bytes(),
    )
    .await
    .expect("send the request");
    let mut raw = String::new();
    sock.read_to_string(&mut raw)
        .await
        .expect("read the response");
    raw.split_once("\r\n\r\n")
        .map(|(_, body)| body.to_string())
        .unwrap_or(raw)
}

/// Wait for the next `sessions.dirty` push on the presence socket, skipping
/// heartbeats; `false` when none arrives within 5 s.
async fn sessions_dirty<S>(ws: &mut S) -> bool
where
    S: futures_util::Stream<Item = tokio_tungstenite::tungstenite::Result<Message>> + Unpin,
{
    tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(msg) = ws.next().await {
            let Ok(Message::Binary(bytes)) = msg else {
                continue;
            };
            if let Ok(Frame::Command(c)) = protocol::decode(&bytes) {
                if c.verb == "sessions.dirty" {
                    return true;
                }
            }
        }
        false
    })
    .await
    .unwrap_or(false)
}

fn flatten(s: &str) -> String {
    s.replace('\\', "/")
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect()
}

/// A scratch daemon in-process over `dir`: one registered repo
/// `owner/statelab`, the store under `dir/store`, and `session_test_child`
/// as every agent. Returns the port and the store.
async fn start_daemon(dir: &std::path::Path) -> (u16, std::path::PathBuf) {
    let store = dir.join("store");
    std::fs::create_dir_all(&store).expect("create the test directory");
    let registry_path = dir.join("repos.toml");
    let mut reg = registry::RegistryStore::default();
    reg.upsert("owner/statelab", &dir.to_string_lossy());
    registry::save_to(&reg, &registry_path).expect("save the registry");

    std::env::set_var("RALPHY_DAEMON_DIR", &store);
    std::env::set_var(
        "RALPHY_DAEMON_AGENT_OVERRIDE",
        env!("CARGO_BIN_EXE_session_test_child"),
    );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind a local port");
    let port = listener
        .local_addr()
        .expect("read the bound address")
        .port();
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
        // The shutdown sender lives as long as the server.
        let _keep = _tx;
        axum::serve(listener, app)
            .await
            .expect("serve the test app");
    });
    (port, store)
}

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

/// An OpenCode console loads Ralphy's state plugin (ADR-0085 D2): the live
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

/// The `/api/sessions` row of the one console once `ready` accepts its
/// `agent_state` block; panics after 10 s.
async fn wait_for_block(
    port: u16,
    ready: impl Fn(&serde_json::Value) -> bool,
) -> serde_json::Value {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let body = http_get(port, "/api/sessions").await;
        let rows: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
        let block = &rows[0]["agent_state"];
        if ready(block) {
            return block.clone();
        }
        assert!(
            Instant::now() < deadline,
            "no such agent_state within 10s: {body}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}
