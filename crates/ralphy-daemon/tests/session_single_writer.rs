//! Single-writer attach policy over a real loopback WebSocket (docs/adr/0032 §2;
//! issue #166 AC4): two browsers can't corrupt one session. While one client is
//! attached, a second reattach WITHOUT `takeover` is refused with HTTP `409`
//! BEFORE the upgrade; the same reattach WITH `takeover=1` succeeds, EVICTS the
//! incumbent (its stream ends), and the taker drives the child. Proves the
//! explicit, race-free takeover the policy promises.

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use ralphy_daemon::protocol::{self, Command, Frame};
use ralphy_daemon::{registry, router};
use ralphy_pty::{CURSOR_POSITION_REPLY, CURSOR_POSITION_REQUEST};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_tungstenite::tungstenite::{self, Message};
use tokio_tungstenite::WebSocketStream;

type Ws = WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

fn terminal(data: &[u8]) -> Message {
    Message::Binary(protocol::encode(&Frame::Terminal {
        session: 1,
        data: data.to_vec(),
    }))
}

fn resize(rows: u16, cols: u16) -> Message {
    Message::Binary(protocol::encode(&Frame::Command(Command {
        id: 0,
        verb: "resize".to_string(),
        payload: serde_json::json!({ "rows": rows, "cols": cols }),
    })))
}

/// Drain `ws` until its stream ends, capturing the FIRST `session-end`
/// announcement seen on the way. Returning the reason at all is the assertion:
/// the loop breaks on Close/Err, so a reason here was necessarily read BEFORE the
/// stream ended — the relation issue #334 is about, not a frame count.
async fn drain_capturing_announcement(ws: &mut Ws, secs: u64) -> (bool, Option<String>) {
    let mut announced: Option<String> = None;
    let ended = tokio::time::timeout(Duration::from_secs(secs), async {
        while let Some(msg) = ws.next().await {
            match msg {
                Ok(Message::Binary(bytes)) => {
                    if let Ok(Frame::Command(cmd)) = protocol::decode(&bytes) {
                        if cmd.verb == "session-end" && announced.is_none() {
                            announced = cmd
                                .payload
                                .get("reason")
                                .and_then(|v| v.as_str())
                                .map(str::to_string);
                        }
                    }
                }
                Ok(Message::Close(_)) | Err(_) => break,
                Ok(_) => continue,
            }
        }
    })
    .await;
    (ended.is_ok(), announced)
}

/// Read until `needle` (or 10s); answer the ConPTY startup `ESC[6n` only when
/// `answer_cursor` (the taker reattaches past startup — see `session_persistence`).
async fn read_until(ws: &mut Ws, needle: &str, answer_cursor: bool) -> String {
    tokio::time::timeout(Duration::from_secs(10), async {
        let mut acc = String::new();
        while let Some(msg) = ws.next().await {
            let bytes = match msg.unwrap() {
                Message::Binary(b) => b,
                _ => continue,
            };
            if let Ok(Frame::Terminal { data, .. }) = protocol::decode(&bytes) {
                if answer_cursor
                    && data
                        .windows(CURSOR_POSITION_REQUEST.len())
                        .any(|w| w == CURSOR_POSITION_REQUEST)
                {
                    ws.send(terminal(CURSOR_POSITION_REPLY)).await.unwrap();
                }
                acc.push_str(&String::from_utf8_lossy(&data));
                if acc.contains(needle) {
                    return acc;
                }
            }
        }
        acc
    })
    .await
    .unwrap_or_else(|_| panic!("timed out (10s) waiting for {needle:?}"))
}

/// Boot a router on an ephemeral loopback port with `dir` registered as
/// `owner/workbench`, and point the launcher at the helper child. Returns the
/// port ALONGSIDE the shutdown sender, which the caller must hold: dropping it
/// makes every bridge's `shutdown.changed()` resolve at once, ending each session
/// as a daemon shutdown.
///
/// The env override is set exactly ONCE: the two tests in this binary run in
/// parallel, and a repeated `set_var` across threads is a data race even when
/// both writes carry the same value.
async fn start_daemon(dir: &std::path::Path) -> (u16, tokio::sync::watch::Sender<bool>) {
    static OVERRIDE: std::sync::Once = std::sync::Once::new();
    OVERRIDE.call_once(|| {
        std::env::set_var(
            "RALPHY_DAEMON_AGENT_OVERRIDE",
            env!("CARGO_BIN_EXE_session_test_child"),
        );
    });

    let registry_path = dir.join("repos.toml");
    let mut store = registry::RegistryStore::default();
    store.upsert("owner/workbench", &dir.to_string_lossy());
    registry::save_to(&store, &registry_path).unwrap();

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let (tx, rx) = tokio::sync::watch::channel(false);
    let app = router(
        None,
        registry_path,
        std::path::PathBuf::from("does-not-exist"),
        ralphy_daemon::StorePaths::default(),
        std::time::Instant::now(),
        rx,
        ralphy_daemon::auth::AuthState::localhost(),
    );
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (port, tx)
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

#[tokio::test]
async fn second_attach_needs_takeover_which_evicts_the_first() {
    let dir = tempfile::tempdir().unwrap();
    let (port, _shutdown) = start_daemon(dir.path()).await;

    // ws1: launch and round-trip a keystroke so it is the attached single writer.
    let url = format!("ws://127.0.0.1:{port}/ws/session?repo=owner%2Fworkbench&agent=claude");
    let (mut ws1, _) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("connecting ws1");
    ws1.send(terminal(b"first\r")).await.unwrap();
    read_until(&mut ws1, "GOT:first", true).await;

    let (_, body) = http_request(port, "GET", "/api/sessions").await;
    let list: serde_json::Value = serde_json::from_str(&body).unwrap();
    let id = list.as_array().unwrap()[0]["id"].as_u64().unwrap();

    // A second attach WITHOUT takeover, while ws1 holds the writer slot, must be
    // refused as HTTP 409 at the handshake — never a silently upgraded socket.
    let busy = format!("ws://127.0.0.1:{port}/ws/session?id={id}");
    match tokio_tungstenite::connect_async(&busy).await {
        Err(tungstenite::Error::Http(resp)) => {
            assert_eq!(
                resp.status(),
                409,
                "a busy session refuses a non-takeover attach"
            );
        }
        Ok(_) => panic!("second attach without takeover must NOT succeed"),
        Err(other) => panic!("expected an HTTP 409, got {other:?}"),
    }

    // The same attach WITH takeover=1 succeeds and evicts ws1.
    let take = format!("ws://127.0.0.1:{port}/ws/session?id={id}&takeover=1");
    let (mut ws2, _) = tokio_tungstenite::connect_async(&take)
        .await
        .expect("takeover attach must succeed");

    // ws1 is evicted: its stream ends within 5s (the server bridge broke and
    // dropped its socket) — NOT a hang. And on the way out it is TOLD why, in a
    // data frame read before the stream ends: the close metadata cannot carry
    // that (issue #334), which is what let the old client mistake an eviction for
    // a flaky link and steal the session back.
    let (ended, announced) = drain_capturing_announcement(&mut ws1, 5).await;
    assert!(
        ended,
        "the evicted first client's stream must end, not hang"
    );
    assert_eq!(
        announced.as_deref(),
        Some("taken-over"),
        "the eviction must be ANNOUNCED before the socket closes, naming its reason"
    );

    // The taker drives the child: a keystroke round-trips (no cursor answer — the
    // child is past startup on this reattach).
    ws2.send(terminal(b"takeover-ok\r")).await.unwrap();
    read_until(&mut ws2, "GOT:takeover-ok", false).await;

    // Close so the helper child (60s-capable in sleep mode) does not linger.
    http_request(port, "POST", &format!("/api/sessions/close?id={id}")).await;
}

/// `?id=N&watch=1` (issue #334): a second client reaches a BUSY session as a
/// reader — no `409`, no eviction — sees the same replay and the same live
/// stream, and its keystrokes and resizes never reach the child. The read half
/// is what makes "both contexts see the session's output" true; the write half
/// is what makes a watcher a watcher rather than a second writer.
#[tokio::test]
async fn a_watcher_reads_but_never_writes() {
    let dir = tempfile::tempdir().unwrap();
    let (port, _shutdown) = start_daemon(dir.path()).await;

    let url = format!("ws://127.0.0.1:{port}/ws/session?repo=owner%2Fworkbench&agent=claude");
    let (mut writer, _) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("connecting the writer");
    writer.send(terminal(b"first\r")).await.unwrap();
    read_until(&mut writer, "GOT:first", true).await;

    let (_, body) = http_request(port, "GET", "/api/sessions").await;
    let list: serde_json::Value = serde_json::from_str(&body).unwrap();
    let id = list.as_array().unwrap()[0]["id"].as_u64().unwrap();

    // The writer still holds the slot, so a plain `?id=` here would be a 409.
    let watch_url = format!("ws://127.0.0.1:{port}/ws/session?id={id}&watch=1");
    let (mut watcher, _) = tokio_tungstenite::connect_async(&watch_url)
        .await
        .expect("a watch attach must succeed on a BUSY session — never a 409");
    let replay = read_until(&mut watcher, "GOT:first", false).await;
    assert!(
        replay.contains("GOT:first"),
        "a watcher gets the same scrollback replay as a writer, got {replay:?}"
    );

    // The watcher types and resizes. Neither may reach the child.
    watcher.send(terminal(b"watcher-typing\r")).await.unwrap();
    watcher.send(resize(99, 200)).await.unwrap();

    // Settle BEFORE the writer speaks, or the oracle is vacuous by race: the
    // reader below stops at the writer's own line, so a watcher echo that merely
    // arrived LATER would never be looked at. Measured: without this wait the
    // assertions still passed with the watcher gate deleted. 600ms is ~12 polls
    // of the child's 50ms size ticker and orders of magnitude over a loopback
    // PTY round-trip.
    tokio::time::sleep(Duration::from_millis(600)).await;

    // The oracle is read on the WRITER's stream, and it is live: the writer's own
    // keystroke round-trips right after, so "the watcher's line is absent" cannot
    // pass merely because nothing was flowing.
    writer.send(terminal(b"after-watch\r")).await.unwrap();
    let seen = read_until(&mut writer, "GOT:after-watch", false).await;
    assert!(
        !seen.contains("GOT:watcher-typing"),
        "a watcher's keystroke must never reach the child, got {seen:?}"
    );
    assert!(
        !seen.contains("SIZE 200x99"),
        "a watcher's resize must never reach the PTY, got {seen:?}"
    );
    // …and the resize oracle itself is live: the WRITER's resize does land.
    writer.send(resize(40, 100)).await.unwrap();
    read_until(&mut writer, "SIZE 100x40", false).await;

    // A takeover evicts the WRITER and leaves the watcher untouched: exactly one
    // writer, and watching is not a claim on the slot.
    let take = format!("ws://127.0.0.1:{port}/ws/session?id={id}&takeover=1");
    let (mut taker, _) = tokio_tungstenite::connect_async(&take)
        .await
        .expect("takeover attach must succeed");
    let (ended, announced) = drain_capturing_announcement(&mut writer, 5).await;
    assert!(ended, "the evicted writer's stream must end");
    assert_eq!(announced.as_deref(), Some("taken-over"));

    taker.send(terminal(b"after-takeover\r")).await.unwrap();
    read_until(&mut taker, "GOT:after-takeover", false).await;
    let watched = read_until(&mut watcher, "GOT:after-takeover", false).await;
    assert!(
        watched.contains("GOT:after-takeover"),
        "the watcher's stream survives a takeover and keeps showing the session, got {watched:?}"
    );

    // A genuine end reaches the watcher too, named as such.
    http_request(port, "POST", &format!("/api/sessions/close?id={id}")).await;
    let (watcher_ended, watcher_told) = drain_capturing_announcement(&mut watcher, 5).await;
    assert!(watcher_ended, "the watcher's stream must end on a close");
    assert_eq!(
        watcher_told.as_deref(),
        Some("child-exited"),
        "a watcher is told the session ended, not left on a dead socket"
    );
}
