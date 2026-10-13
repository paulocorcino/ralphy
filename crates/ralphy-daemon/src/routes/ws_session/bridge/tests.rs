use super::*;
use crate::routes::ws_session::outbound::SESSION_SIZE;
use crate::routes::ws_session::traffic::SESSION_OPEN;
use std::sync::{Arc, Mutex};

use axum::extract::ws::WebSocketUpgrade;
use futures_util::StreamExt;
use tokio_tungstenite::tungstenite;

/// A console whose child has exited: the eviction is signalled and the
/// attachment's output stream is the test's own, holding `chunks`, so the
/// client receives exactly these and nothing the shell prints.
fn exited_session(
    chunks: &[Vec<u8>],
    capacity: usize,
) -> (session::SessionId, session::Attachment) {
    let manager = Arc::new(session::SessionManager::new());
    let spec = session::console_spec(std::env::temp_dir(), 24, 80, None);
    let (id, mut attach) = manager
        .spawn_attached(
            "~".to_string(),
            "console".to_string(),
            "console".to_string(),
            None,
            None,
            None,
            spec,
        )
        .expect("the platform shell must spawn — the free console depends on it");
    let (tx, rx) = tokio::sync::broadcast::channel::<Vec<u8>>(capacity);
    attach.rx = rx;
    attach.snapshot.clear();
    for chunk in chunks {
        tx.send(chunk.clone())
            .expect("the attachment holds a receiver");
    }
    assert!(manager.close(id), "the session was live");
    assert_eq!(
        attach.evict.reason(),
        Some(session::EndReason::ChildExited),
        "the eviction is signalled before the bridge starts"
    );
    (id, attach)
}

/// One in-memory connection, accepted once. The bytes move between tasks
/// without the kernel, so paused time cannot move forward while bytes
/// are still on their way, as it can with a loopback socket.
struct OneConnection(Option<tokio::io::DuplexStream>);

impl axum::serve::Listener for OneConnection {
    type Io = tokio::io::DuplexStream;
    type Addr = ();

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        match self.0.take() {
            Some(io) => (io, ()),
            None => std::future::pending().await,
        }
    }

    fn local_addr(&self) -> std::io::Result<Self::Addr> {
        Ok(())
    }
}

/// Serve `session_ws` for one client on `listener`; `gzip` is what the
/// client's tab asked for.
fn serve_bridge<L>(
    listener: L,
    id: session::SessionId,
    attach: session::Attachment,
    gzip: bool,
) -> tokio::task::JoinHandle<std::io::Result<()>>
where
    L: axum::serve::Listener,
    L::Addr: std::fmt::Debug,
{
    let slot = Arc::new(Mutex::new(Some(attach)));
    let (stop, shutdown) = tokio::sync::watch::channel(false);
    let app = axum::Router::new().route(
        "/",
        axum::routing::get(move |ws: WebSocketUpgrade| {
            let attach = slot
                .lock()
                .expect("slot mutex")
                .take()
                .expect("one client connects");
            let shutdown = shutdown.clone();
            async move {
                ws.on_upgrade(move |socket| {
                    session_ws(
                        socket,
                        attach,
                        id,
                        "daemon".to_string(),
                        "test".to_string(),
                        SessionLabels {
                            name: None,
                            checkout: None,
                            watching: false,
                        },
                        Tab {
                            gzip,
                            ..Tab::default()
                        },
                        shutdown,
                    )
                })
            }
        }),
    );
    tokio::spawn(async move {
        // The sender stays alive as long as the server runs.
        let _stop = stop;
        axum::serve(listener, app).await
    })
}

/// What a client saw: the output, where `session-end` came in it, and
/// whether the `Close` frame followed.
struct Seen {
    received: Vec<u8>,
    ended_after: Option<usize>,
    closed: bool,
}

/// Read until the `Close` frame, pausing `pause` after every message.
async fn read_until_close(
    client: &mut (impl futures_util::Stream<Item = Result<tungstenite::Message, tungstenite::Error>>
              + Unpin),
    pause: Duration,
) -> Seen {
    let mut seen = Seen {
        received: Vec::new(),
        ended_after: None,
        closed: false,
    };
    // The bridge drops the socket right after its Close frame, so the
    // read that follows the Close can fail: stop at the Close.
    while let Some(Ok(message)) = client.next().await {
        let bytes = match message {
            tungstenite::Message::Binary(bytes) => bytes,
            tungstenite::Message::Close(_) => {
                seen.closed = true;
                break;
            }
            _ => continue,
        };
        match protocol::decode(&bytes).expect("a well-formed frame") {
            Frame::Terminal { data, .. } => {
                assert!(
                    seen.ended_after.is_none(),
                    "output arrived after session-end"
                );
                seen.received.extend_from_slice(&data);
            }
            Frame::Command(cmd) if cmd.verb == "session-end" => {
                assert_eq!(cmd.payload["reason"], "child-exited");
                seen.ended_after = Some(seen.received.len());
            }
            _ => {}
        }
        tokio::time::sleep(pause).await;
    }
    seen
}

/// A child that exits leaves its last output queued on the attachment
/// while the eviction is already signalled. Every queued chunk reaches the
/// client before `session-end`. With both arms ready, an unbiased select
/// takes the eviction about half the time, so a loop that drops queued
/// output keeps all 32 chunks with a chance near 2^-32.
#[tokio::test]
async fn queued_output_reaches_the_client_before_the_session_end() {
    let chunks: Vec<Vec<u8>> = (0..32)
        .map(|n| format!("line {n}\r\n").into_bytes())
        .collect();
    let (id, attach) = exited_session(&chunks, 64);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind a loopback port");
    let addr = listener
        .local_addr()
        .expect("a bound listener has an address");
    let server = serve_bridge(listener, id, attach, false);

    let (mut client, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/"))
        .await
        .expect("the bridge accepts the socket");
    let seen = tokio::time::timeout(
        Duration::from_secs(10),
        read_until_close(&mut client, Duration::ZERO),
    )
    .await
    .expect("the bridge closes the socket after session-end");
    server.abort();

    assert_eq!(
        String::from_utf8_lossy(&seen.received),
        String::from_utf8_lossy(&chunks.concat()),
        "every queued chunk reaches the client"
    );
    assert!(seen.ended_after.is_some(), "the end is announced");
}

/// A client that takes the queue too slowly cannot hold back the end:
/// the drain stops at its own limit, so `session-end` and `Close` are
/// still sent inside the 5 s end window. The connection is a 4 KiB pipe
/// and the client reads one chunk per 100 ms, so 100 chunks need about
/// 10 s. Time is paused: tokio moves the clock forward only while every
/// task waits, so the limits are measured in test time, not in the speed
/// of the machine.
#[tokio::test(start_paused = true)]
async fn a_drain_that_cannot_finish_still_sends_the_end_and_the_close() {
    let chunks: Vec<Vec<u8>> = (0..100u8).map(|n| vec![n; 64 * 1024]).collect();
    let (id, attach) = exited_session(&chunks, 128);
    let (server_io, client_io) = tokio::io::duplex(4096);
    let server = serve_bridge(OneConnection(Some(server_io)), id, attach, false);

    let (mut client, _) = tokio_tungstenite::client_async("ws://localhost/", client_io)
        .await
        .expect("the bridge upgrades the connection");
    let seen = tokio::time::timeout(
        Duration::from_secs(15),
        read_until_close(&mut client, Duration::from_millis(100)),
    )
    .await
    .expect("the bridge closes the socket inside its end window");
    server.abort();

    assert!(seen.ended_after.is_some(), "the end is announced");
    assert!(seen.closed, "the Close frame follows the end");
    assert!(
        seen.received.len() < chunks.concat().len(),
        "the drain stopped at its limit instead of sending everything"
    );
}

/// The next binary frame the client gets, decoded.
async fn next_frame(
    client: &mut (impl futures_util::Stream<Item = Result<tungstenite::Message, tungstenite::Error>>
              + Unpin),
) -> Frame {
    loop {
        let message = tokio::time::timeout(Duration::from_secs(10), client.next())
            .await
            .expect("a frame within 10 s")
            .expect("the socket is open")
            .expect("a well-formed message");
        if let tungstenite::Message::Binary(bytes) = message {
            return protocol::decode(&bytes).expect("a well-formed frame");
        }
    }
}

/// The session size as a frame says it, or `None` for another frame.
fn size_of(frame: &Frame) -> Option<(u64, u64)> {
    match frame {
        Frame::Command(cmd) if cmd.verb == SESSION_SIZE => {
            Some((cmd.payload["rows"].as_u64()?, cmd.payload["cols"].as_u64()?))
        }
        _ => None,
    }
}

/// A watcher's socket gets `session-size` after `session-open` and before
/// the replay, and again when the writer resizes the session (ADR-0079 D6).
/// The first size is the one the replay was taken at, also when a resize
/// came between the attach and the open; that resize follows the replay.
#[tokio::test]
async fn a_watcher_gets_the_size_before_the_replay_and_on_each_resize() {
    let manager = Arc::new(session::SessionManager::new());
    let spec = session::console_spec(std::env::temp_dir(), 24, 80, None);
    let (id, writer) = manager
        .spawn_attached(
            "~".to_string(),
            "console".to_string(),
            "console".to_string(),
            None,
            None,
            None,
            spec,
        )
        .expect("the platform shell must spawn — the free console depends on it");
    let mut watcher = manager
        .watch(id, None)
        .expect("a live session is watchable");
    watcher.snapshot = b"REPLAY".to_vec();
    writer.resize(28, 90).expect("the PTY takes a resize");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind a loopback port");
    let addr = listener
        .local_addr()
        .expect("a bound listener has an address");
    let server = serve_bridge(listener, id, watcher, false);
    let (mut client, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/"))
        .await
        .expect("the bridge accepts the socket");

    let open = next_frame(&mut client).await;
    assert!(
        matches!(&open, Frame::Command(cmd) if cmd.verb == SESSION_OPEN),
        "the first frame is session-open: {open:?}"
    );
    assert_eq!(size_of(&next_frame(&mut client).await), Some((24, 80)));
    match next_frame(&mut client).await {
        Frame::Terminal { data, .. } => assert_eq!(data, b"REPLAY"),
        other => panic!("the replay follows the size: {other:?}"),
    }
    let after_replay = loop {
        if let Some(size) = size_of(&next_frame(&mut client).await) {
            break size;
        }
    };
    assert_eq!(after_replay, (28, 90));

    writer.resize(30, 100).expect("the PTY takes a resize");
    let resized = loop {
        if let Some(size) = size_of(&next_frame(&mut client).await) {
            break size;
        }
    };
    assert_eq!(resized, (30, 100));
    server.abort();
    manager.close(id);
}

/// A watcher of a live console session, changed by `edit` and bridged on
/// a loopback socket; `gzip` is what the tab asked for.
async fn bridged_watcher(
    gzip: bool,
    edit: impl FnOnce(&mut session::Attachment),
) -> (
    Arc<session::SessionManager>,
    session::SessionId,
    tokio::task::JoinHandle<std::io::Result<()>>,
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
) {
    let manager = Arc::new(session::SessionManager::new());
    let spec = session::console_spec(std::env::temp_dir(), 24, 80, None);
    let (id, writer) = manager
        .spawn_attached(
            "~".to_string(),
            "console".to_string(),
            "console".to_string(),
            None,
            None,
            None,
            spec,
        )
        .expect("the platform shell must spawn — the free console depends on it");
    // The writer leaves; the session lives on, watched.
    drop(writer);
    let mut watcher = manager
        .watch(id, None)
        .expect("a live session is watchable");
    edit(&mut watcher);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind a loopback port");
    let addr = listener
        .local_addr()
        .expect("a bound listener has an address");
    let server = serve_bridge(listener, id, watcher, gzip);
    let (client, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/"))
        .await
        .expect("the bridge accepts the socket");
    (manager, id, server, client)
}

/// `session-open` names the byte count at the end of the replay and
/// whether the replay resumes the tab's terminal; an empty resume sends
/// no replay frame.
#[tokio::test]
async fn session_open_says_the_offset_and_whether_the_replay_resumes() {
    for resume in [true, false] {
        let (manager, id, server, mut client) = bridged_watcher(false, |watcher| {
            watcher.snapshot.clear();
            watcher.offset = 4096;
            watcher.resume = resume;
        })
        .await;

        let Frame::Command(open) = next_frame(&mut client).await else {
            panic!("the first frame is session-open");
        };
        assert_eq!(open.payload["offset"], serde_json::json!(4096));
        assert_eq!(open.payload["resume"], serde_json::json!(resume));
        assert_eq!(open.payload["replay"], serde_json::json!(false));
        server.abort();
        manager.close(id);
    }
}

/// The replay a tab gets: gzipped and named so in `session-open` when it
/// asks for gzip, raw with no `replay_encoding` when it does not.
#[tokio::test]
async fn the_replay_is_gzipped_only_when_the_tab_asks() {
    use std::io::Read;
    let snapshot = b"\x1b[2J\x1b[Hthe screen as it is now\r\n".repeat(50);
    for gzip in [true, false] {
        let (manager, id, server, mut client) =
            bridged_watcher(gzip, |watcher| watcher.snapshot = snapshot.clone()).await;

        let Frame::Command(open) = next_frame(&mut client).await else {
            panic!("the first frame is session-open");
        };
        let encoding = open.payload.get("replay_encoding").cloned();
        let size = next_frame(&mut client).await;
        assert!(size_of(&size).is_some(), "the size follows: {size:?}");
        let Frame::Terminal { data, .. } = next_frame(&mut client).await else {
            panic!("the replay follows the size");
        };
        if gzip {
            assert_eq!(encoding, Some(serde_json::json!("gzip")));
            let mut raw = Vec::new();
            flate2::read::GzDecoder::new(data.as_slice())
                .read_to_end(&mut raw)
                .expect("the replay is gzip");
            assert_eq!(raw, snapshot);
            assert!(data.len() < snapshot.len(), "the gzip is smaller");
        } else {
            assert_eq!(encoding, None, "a raw replay names no encoding");
            assert_eq!(data, snapshot);
        }
        server.abort();
        manager.close(id);
    }
}

/// A receiver that lagged makes the bridge start the page again: a new
/// `session-open` with `resume: false`, the size, the screen snapshot,
/// then the live bytes from the new offset, with no gap and no repeat.
#[tokio::test]
async fn a_lagged_receiver_sends_the_screen_again_from_a_new_offset() {
    let (lag, lagging) = tokio::sync::broadcast::channel::<Vec<u8>>(1);
    let (manager, id, server, mut client) = bridged_watcher(false, |watcher| {
        watcher.snapshot = b"OLD".to_vec();
        watcher.rx = lagging;
    })
    .await;
    for chunk in [b"lost".to_vec(), b"kept".to_vec()] {
        lag.send(chunk).expect("the bridge holds a receiver");
    }
    for _ in 0..3 {
        next_frame(&mut client).await; // the first open, size and replay
    }

    let Frame::Command(open) = next_frame(&mut client).await else {
        panic!("a lag sends session-open again");
    };
    assert_eq!(open.verb, SESSION_OPEN);
    assert_eq!(open.payload["resume"], serde_json::json!(false));
    let offset = open.payload["offset"].as_u64().expect("an offset");
    assert!(size_of(&next_frame(&mut client).await).is_some());
    if open.payload["replay"] == serde_json::json!(true) {
        let Frame::Terminal { data, .. } = next_frame(&mut client).await else {
            panic!("the snapshot follows the size");
        };
        assert_ne!(data, b"OLD", "the snapshot of now, not the first replay");
    }
    // The shell echoes what is typed: output after the resync.
    let writer = manager.attach(id, false).expect("the writer slot is free");
    writer
        .write(b"echo resync\r")
        .expect("the shell takes input");
    // The session's bytes after `offset`, as a resume at it would get them.
    let mut tail = manager.watch(id, Some(offset)).expect("a live session");
    for _ in 0..200 {
        if tail.resume && !tail.snapshot.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
        tail = manager.watch(id, Some(offset)).expect("a live session");
    }
    assert!(tail.resume && !tail.snapshot.is_empty(), "the shell wrote");
    let mut live = Vec::new();
    while live.len() < tail.snapshot.len() {
        match next_frame(&mut client).await {
            Frame::Terminal { data, .. } => live.extend(data),
            other => panic!("only live output follows: {other:?}"),
        }
    }
    assert_eq!(live[..tail.snapshot.len()], tail.snapshot[..]);
    drop(lag);
    server.abort();
    manager.close(id);
}

fn resize_command(payload: serde_json::Value) -> Command {
    Command {
        id: 0,
        verb: "resize".to_string(),
        payload,
    }
}

#[test]
fn a_resize_names_both_dimensions_as_u16() {
    let cmd = resize_command(serde_json::json!({ "rows": 40, "cols": 100 }));
    assert_eq!(resize_of(&cmd), Some((40, 100)));
}

#[test]
fn a_resize_with_a_missing_or_garbage_dimension_is_ignored() {
    for payload in [
        serde_json::json!({ "rows": 40 }),
        serde_json::json!({ "cols": 100 }),
        serde_json::json!({ "rows": 40, "cols": 70000 }),
        serde_json::json!({ "rows": -1, "cols": 100 }),
        serde_json::json!({ "rows": "40", "cols": 100 }),
    ] {
        assert_eq!(
            resize_of(&resize_command(payload.clone())),
            None,
            "{payload}"
        );
    }
}
