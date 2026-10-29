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
//! The presence socket `/ws` pushes `sessions.dirty` when the console
//! starts, when its agent state changes, and when it ends (ADR-0070 D2).
//!
//! A console of a daemon whose start check failed is silent at once: its
//! row carries `source: "silent"` and a reason, and no state, until the
//! first hook line.
//!
//! The environment and the start check are process-global, so each test
//! needs a process of its own (nextest runs each test in its own process).

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
    std::fs::create_dir_all(&store).unwrap();
    let registry_path = dir.join("repos.toml");
    let mut reg = registry::RegistryStore::default();
    reg.upsert("owner/statelab", &dir.to_string_lossy());
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
        // The shutdown sender lives as long as the server.
        let _keep = _tx;
        axum::serve(listener, app).await.unwrap();
    });
    (port, store)
}

#[tokio::test]
async fn a_claude_console_gets_the_status_hooks_and_reports_its_agent_state() {
    let dir = tempfile::tempdir().unwrap();
    let (port, store) = start_daemon(dir.path()).await;

    let (mut presence, _resp) =
        tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{port}/ws"))
            .await
            .expect("connecting to /ws");

    let url = format!("ws://127.0.0.1:{port}/ws/session?repo=owner%2Fstatelab&agent=claude");
    let (mut ws, _resp) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("a claude console launches");
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
            r#"{{"event":"PreToolUse","tool_name":"AskUserQuestion","question":"which port?","ts":"2026-09-15T10:00:00-03:00"}}"#
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
    let url = format!("ws://127.0.0.1:{port}/ws/session?repo=owner%2Fstatelab&agent=codex");
    let (mut ws, _resp) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("a codex console launches");
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
            r#"{{"event":"PermissionRequest","session_id":"s1","tool_name":"Bash","ts":"2026-10-28T10:00:00-03:00"}}"#
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

/// A daemon whose start check failed, and a Claude console on it. Claude is
/// `session_test_child` found on `PATH`, so the check reads the version
/// `2.1.312` and runs the exec form; the hook program does not exist, so the
/// check fails. Returns the port, the console's status file and its socket.
async fn silent_console(
    dir: &std::path::Path,
) -> (
    u16,
    std::path::PathBuf,
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
) {
    let bin = dir.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let claude = bin.join(format!("claude{}", std::env::consts::EXE_SUFFIX));
    std::fs::copy(env!("CARGO_BIN_EXE_session_test_child"), &claude).unwrap();
    let path = std::env::var_os("PATH").unwrap_or_default();
    let mut dirs = vec![bin];
    dirs.extend(std::env::split_paths(&path));
    std::env::set_var("PATH", std::env::join_paths(dirs).unwrap());
    std::env::set_var("RALPHY_EXE_OVERRIDE", dir.join("no-such-ralphy"));
    tokio::task::spawn_blocking(ralphy_daemon::agent_state::run_start_check)
        .await
        .unwrap();
    let cause = ralphy_daemon::agent_state::start_check_failure()
        .expect("the hook program is missing, so the start check fails");
    assert!(cause.contains("no-such-ralphy"), "{cause}");

    let (port, store) = start_daemon(dir).await;
    let url = format!("ws://127.0.0.1:{port}/ws/session?repo=owner%2Fstatelab&agent=claude");
    let (ws, _resp) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("a claude console launches");
    let status = store.join("sessions").join("1.agent-status.jsonl");
    (port, status, ws)
}

#[tokio::test]
async fn a_silent_console_carries_source_and_reason_and_no_state() {
    let dir = tempfile::tempdir().unwrap();
    let (port, _status, _ws) = silent_console(dir.path()).await;
    let block = wait_for_block(port, |b| b.get("source").is_some()).await;
    assert_eq!(block["source"], "silent", "{block}");
    let reason = block["reason"].as_str().unwrap_or("");
    assert!(reason.contains("no-such-ralphy"), "{block}");
    assert!(block.get("state").is_none(), "{block}");
}

#[tokio::test]
async fn the_first_hook_line_clears_the_silence() {
    use std::io::Write;
    let dir = tempfile::tempdir().unwrap();
    let (port, status, _ws) = silent_console(dir.path()).await;
    wait_for_block(port, |b| b["source"] == "silent").await;
    let mut f = std::fs::OpenOptions::new()
        .append(true)
        .open(&status)
        .unwrap();
    writeln!(
        f,
        r#"{{"event":"UserPromptSubmit","ts":"2026-10-28T10:00:00-03:00"}}"#
    )
    .unwrap();
    let block = wait_for_block(port, |b| b.get("state").is_some()).await;
    assert_eq!(block["state"], "working", "{block}");
    assert!(block.get("source").is_none(), "{block}");
    assert!(block.get("reason").is_none(), "{block}");
}
