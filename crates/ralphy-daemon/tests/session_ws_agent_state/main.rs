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
    clippy::tests_outside_test_module,
    reason = "an integration test crate is the test module"
)]
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

mod claude;
mod codex;
mod opencode;
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
        .map(|(_, body)| body.to_owned())
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
    let (tx, rx) = tokio::sync::watch::channel(false);
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
        let _keep = tx;
        axum::serve(listener, app)
            .await
            .expect("serve the test app");
    });
    (port, store)
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

/// The console's output once `done` accepts its flattened form, answering
/// the cursor position request on the way; panics after 10 s.
async fn read_echo(ws: &mut page::Channel, done: impl Fn(&str) -> bool) -> String {
    tokio::time::timeout(Duration::from_secs(10), async {
        let mut acc = String::new();
        while let Some(msg) = ws.next().await {
            let Message::Binary(bytes) = msg.expect("read the next frame") else {
                continue;
            };
            if let Ok(Frame::Terminal { data, .. }) = protocol::decode(&bytes) {
                if data
                    .windows(CURSOR_POSITION_REQUEST.len())
                    .any(|w| w == CURSOR_POSITION_REQUEST)
                {
                    ws.send(terminal(CURSOR_POSITION_REPLY))
                        .await
                        .expect("answer the cursor position request");
                }
                acc.push_str(&String::from_utf8_lossy(&data));
                if done(&flatten(&acc)) {
                    return acc;
                }
            }
        }
        acc
    })
    .await
    .expect("the env + argv round-trip must complete within 10s")
}

/// Append one line to the status file, as the hook does.
fn append_status(status: &std::path::Path, line: &str) {
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new()
        .append(true)
        .open(status)
        .expect("open the status file");
    writeln!(f, "{line}").expect("append to the status file");
}

/// The `/api/sessions` body once it carries an `agent_state`; panics after 5 s.
async fn wait_for_agent_state(port: u16) -> String {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let body = http_get(port, "/api/sessions").await;
        if body.contains("agent_state") {
            return body;
        }
        assert!(
            Instant::now() < deadline,
            "no agent_state within 5s: {body}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Wait until `/api/sessions` is empty; panics after 10 s.
async fn wait_for_no_session(port: u16) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while http_get(port, "/api/sessions").await != "[]" {
        assert!(Instant::now() < deadline, "the session did not end");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}
