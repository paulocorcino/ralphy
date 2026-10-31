//! Single-writer attach policy over `console` channels of real loopback page
//! sockets (docs/adr/0032 §2; issue #166 AC4): two browsers can't corrupt one
//! session. While one client is attached, a second reattach WITHOUT `takeover`
//! is refused: its channel gets no frame and closes as `refused`, and the
//! incumbent keeps the slot. The same reattach WITH `takeover: 1` succeeds,
//! EVICTS the incumbent (its channel ends), and the taker drives the child.
//! Proves the explicit, race-free takeover the policy promises.

#![expect(
    clippy::disallowed_methods,
    reason = "nextest runs each test in its own process, so a test may change its environment"
)]

use std::time::Duration;

use ralphy_daemon::protocol::{self, Command, Frame};
use ralphy_daemon::{registry, router};
use ralphy_pty::{CURSOR_POSITION_REPLY, CURSOR_POSITION_REQUEST};
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_tungstenite::tungstenite::Message;

#[path = "support/page.rs"]
pub mod page;

use page::Channel as Ws;

fn terminal(data: &[u8]) -> Message {
    Message::Binary(
        protocol::encode(&Frame::Terminal {
            session: 1,
            data: data.to_vec(),
        })
        .into(),
    )
}

fn resize(rows: u16, cols: u16) -> Message {
    Message::Binary(
        protocol::encode(&Frame::Command(Command {
            id: 0,
            verb: "resize".to_string(),
            payload: serde_json::json!({ "rows": rows, "cols": cols }),
        }))
        .into(),
    )
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
            let bytes = match msg.expect("read the next frame") {
                Message::Binary(b) => b,
                Message::Text(_)
                | Message::Ping(_)
                | Message::Pong(_)
                | Message::Close(_)
                | Message::Frame(_) => continue,
            };
            if let Ok(Frame::Terminal { data, .. }) = protocol::decode(&bytes) {
                if answer_cursor
                    && data
                        .windows(CURSOR_POSITION_REQUEST.len())
                        .any(|w| w == CURSOR_POSITION_REQUEST)
                {
                    ws.send(terminal(CURSOR_POSITION_REPLY))
                        .await
                        .expect("send the frame");
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
    registry::save_to(&store, &registry_path).expect("save the registry");

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
        std::time::Instant::now(),
        rx,
        ralphy_daemon::auth::AuthState::localhost(),
    );
    tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("serve the test app");
    });
    (port, tx)
}

/// Raw HTTP over a fresh `TcpStream` (no HTTP client in dev-deps). `(status, body)`.
async fn http_request(port: u16, method: &str, path: &str) -> (u16, String) {
    let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .expect("connect to the test server");
    let req = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    );
    stream
        .write_all(req.as_bytes())
        .await
        .expect("send the request");
    let mut buf = Vec::new();
    stream
        .read_to_end(&mut buf)
        .await
        .expect("read the response");
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
    let mut ws1 = page::console(
        port,
        json!({ "repo": "owner/workbench", "agent": "claude" }),
    )
    .await;
    ws1.send(terminal(b"first\r")).await.unwrap();
    read_until(&mut ws1, "GOT:first", true).await;

    let (_, body) = http_request(port, "GET", "/api/sessions").await;
    let list: serde_json::Value = serde_json::from_str(&body).unwrap();
    let id = list.as_array().unwrap()[0]["id"].as_u64().unwrap();

    // A second attach WITHOUT takeover, while ws1 holds the writer slot, must be
    // refused before any frame — never a silently attached channel.
    refused(
        port,
        json!({ "id": id }),
        "a busy session refuses a non-takeover attach",
    )
    .await;
    // The refusal evicted nobody: ws1 still drives the child.
    ws1.send(terminal(b"still-first\r")).await.unwrap();
    read_until(&mut ws1, "GOT:still-first", false).await;

    // The same attach WITH takeover: 1 succeeds and evicts ws1.
    let mut ws2 = page::console(port, json!({ "id": id, "takeover": 1 })).await;

    // ws1 is evicted: its channel ends within 5s (the server bridge broke and
    // closed its channel) — NOT a hang. And on the way out it is TOLD why, in a
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
    assert_eq!(ws1.closed_reason(), Some("done"), "the daemon closed ws1");

    // The taker drives the child: a keystroke round-trips (no cursor answer — the
    // child is past startup on this reattach).
    ws2.send(terminal(b"takeover-ok\r")).await.unwrap();
    read_until(&mut ws2, "GOT:takeover-ok", false).await;

    // Close so the helper child (60s-capable in sleep mode) does not linger.
    http_request(port, "POST", &format!("/api/sessions/close?id={id}")).await;
}

/// `id: N, watch: 1` (issue #334): a second client reaches a BUSY session as a
/// reader — no refusal, no eviction — sees the same replay and the same live
/// stream, and its keystrokes and resizes never reach the child. The read half
/// is what makes "both contexts see the session's output" true; the write half
/// is what makes a watcher a watcher rather than a second writer.
#[tokio::test]
async fn a_watcher_reads_but_never_writes() {
    let dir = tempfile::tempdir().unwrap();
    let (port, _shutdown) = start_daemon(dir.path()).await;

    let mut writer = page::console(
        port,
        json!({ "repo": "owner/workbench", "agent": "claude" }),
    )
    .await;
    writer.send(terminal(b"first\r")).await.unwrap();
    read_until(&mut writer, "GOT:first", true).await;

    let (_, body) = http_request(port, "GET", "/api/sessions").await;
    let list: serde_json::Value = serde_json::from_str(&body).unwrap();
    let id = list.as_array().unwrap()[0]["id"].as_u64().unwrap();

    // An unknown id is refused even as a watcher — `watch` maps only `Unknown`.
    refused(
        port,
        json!({ "id": id + 9999, "watch": 1 }),
        "a watch on an unknown id is refused",
    )
    .await;

    // The writer still holds the slot, so a plain `id` here would be refused.
    let mut watcher = page::console(port, json!({ "id": id, "watch": 1 })).await;
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
    // The PTY gets the size clamped to 300 rows and 500 columns, as the
    // daemon's screen does; then the test goes back to 40x100.
    writer.send(resize(u16::MAX, u16::MAX)).await.unwrap();
    read_until(&mut writer, "SIZE 500x300", false).await;
    writer.send(resize(40, 100)).await.unwrap();
    read_until(&mut writer, "SIZE 100x40", false).await;

    // `watch: 1` is dispatched BEFORE the attach branch, so a stray `takeover: 1`
    // riding along cannot evict the writer. Swapping that order reds this.
    let mut sneak = page::console(port, json!({ "id": id, "watch": 1, "takeover": 1 })).await;
    writer.send(terminal(b"still-mine\r")).await.unwrap();
    read_until(&mut writer, "GOT:still-mine", false).await;
    // The channel was accepted as a watch: it shows the same output.
    read_until(&mut sneak, "GOT:still-mine", false).await;
    drop(sneak.close(None).await);

    // A takeover evicts the WRITER and leaves the watcher untouched: exactly one
    // writer, and watching is not a claim on the slot.
    let mut taker = page::console(port, json!({ "id": id, "takeover": 1 })).await;
    let (ended, announced) = drain_capturing_announcement(&mut writer, 5).await;
    assert!(ended, "the evicted writer's stream must end");
    assert_eq!(announced.as_deref(), Some("taken-over"));
    assert_eq!(
        writer.closed_reason(),
        Some("done"),
        "the daemon closed the writer"
    );

    // The evicted incumbent's guard-drop must not clear the TAKER's slot: asked
    // again AFTER the eviction has settled, a plain attach is still refused.
    // (Before the takeover the same probe passes even with the `ptr_eq` guard
    // removed, which is why it is asked here.)
    refused(
        port,
        json!({ "id": id }),
        "the taker must still hold the slot after the incumbent's guard dropped",
    )
    .await;

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
    assert_eq!(
        watcher.closed_reason(),
        Some("done"),
        "the daemon closed the watcher"
    );
}

/// The SILENT half of the announcement invariant (issue #334): a bridge that
/// falls out of its loop for any reason OTHER than a deliberate end must say
/// nothing. A client that closed its own socket, or whose link dropped, would
/// otherwise be told its session ended — and would park instead of recovering,
/// which is the mirror image of the bug this issue fixes.
///
/// Read from a WATCHER, because the client whose socket went away cannot report
/// what the daemon did or did not send it.
#[tokio::test]
async fn a_client_close_announces_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let (port, _shutdown) = start_daemon(dir.path()).await;

    let mut writer = page::console(
        port,
        json!({ "repo": "owner/workbench", "agent": "claude" }),
    )
    .await;
    writer.send(terminal(b"first\r")).await.unwrap();
    read_until(&mut writer, "GOT:first", true).await;

    let (_, body) = http_request(port, "GET", "/api/sessions").await;
    let list: serde_json::Value = serde_json::from_str(&body).unwrap();
    let id = list.as_array().unwrap()[0]["id"].as_u64().unwrap();

    let mut watcher = page::console(port, json!({ "id": id, "watch": 1 })).await;
    read_until(&mut watcher, "GOT:first", false).await;

    // The WRITER goes away on its own: it closes its channel and its page
    // socket. The session lives on (the tmux model), so nobody was evicted and
    // there is nothing to announce.
    writer.close(None).await.unwrap();
    drop(writer);

    // Drain the watcher for a bounded window: it must see no `session-end`. The
    // stream must also stay OPEN — a watcher is not collateral of a detach.
    let quiet = tokio::time::timeout(Duration::from_millis(1500), async {
        while let Some(msg) = watcher.next().await {
            match msg {
                Ok(Message::Binary(bytes)) => {
                    if let Ok(Frame::Command(cmd)) = protocol::decode(&bytes) {
                        if cmd.verb == "session-end" {
                            return Some(cmd);
                        }
                    }
                }
                Ok(Message::Close(_)) | Err(_) => return None,
                Ok(_) => continue,
            }
        }
        None
    })
    .await;
    assert!(
        quiet.is_err(),
        "a client-initiated close must announce NOTHING and end no other stream, got {quiet:?}"
    );

    // Still live: the slot was released, so a plain reattach succeeds and drives.
    let mut again = page::console(port, json!({ "id": id })).await;
    again.send(terminal(b"still-here\r")).await.unwrap();
    read_until(&mut again, "GOT:still-here", false).await;

    http_request(port, "POST", &format!("/api/sessions/close?id={id}")).await;
}

/// The third reason on the wire: a daemon going down tells every attachment so,
/// and names itself rather than the child (`session_ws`'s `shutdown.changed()`
/// arm, which no other test drives).
#[tokio::test]
async fn daemon_shutdown_is_announced_as_such() {
    let dir = tempfile::tempdir().unwrap();
    let (port, shutdown) = start_daemon(dir.path()).await;

    let mut ws = page::console(
        port,
        json!({ "repo": "owner/workbench", "agent": "claude" }),
    )
    .await;
    ws.send(terminal(b"first\r")).await.unwrap();
    read_until(&mut ws, "GOT:first", true).await;

    shutdown
        .send(true)
        .expect("the shutdown watch must accept a flip");

    let (ended, announced) = drain_capturing_announcement(&mut ws, 5).await;
    assert!(
        ended,
        "a shutting-down daemon must end the bridge, not hang it"
    );
    assert_eq!(
        announced.as_deref(),
        Some("daemon-shutdown"),
        "a daemon shutdown is its own reason — not the child exiting"
    );
}

/// Expect the attach `fields` to be refused: its channel gets no frame (no
/// `session-open`), then `channel.closed` with reason `refused`.
async fn refused(port: u16, fields: serde_json::Value, why: &str) {
    let mut channel = page::console(port, fields).await;
    let mut frames = Vec::new();
    tokio::time::timeout(Duration::from_secs(10), async {
        while let Some(message) = channel.next().await {
            match message.expect("the page socket reads") {
                Message::Binary(bytes) => frames.push(bytes.to_vec()),
                Message::Close(_) => break,
                Message::Text(_) | Message::Ping(_) | Message::Pong(_) | Message::Frame(_) => {}
            }
        }
    })
    .await
    .unwrap_or_else(|_| panic!("{why}: the daemon ends the attach within 10s"));
    assert!(
        frames.is_empty(),
        "{why}: a refused attach sends no frame: {frames:?}"
    );
    assert_eq!(channel.closed_reason(), Some("refused"), "{why}");
}

/// ADR-0051 §9 amendment 2026-09-22: a reattach naming the holder that claimed
/// the slot reclaims it without `takeover`. The incumbent there is the same
/// tab's own earlier socket, which a tunnel can keep half-open after a network
/// change. It is evicted by name, so its bridge ends. Any other client stays
/// refused: a different holder, no holder at all, or a slot claimed with none.
#[tokio::test]
async fn a_reattach_as_the_same_holder_reclaims_its_own_slot() {
    let dir = tempfile::tempdir().unwrap();
    let (port, _shutdown) = start_daemon(dir.path()).await;

    let launch = json!({ "repo": "owner/workbench", "agent": "claude", "holder": "tab-A" });
    let mut ws1 = page::console(port, launch).await;
    ws1.send(terminal(b"first\r")).await.unwrap();
    read_until(&mut ws1, "GOT:first", true).await;

    let (_, body) = http_request(port, "GET", "/api/sessions").await;
    let list: serde_json::Value = serde_json::from_str(&body).unwrap();
    let id = list.as_array().unwrap()[0]["id"].as_u64().unwrap();

    refused(
        port,
        json!({ "id": id }),
        "a reattach naming no holder is not the holder",
    )
    .await;
    refused(
        port,
        json!({ "id": id, "holder": "tab-B" }),
        "another tab is not the holder",
    )
    .await;
    // A malformed holder counts as none, so it cannot pass for tab-A.
    refused(
        port,
        json!({ "id": id, "holder": "tab-A&" }),
        "a malformed holder is no holder",
    )
    .await;

    // The launching tab reclaims its own slot without takeover.
    let mut ws2 = page::console(port, json!({ "id": id, "holder": "tab-A" })).await;
    let (ended, announced) = drain_capturing_announcement(&mut ws1, 5).await;
    assert!(ended, "the reclaimed socket's bridge must end");
    assert_eq!(announced.as_deref(), Some("taken-over"));
    assert_eq!(ws1.closed_reason(), Some("done"), "the daemon closed ws1");
    ws2.send(terminal(b"reclaimed\r")).await.unwrap();
    read_until(&mut ws2, "GOT:reclaimed", false).await;

    // The reclaim keeps the name: the slot is still tab-A's, and still nobody else's.
    refused(
        port,
        json!({ "id": id, "holder": "tab-B" }),
        "the reclaimed slot is tab-A's",
    )
    .await;
    drop(ws2);

    // A slot claimed with NO holder is reclaimable by nobody: `None` never matches.
    let mut anon = page::console(port, json!({ "id": id })).await;
    anon.send(terminal(b"anon\r")).await.unwrap();
    read_until(&mut anon, "GOT:anon", false).await;
    refused(
        port,
        json!({ "id": id, "holder": "tab-A" }),
        "a holder cannot claim a nameless slot",
    )
    .await;

    http_request(port, "POST", &format!("/api/sessions/close?id={id}")).await;
}

/// A page sends its last keys and closes the console channel at once
/// (#834). The console reads the keys before it ends, and the daemon sends
/// no `channel.closed`: the page asked for the close.
#[tokio::test]
async fn keys_sent_just_before_a_channel_close_reach_the_console() {
    let dir = tempfile::tempdir().unwrap();
    let (port, _shutdown) = start_daemon(dir.path()).await;

    let mut ws = page::console(
        port,
        json!({ "repo": "owner/workbench", "agent": "claude" }),
    )
    .await;
    ws.send(terminal(b"first\r")).await.unwrap();
    read_until(&mut ws, "GOT:first", true).await;
    let (_, body) = http_request(port, "GET", "/api/sessions").await;
    let list: serde_json::Value = serde_json::from_str(&body).unwrap();
    let id = list.as_array().unwrap()[0]["id"].as_u64().unwrap();

    ws.send(terminal(b"last\r")).await.unwrap();
    ws.close_channel().await.unwrap();
    let closed = tokio::time::timeout(Duration::from_millis(1500), async {
        while let Some(message) = ws.next().await {
            if matches!(message, Ok(Message::Close(_))) {
                return;
            }
        }
    })
    .await;
    assert!(
        closed.is_err(),
        "no channel.closed: {:?}",
        ws.closed_reason()
    );

    let mut again = page::console(port, json!({ "id": id })).await;
    read_until(&mut again, "GOT:last", false).await;
    http_request(port, "POST", &format!("/api/sessions/close?id={id}")).await;
}
