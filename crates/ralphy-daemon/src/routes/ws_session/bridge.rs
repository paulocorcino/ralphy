//! The byte bridge of an attached `/ws/session`: replay, live stream, input,
//! resize, and the eviction/exit teardown.

use std::ops::ControlFlow;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use axum::extract::ws::{Message, WebSocket};
use tokio::sync::broadcast::error::RecvError;

use super::outbound::{announce_end, open, resync, send_output, send_size, Opening};
use super::traffic::{Tab, Traffic};
use super::SessionLabels;
use crate::protocol::{Command, Frame};
use crate::{protocol, session};

/// How often the bridge pings its client, and how long the client may stay
/// silent before the bridge gives up on it (ADR-0051 §9 amendment 2026-09-22).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Liveness {
    pub(crate) ping_every: Duration,
    pub(crate) silent_after: Duration,
}

/// Test seam, like `RALPHY_DAEMON_AGENT_OVERRIDE`: the ping period in
/// milliseconds. A test cannot wait out the production window.
const PING_MS_ENV: &str = "RALPHY_DAEMON_WS_PING_MS";

impl Liveness {
    /// Two whole ping periods plus a quarter of slack: one lost pong on a lossy
    /// link is not a dead peer, two in a row is.
    fn from_ping(ping_every: Duration) -> Liveness {
        Liveness {
            ping_every,
            silent_after: ping_every * 9 / 4,
        }
    }

    /// 20 s pings (well under common 30–60 s proxy idle windows), so a silent
    /// client is released after 45 s.
    pub(crate) fn current() -> Liveness {
        static LIVENESS: OnceLock<Liveness> = OnceLock::new();
        *LIVENESS.get_or_init(|| {
            let ms = std::env::var(PING_MS_ENV)
                .ok()
                .and_then(|v| v.parse::<u64>().ok())
                .filter(|ms| *ms > 0)
                .unwrap_or(20_000);
            Liveness::from_ping(Duration::from_millis(ms))
        })
    }

    /// Whether a client last heard from `heard` ago has gone silent.
    pub(crate) fn is_silent(&self, heard: Duration) -> bool {
        heard >= self.silent_after
    }
}

/// Whether the bridge loop goes on. `Break` carries the DELIBERATE end to
/// announce (daemon shutdown, takeover/child-exit eviction, or the broadcast
/// sender closing). `Break(None)` means the loop fell out some other way
/// (client close, network drop, write failure) and the bridge stays SILENT:
/// announcing there would tell a client its session ended when it did not, and
/// the client would park instead of recovering the flaky link (issue #334).
type Flow = ControlFlow<Option<session::EndReason>>;

/// A failed send ends the loop without an announcement: the socket is gone.
fn silent_if_err<T, E>(sent: Result<T, E>) -> Flow {
    match sent {
        Ok(_) => ControlFlow::Continue(()),
        Err(_) => ControlFlow::Break(None),
    }
}

/// Bridge one WebSocket to one daemon-owned session (the tmux model, #166).
/// FIRST announces `session-open` with the hosting identity, then the session
/// size (`session-size`), then replays the screen snapshot and loops: session
/// output (via the broadcast `rx`) → `Frame::Terminal`; a new session size →
/// `session-size`; client `Frame::Terminal` → PTY stdin; client
/// `Frame::Command{verb:"resize"}` → PTY resize. The loop breaks on client
/// close/error, a send failure, an eviction (a `takeover` reattach OR the child
/// exiting), or daemon shutdown.
///
/// TEARDOWN INVARIANT (INVERTED vs #162): on EVERY exit path the bridge drops
/// `attach` — releasing the single-writer slot — and does NOT close the session.
/// A WebSocket drop detaches; the child survives it and a later reattach resumes
/// it. A session ends only via `POST /api/sessions/close` or its child exiting,
/// never because a browser tab closed.
///
/// LIVENESS: the browser answers every ping with a pong from its network stack,
/// even in a background tab, so a client that has sent NOTHING for
/// [`Liveness::silent_after`] is gone. A send succeeding proves nothing: a tunnel
/// agent (measured: TunnelDeck for dev tunnels, 2026-09-22) keeps its leg to the
/// daemon open for minutes after the browser behind it vanished, and that leg
/// would hold the writer slot against the client's own reattach.
/// `tab` is the browser tab that opened the socket.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn session_ws(
    mut socket: WebSocket,
    mut attach: session::Attachment,
    id: session::SessionId,
    daemon_id: String,
    environment: String,
    labels: SessionLabels,
    tab: Tab,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) {
    // Register the eviction waiter BEFORE the first await. Pin ONE `notified`
    // future across the whole loop (a fresh `notified()` per iteration could miss
    // an eviction firing mid-iteration), and `enable()` it up front so the waiter
    // is parked before the snapshot replay below: an eviction (a `takeover`
    // reattach or the child exiting) that fires during that replay `await` — or in
    // the `on_upgrade` scheduling gap — is delivered as a stored permit and breaks
    // the loop on the first poll, never lost. Missing this leaks the single-writer
    // slot AND hangs the bridge forever (the `Attachment` keeps `tx` alive, so
    // `rx.recv()` never returns `Closed`).
    let evict = attach.evict.clone();
    let notified = evict.notify.notified();
    tokio::pin!(notified);
    notified.as_mut().enable();
    let gzip = tab.gzip;
    let mut traffic = Traffic::local(id, tab, Instant::now());

    let opening = Opening::new(id, daemon_id, environment, &labels, gzip);
    if open(&mut socket, id, &opening, &mut attach, &mut traffic)
        .await
        .is_err()
    {
        traffic.log(None, Instant::now());
        return;
    }
    // Keep the socket warm on quiet/low-quality links: an idle terminal sends no
    // bytes, so a NAT/proxy idle-timeout silently drops it. The pongs the pings
    // earn are what `heard` counts (see LIVENESS above).
    let liveness = Liveness::current();
    let mut ping = tokio::time::interval(liveness.ping_every);
    ping.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    ping.tick().await; // consume the immediate first tick — no ping on connect
    let mut heard = Instant::now();
    // Each ping carries its own number, so a pong is timed against its ping.
    let mut ping_seq: u64 = 0;

    // False once the size sender is gone, so `changed()` cannot return at once
    // on every turn of the loop.
    let mut sizes = true;
    let end = loop {
        let flow = tokio::select! {
            _ = shutdown.changed() => {
                ControlFlow::Break(Some(session::EndReason::DaemonShutdown))
            }
            // Taken over, closed, or the child exited — the token carries which.
            _ = &mut notified => ControlFlow::Break(Some(
                evict.reason().unwrap_or(session::EndReason::ChildExited),
            )),
            _ = ping.tick() => {
                on_ping(&mut socket, &mut traffic, &liveness, heard, &mut ping_seq).await
            }
            changed = attach.size.changed(), if sizes => {
                on_size(changed, &mut socket, id, &mut attach, &mut sizes).await
            }
            recv = attach.rx.recv() => {
                on_output(recv, &mut socket, id, &opening, &mut attach, &mut traffic).await
            }
            incoming = socket.recv() => {
                on_client(incoming, &mut heard, &attach, &mut traffic)
            }
        };
        if let ControlFlow::Break(end) = flow {
            break end;
        }
    };
    // `None` announces nothing, and so does the early `return` above: the socket
    // is already gone there.
    if let Some(reason) = end {
        announce_end(&mut socket, id, &opening, &mut attach, &mut traffic, reason).await;
    }
    traffic.log(end.map(|r| r.as_wire()), Instant::now());
    // Detach, do NOT close: dropping `attach` releases the single-writer slot; the
    // session (and its child) live on for a later reattach.
    drop(attach);
}

/// Silent, not announced: a client that is in fact alive reads the drop as a
/// flaky link and reattaches (issue #334).
async fn on_ping(
    socket: &mut WebSocket,
    traffic: &mut Traffic,
    liveness: &Liveness,
    heard: Instant,
    ping_seq: &mut u64,
) -> Flow {
    if liveness.is_silent(heard.elapsed()) {
        return ControlFlow::Break(None);
    }
    *ping_seq += 1;
    let payload = ping_seq.to_le_bytes();
    traffic.ping_sent(&payload, Instant::now());
    silent_if_err(socket.send(Message::Ping(payload.to_vec().into())).await)
}

/// A new session size goes to the client. A closed size sender stops the arm.
async fn on_size(
    changed: Result<(), tokio::sync::watch::error::RecvError>,
    socket: &mut WebSocket,
    id: session::SessionId,
    attach: &mut session::Attachment,
    sizes: &mut bool,
) -> Flow {
    if changed.is_err() {
        *sizes = false;
        return ControlFlow::Continue(());
    }
    let size = *attach.size.borrow_and_update();
    silent_if_err(send_size(socket, id, size).await)
}

/// One turn of the live output stream. A lag sends the screen again.
async fn on_output(
    recv: Result<Vec<u8>, RecvError>,
    socket: &mut WebSocket,
    id: session::SessionId,
    opening: &Opening,
    attach: &mut session::Attachment,
    traffic: &mut Traffic,
) -> Flow {
    match recv {
        Ok(bytes) => silent_if_err(send_output(socket, id, bytes, traffic).await),
        Err(RecvError::Lagged(skipped)) => {
            traffic.lagged(skipped);
            silent_if_err(resync(socket, id, opening, attach, traffic).await)
        }
        Err(RecvError::Closed) => ControlFlow::Break(Some(session::EndReason::ChildExited)),
    }
}

/// One message from the client. Every message that arrives, of any kind,
/// counts as `heard`.
fn on_client(
    incoming: Option<Result<Message, axum::Error>>,
    heard: &mut Instant,
    attach: &session::Attachment,
    traffic: &mut Traffic,
) -> Flow {
    if matches!(incoming, Some(Ok(_))) {
        *heard = Instant::now();
    }
    match incoming {
        Some(Ok(Message::Binary(bytes))) => {
            traffic.inbound(bytes.len());
            on_client_frame(&bytes, attach, traffic)
        }
        Some(Ok(Message::Close(_))) => {
            traffic.client_closed();
            ControlFlow::Break(None)
        }
        Some(Ok(Message::Pong(payload))) => {
            traffic.pong_received(&payload, Instant::now());
            ControlFlow::Continue(())
        }
        // Text and ping: counted in `heard`, nothing else.
        Some(Ok(_)) => ControlFlow::Continue(()),
        None | Some(Err(_)) => ControlFlow::Break(None),
    }
}

fn on_client_frame(bytes: &[u8], attach: &session::Attachment, traffic: &mut Traffic) -> Flow {
    match protocol::decode(bytes) {
        Ok(Frame::Terminal { data, .. }) => silent_if_err(attach.write(&data)),
        Ok(Frame::Command(cmd)) if cmd.verb == "resize" => {
            if let Some((rows, cols)) = resize_of(&cmd) {
                let _ = attach.resize(rows, cols);
            }
            ControlFlow::Continue(())
        }
        Ok(Frame::Command(cmd)) => {
            traffic.client_command(&cmd);
            ControlFlow::Continue(())
        }
        // Other frames carry no session meaning here.
        _ => ControlFlow::Continue(()),
    }
}

/// The size a `resize` command asks for. `try_into` rejects a garbage or
/// oversized dimension rather than truncating it into a wrong terminal size.
fn resize_of(cmd: &Command) -> Option<(u16, u16)> {
    let dimension = |key: &str| -> Option<u16> {
        cmd.payload
            .get(key)
            .and_then(|v| v.as_u64()?.try_into().ok())
    };
    Some((dimension("rows")?, dimension("cols")?))
}

#[cfg(test)]
mod tests {
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
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
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
}
