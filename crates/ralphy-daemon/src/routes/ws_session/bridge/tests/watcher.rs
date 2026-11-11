use super::*;

/// The next inner frame the client gets, decoded.
async fn next_frame(
    client: &mut (impl futures_util::Stream<Item = Result<tungstenite::Message, tungstenite::Error>>
              + Unpin),
) -> Frame {
    let inner = tokio::time::timeout(Duration::from_secs(10), next_inner(client))
        .await
        .expect("a frame within 10 s")
        .expect("the socket is open");
    protocol::decode(&inner).expect("a well-formed inner frame")
}

/// The session size as a frame says it, or `None` for another frame.
fn size_of(frame: &Frame) -> Option<(u64, u64)> {
    match frame {
        Frame::Command(cmd) if cmd.verb == SESSION_SIZE => {
            Some((cmd.payload["rows"].as_u64()?, cmd.payload["cols"].as_u64()?))
        }
        Frame::Command(_) | Frame::Terminal { .. } | Frame::Presence(_) | Frame::Channel { .. } => {
            None
        }
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
            "~".to_owned(),
            "console".to_owned(),
            "console".to_owned(),
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
        other @ (Frame::Command(_) | Frame::Presence(_) | Frame::Channel { .. }) => {
            panic!("the replay follows the size: {other:?}")
        }
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
            "~".to_owned(),
            "console".to_owned(),
            "console".to_owned(),
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
            other @ (Frame::Command(_) | Frame::Presence(_) | Frame::Channel { .. }) => {
                panic!("only live output follows: {other:?}")
            }
        }
    }
    assert_eq!(live[..tail.snapshot.len()], tail.snapshot[..]);
    drop(lag);
    server.abort();
    manager.close(id);
}
