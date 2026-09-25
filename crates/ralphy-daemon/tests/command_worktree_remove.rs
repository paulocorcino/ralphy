//! The daemon's own gate on `worktree.remove` (ADR-0063 §2, issue #409): while
//! a live session's `checkout` names the worktree, the verb is refused IN the
//! daemon — `status:"error"`, message `has a live console` — before any argv
//! is composed or any child spawned; once that session is closed the same
//! request reaches the child, which echoes `worktree remove -- <name>` and
//! exits non-zero so the Mutate branch relays its argv as the message. The
//! gate is per name: another worktree of the same repo, with no console in it,
//! goes through while the first is refused.
//!
//! One live loopback daemon, legs sequential. SOLE setter in its process of
//! `RALPHY_EXE_OVERRIDE` (= `command_test_child`, `RALPHY_TEST_EXIT_CODE=1`)
//! and `RALPHY_DAEMON_AGENT_OVERRIDE` (= `session_test_child`).

use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use ralphy_daemon::protocol::{self, Command, Frame};
use ralphy_daemon::{registry, router};
use ralphy_pty::{CURSOR_POSITION_REPLY, CURSOR_POSITION_REQUEST};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_tungstenite::tungstenite::Message;

type Ws =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// Send one command on a fresh `/ws/command` socket and return the whole reply.
async fn command_reply(
    port: u16,
    id: u64,
    verb: &str,
    payload: serde_json::Value,
) -> serde_json::Value {
    let url = format!("ws://127.0.0.1:{port}/ws/command");
    let (mut ws, _resp) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("connecting to /ws/command");
    ws.send(Message::Binary(
        protocol::encode(&Frame::Command(Command {
            id,
            verb: verb.to_string(),
            payload,
        }))
        .into(),
    ))
    .await
    .unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        while let Some(msg) = ws.next().await {
            let bytes = match msg.unwrap() {
                Message::Binary(b) => b,
                Message::Close(_) => break,
                _ => continue,
            };
            if let Ok(Frame::Command(cmd)) = protocol::decode(&bytes) {
                if cmd.id == id {
                    return Some(cmd.payload);
                }
            }
        }
        None
    })
    .await
    .expect("a reply must arrive within 10s")
    .expect("a reply on the requesting id")
}

fn terminal(data: &[u8]) -> Message {
    Message::Binary(
        protocol::encode(&Frame::Terminal {
            session: 1,
            data: data.to_vec(),
        })
        .into(),
    )
}

/// Read terminal frames until `needle`, answering ConPTY's startup `ESC[6n`,
/// and capture the `session-open` payload (sent before any terminal byte).
async fn read_until(ws: &mut Ws, needle: &str) -> Option<serde_json::Value> {
    let mut terminal_text = String::new();
    let mut open = None;
    let completed = tokio::time::timeout(Duration::from_secs(10), async {
        while let Some(message) = ws.next().await {
            match message.unwrap() {
                Message::Binary(bytes) => match protocol::decode(&bytes) {
                    Ok(Frame::Terminal { data, .. }) => {
                        if data
                            .windows(CURSOR_POSITION_REQUEST.len())
                            .any(|window| window == CURSOR_POSITION_REQUEST)
                        {
                            ws.send(terminal(CURSOR_POSITION_REPLY)).await.unwrap();
                        }
                        terminal_text.push_str(&String::from_utf8_lossy(&data));
                        if terminal_text.replace("\r\n", "").contains(needle) {
                            return true;
                        }
                    }
                    Ok(Frame::Command(command)) if command.verb == "session-open" => {
                        open = Some(command.payload);
                    }
                    _ => {}
                },
                Message::Close(_) => return true,
                _ => {}
            }
        }
        true
    })
    .await;
    assert!(
        completed.is_ok(),
        "timed out waiting for {needle:?}; open={open:?}; terminal={terminal_text:?}"
    );
    open
}

/// Raw HTTP over a fresh `TcpStream` (no HTTP client in dev-deps). `(status, body)`.
async fn http_request(port: u16, method: &str, path: &str) -> (u16, String) {
    let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .unwrap();
    let req = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    );
    stream.write_all(req.as_bytes()).await.unwrap();
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).await.unwrap();
    let text = String::from_utf8_lossy(&buf).into_owned();
    let status = text
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let body = text
        .split_once("\r\n\r\n")
        .map(|(_, b)| b.to_string())
        .unwrap_or_default();
    (status, body)
}

async fn sessions(port: u16) -> Vec<serde_json::Value> {
    let (_, body) = http_request(port, "GET", "/api/sessions?local=1").await;
    serde_json::from_str(&body).unwrap_or_else(|e| panic!("sessions body {body:?}: {e}"))
}

fn message(reply: &serde_json::Value) -> String {
    reply["message"]
        .as_str()
        .unwrap_or_else(|| panic!("an error message string; got {reply}"))
        .to_string()
}

#[tokio::test]
async fn worktree_remove_is_refused_while_a_console_lives_in_it_and_proceeds_after_close() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    // The primary LOOKS like a git checkout — the Cursor gate walks for `.git`.
    std::fs::create_dir(root.join(".git")).unwrap();
    let registry_path = root.join("repos.toml");
    let mut store = registry::RegistryStore::default();
    let slug = "owner/wtrm";
    store.upsert(slug, &root.to_string_lossy());
    registry::save_to(&store, &registry_path).unwrap();
    // Two linked worktrees as git records them: pointer FILES, no git spawn.
    for name in ["wt-a", "wt-b"] {
        let wt = root.join(".ralphy").join("worktrees").join(name);
        std::fs::create_dir_all(&wt).unwrap();
        std::fs::write(
            wt.join(".git"),
            format!("gitdir: /r/.git/worktrees/{name}\n"),
        )
        .unwrap();
    }

    std::env::set_var(
        "RALPHY_EXE_OVERRIDE",
        env!("CARGO_BIN_EXE_command_test_child"),
    );
    // Non-zero so the Mutate branch relays the child's output (the echoed argv).
    std::env::set_var("RALPHY_TEST_EXIT_CODE", "1");
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

    // --- (a) a claude console lives in wt-a.
    let (mut ws_a, _) = tokio_tungstenite::connect_async(format!(
        "ws://127.0.0.1:{port}/ws/session?repo=owner%2Fwtrm&agent=claude&checkout=wt-a"
    ))
    .await
    .expect("a known checkout must upgrade");
    let open = read_until(&mut ws_a, "READY")
        .await
        .expect("session-open precedes the first terminal byte");
    assert_eq!(open["checkout"], "wt-a", "(a) got {open}");
    let id_a = open["session"].as_u64().expect("a numeric session id");

    // --- (b) removing wt-a is refused in the daemon: no child ran (the child
    // echoes `dispatch-argv` on every run, so its absence is the proof).
    let reply = command_reply(
        port,
        2,
        "worktree.remove",
        serde_json::json!({ "repo": slug, "name": "wt-a" }),
    )
    .await;
    assert_eq!(reply["status"], "error", "(b) got {reply}");
    let msg = message(&reply);
    assert!(
        msg.contains("has a live console"),
        "(b) the daemon's own refusal; got {msg:?}"
    );
    assert!(
        !msg.contains("dispatch-argv"),
        "(b) no child may have been spawned; got {msg:?}"
    );

    // --- (b') the gate is per name: wt-b has no console and reaches the child.
    let reply = command_reply(
        port,
        3,
        "worktree.remove",
        serde_json::json!({ "repo": slug, "name": "wt-b" }),
    )
    .await;
    assert_eq!(reply["status"], "error", "(b') got {reply}");
    let msg = message(&reply);
    assert!(
        msg.contains("worktree remove -- wt-b"),
        "(b') the argv must reach the child; got {msg:?}"
    );

    // --- (c) close the console.
    let (status, _) = http_request(port, "POST", &format!("/api/sessions/close?id={id_a}")).await;
    assert_eq!(status, 200, "(c) close must succeed");
    assert!(
        sessions(port).await.is_empty(),
        "(c) no session row survives the close"
    );

    // --- (d) the same request now reaches the child.
    let reply = command_reply(
        port,
        4,
        "worktree.remove",
        serde_json::json!({ "repo": slug, "name": "wt-a" }),
    )
    .await;
    assert_eq!(reply["status"], "error", "(d) got {reply}");
    let msg = message(&reply);
    assert!(
        msg.contains("worktree remove -- wt-a"),
        "(d) after the close the argv must reach the child; got {msg:?}"
    );
}
