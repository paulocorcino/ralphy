//! The `spawn` channel: one command from the open, the run's frames on the
//! channel, then `channel.closed`. The spawned exe is `command_test_child`,
//! as in `tests/command_ws.rs`.
//!
//! The exe override is a process-wide variable. Each test sets what it needs
//! and runs in its own process under nextest.

#![expect(
    clippy::disallowed_methods,
    reason = "nextest runs each test in its own process, so a test may change its environment"
)]

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use ralphy_daemon::protocol::{self, Frame};
use ralphy_daemon::registry;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

use crate::support::{connect, next_closed, next_inner, open, serve_with, Daemon, Ws};

const WITHIN: Duration = Duration::from_secs(10);
const SLUG: &str = "owner/spawn-channel";

/// A daemon whose registry holds `SLUG` with the checkout `repo`, and the
/// path of its store.
async fn serve_registered(repo: &Path) -> (Daemon, PathBuf) {
    let mut store_dir = PathBuf::new();
    let daemon = serve_with(|store| {
        store_dir = store.to_path_buf();
        let mut entries = registry::RegistryStore::default();
        entries.upsert(SLUG, &repo.to_string_lossy());
        registry::save_to(&entries, &store.join("repos.toml")).expect("save the registry");
    })
    .await;
    std::env::set_var(
        "RALPHY_EXE_OVERRIDE",
        env!("CARGO_BIN_EXE_command_test_child"),
    );
    (daemon, store_dir)
}

/// The open fields of a `run` of `SLUG`.
fn run_open() -> serde_json::Value {
    serde_json::json!({
        "command": {
            "id": 5,
            "verb": "run",
            "payload": { "repo": SLUG, "agent": "claude", "branchMode": "new" },
        },
    })
}

/// The `status` and the whole payload of the next command frame on
/// `channel`.
async fn next_status(ws: &mut Ws, channel: u32) -> Option<(String, serde_json::Value)> {
    let inner = next_inner(ws, channel, WITHIN).await?;
    let Ok(Frame::Command(command)) = protocol::decode(&inner) else {
        return None;
    };
    assert_eq!(command.id, 5, "every frame answers the command that opened");
    let status = command.payload["status"].as_str()?.to_string();
    Some((status, command.payload))
}

/// Read `channel` up to its `exited` frame: every status, in order.
async fn statuses_until_exited(ws: &mut Ws, channel: u32) -> Vec<(String, serde_json::Value)> {
    let mut seen = Vec::new();
    while let Some((status, payload)) = next_status(ws, channel).await {
        let last = status == "exited";
        seen.push((status, payload));
        if last {
            break;
        }
    }
    seen
}

#[tokio::test]
async fn a_spawn_channel_streams_a_run_then_closes() {
    let repo = tempfile::tempdir().expect("a checkout");
    let (daemon, _store) = serve_registered(repo.path()).await;
    std::env::set_var("RALPHY_TEST_EXIT_CODE", "7");
    let mut ws = connect(&daemon).await;

    open(&mut ws, 1, "spawn", run_open()).await;

    let seen = statuses_until_exited(&mut ws, 1).await;
    let names: Vec<&str> = seen.iter().map(|(status, _)| status.as_str()).collect();
    assert_eq!(names.first(), Some(&"spawned"), "{names:?}");
    assert_eq!(names.last(), Some(&"exited"), "{names:?}");
    assert!(names.contains(&"output"), "{names:?}");
    assert!(seen[0].1["pid"].as_u64().is_some(), "{:?}", seen[0].1);
    let output: String = seen
        .iter()
        .filter_map(|(_, payload)| payload["chunk"].as_str())
        .collect();
    assert!(output.contains("dispatch-stdout-marker"), "{output:?}");
    assert!(output.contains("dispatch-stderr-marker"), "{output:?}");
    let exited = &seen[seen.len() - 1].1;
    assert_eq!(exited["code"].as_i64(), Some(7), "{exited}");

    // The end comes after the last frame of the run, never before it.
    let closed = next_closed(&mut ws, WITHIN)
        .await
        .expect("a channel.closed after the exit");
    assert_eq!((closed.channel, closed.reason.as_str()), (1, "done"));
}

/// The `ralphy_device=…` cookie the daemon gives a first `/api/session`.
async fn device_cookie(port: u16) -> String {
    let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .expect("the test socket connects");
    let request =
        format!("GET /api/session HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n");
    stream
        .write_all(request.as_bytes())
        .await
        .expect("the request is written");
    let mut reply = String::new();
    stream
        .read_to_string(&mut reply)
        .await
        .expect("the reply is read");
    reply
        .lines()
        .filter_map(|line| line.split_once(':'))
        .filter(|(name, _)| name.eq_ignore_ascii_case("set-cookie"))
        .map(|(_, value)| value.trim())
        .find(|value| value.starts_with("ralphy_device="))
        .unwrap_or_else(|| panic!("a device cookie in {reply}"))
        .split(';')
        .next()
        .expect("the reply sets a device cookie")
        .to_string()
}

#[tokio::test]
async fn a_spawn_on_a_channel_is_recorded_with_its_device() {
    let repo = tempfile::tempdir().expect("a checkout");
    let (daemon, store) = serve_registered(repo.path()).await;
    let cookie = device_cookie(daemon.port).await;
    let device = cookie
        .split('.')
        .nth(1)
        .expect("the cookie names a device")
        .to_string();
    let mut request = format!("ws://127.0.0.1:{}/ws", daemon.port)
        .into_client_request()
        .expect("a /ws request");
    request.headers_mut().insert(
        "cookie",
        cookie.parse().expect("the cookie is a header value"),
    );
    let (mut ws, _resp) = tokio_tungstenite::connect_async(request)
        .await
        .expect("connect to /ws");

    open(&mut ws, 1, "spawn", run_open()).await;
    statuses_until_exited(&mut ws, 1).await;

    let log = std::fs::read_to_string(store.join("daemon-audit.jsonl")).expect("the audit log");
    let commands: Vec<serde_json::Value> = log
        .lines()
        .map(|line| serde_json::from_str(line).expect("a JSON line"))
        .filter(|line: &serde_json::Value| line["event"] == "command")
        .collect();
    assert_eq!(commands.len(), 1, "{log}");
    assert_eq!(commands[0]["verb"], "run");
    assert_eq!(commands[0]["repo"], SLUG);
    assert_eq!(commands[0]["device"], device.as_str());
    assert_eq!(commands[0]["actor"], "device");
}

#[tokio::test]
async fn a_spawn_survives_the_page_socket_closing() {
    let repo = tempfile::tempdir().expect("a checkout");
    let (daemon, _store) = serve_registered(repo.path()).await;
    let done = repo.path().join("done");
    std::env::set_var("RALPHY_TEST_EXIT_CODE", "0");
    // The run outlives the socket: it writes the sentinel only just before
    // it exits.
    std::env::set_var("RALPHY_TEST_SLEEP_MS", "300");
    std::env::set_var("RALPHY_TEST_DONE_FILE", &done);
    let mut ws = connect(&daemon).await;

    open(&mut ws, 1, "spawn", run_open()).await;
    let (status, _) = next_status(&mut ws, 1).await.expect("a first frame");
    assert_eq!(status, "spawned");
    drop(ws);

    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Ok(text) = std::fs::read_to_string(&done) {
            assert_eq!(text, "dispatch-done");
            return;
        }
        assert!(
            Instant::now() < deadline,
            "the run did not survive the page socket closing"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}
