//! `/ws`: the presence heartbeat.

use std::path::Path;
use std::time::{Duration, Instant};

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::routing::get;
use axum::Router;
use tokio::sync::broadcast;

use super::RouterShared;
use crate::protocol::{Command, Frame, Presence};
use crate::{identity, presence_board, protocol};

/// A change to a shown fact the daemon owns, relayed on every open `/ws` as a
/// `<fact>.dirty` command with id 0 (ADR-0070 D2 event 1). `/ws` carries them
/// because it is the one socket every tab keeps open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Push {
    /// `PUT /api/desk` changed the desk; `tab` is the writer's tab id, so
    /// that tab does not read its own write again.
    Desk { tab: Option<String> },
    /// `repos.toml` changed on disk (`ralphy daemon add` writes it from
    /// another process).
    Repos,
    /// The peer store changed on disk (`ralphy host …` writes it from
    /// another process).
    Peers,
    /// The attention list changed (`attention::tick`).
    Attention,
}

impl Push {
    fn verb(&self) -> &'static str {
        match self {
            Push::Desk { .. } => "desk.dirty",
            Push::Repos => "repos.dirty",
            Push::Peers => "peers.dirty",
            Push::Attention => "attention.dirty",
        }
    }

    fn payload(&self) -> serde_json::Value {
        match self {
            Push::Desk { tab } => serde_json::json!({ "tab": tab }),
            Push::Repos | Push::Peers | Push::Attention => serde_json::json!({}),
        }
    }

    /// Every push once: what a receiver that lagged sends, because a read
    /// again is harmless and a lost push is not.
    fn all() -> [Push; 4] {
        [
            Push::Desk { tab: None },
            Push::Repos,
            Push::Peers,
            Push::Attention,
        ]
    }
}

/// Capacity of the daemon-wide [`Push`] bus.
pub(crate) const PUSH_CAP: usize = 32;

/// Put a push on the bus. A send with no open `/ws` is `Err`, and a push no
/// tab hears needs no message: a tab that opens reads every fact anyway.
pub(crate) fn push(pushes: &broadcast::Sender<Push>, p: Push) {
    if pushes.send(p).is_err() {
        // No open `/ws`: nothing to tell.
    }
}

/// How often [`watch_stores`] looks at the files other processes write.
pub(crate) const STORE_STAMP_EVERY: Duration = Duration::from_secs(2);

/// A stamp of one file: its change time and length, or why it cannot be
/// read. Two equal stamps mean "no change seen".
pub(crate) fn store_stamp(path: &Path) -> String {
    match std::fs::metadata(path) {
        Ok(m) => {
            let modified = m
                .modified()
                .map(|t| format!("{t:?}"))
                .unwrap_or_else(|e| e.to_string());
            format!("{modified}/{}", m.len())
        }
        Err(e) => format!("error: {e}"),
    }
}

/// A stamp of a directory: the sorted name, change time and length of each
/// entry, or why it cannot be read.
pub(crate) fn peers_stamp(dir: &Path) -> String {
    match std::fs::read_dir(dir) {
        Ok(entries) => {
            let mut rows: Vec<String> = entries
                .map(|entry| match entry {
                    Ok(entry) => format!(
                        "{}:{}",
                        entry.file_name().to_string_lossy(),
                        store_stamp(&entry.path())
                    ),
                    Err(e) => format!("error: {e}"),
                })
                .collect();
            rows.sort();
            rows.join("\n")
        }
        Err(e) => format!("error: {e}"),
    }
}

/// Push `repos.dirty` / `peers.dirty` when `repos.toml` or the peer store
/// changes on disk. Another process writes both files, so only the files show
/// the change; a stat every [`STORE_STAMP_EVERY`] works the same on every OS
/// and needs no watcher thread. Ends on daemon shutdown.
pub(crate) async fn watch_stores(
    registry_path: std::path::PathBuf,
    peers_dir: std::path::PathBuf,
    pushes: broadcast::Sender<Push>,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) {
    let stamp = |r: std::path::PathBuf, p: std::path::PathBuf| {
        tokio::task::spawn_blocking(move || (store_stamp(&r), peers_stamp(&p)))
    };
    let mut last = match stamp(registry_path.clone(), peers_dir.clone()).await {
        Ok(stamps) => stamps,
        Err(e) => {
            tracing::warn!(error = %e, "the store watch stopped; repos and peers are no longer pushed");
            return;
        }
    };
    let mut tick = tokio::time::interval(STORE_STAMP_EVERY);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    tick.tick().await;
    loop {
        tokio::select! {
            _ = shutdown.changed() => break,
            _ = tick.tick() => {
                let now = match stamp(registry_path.clone(), peers_dir.clone()).await {
                    Ok(stamps) => stamps,
                    Err(e) => {
                        tracing::warn!(error = %e, "the store watch stopped; repos and peers are no longer pushed");
                        break;
                    }
                };
                if now.0 != last.0 {
                    push(&pushes, Push::Repos);
                }
                if now.1 != last.1 {
                    push(&pushes, Push::Peers);
                }
                last = now;
            }
        }
    }
}

async fn send_push(socket: &mut WebSocket, verb: &str, payload: serde_json::Value) -> bool {
    let frame = Frame::Command(Command {
        id: 0,
        verb: verb.to_string(),
        payload,
    });
    socket
        .send(Message::Binary(protocol::encode(&frame).into()))
        .await
        .is_ok()
}

/// Build the presence heartbeat for the loaded identity and the daemon's
/// current uptime. `None` identity → a heartbeat with no name/avatar (the
/// daemon is alive but un-baptized).
pub(crate) fn build_presence(identity: Option<&identity::Identity>, uptime: Duration) -> Frame {
    Frame::Presence(Presence {
        name: identity.map(|i| i.name.clone()),
        avatar: identity.map(|i| i.avatar.clone()),
        uptime_secs: uptime.as_secs(),
        build: Some(crate::assets::build_id().to_string()),
    })
}

/// Push a presence heartbeat to a connected client every 2s until it hangs up
/// or the daemon shuts down. The send loop MUST exit on every teardown path —
/// a `None`/`Close`/error from the client (the `recv` arm) OR a daemon shutdown
/// (the `shutdown` arm) — and drop the socket, so no task keeps sending after a
/// disconnect and a held-open connection cannot stall graceful shutdown.
///
/// Between heartbeats it relays `sessions.dirty` from `sessions` and every
/// [`Push`] from `pushes`. A receiver that lagged sends every verb once; a
/// closed one turns its arm off, so the loop never spins on it.
///
/// The page sends `presence.report` on the same socket (docs/adr/0085 D14):
/// each one goes on `board`, under this socket's entry, which goes when the
/// loop ends. A frame that is not a report is ignored, and the socket stays
/// open.
pub(crate) async fn ws_presence_loop(
    mut socket: WebSocket,
    identity: Option<identity::Identity>,
    start: Instant,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
    (mut sessions, mut pushes): (broadcast::Receiver<()>, broadcast::Receiver<Push>),
    board: presence_board::Shared,
) {
    let page = PageEntry::open(board);
    let mut tick = tokio::time::interval(Duration::from_secs(2));
    let mut sessions_open = true;
    let mut pushes_open = true;
    loop {
        let stop = tokio::select! {
            changed = sessions.recv(), if sessions_open => {
                on_sessions(&mut socket, changed).await.stops(&mut sessions_open)
            }
            pushed = pushes.recv(), if pushes_open => {
                on_push(&mut socket, pushed).await.stops(&mut pushes_open)
            }
            // Ok = the daemon signalled shutdown; Err = the sender was dropped
            // (its runtime is going away). Either way, stop serving this socket.
            _ = shutdown.changed() => true,
            _ = tick.tick() => !send_presence(&mut socket, identity.as_ref(), start).await,
            incoming = socket.recv() => {
                page.take(&incoming);
                ends_the_loop(&incoming)
            }
        };
        if stop {
            break;
        }
    }
}

/// The entry of one `/ws` socket on the presence board, removed when the
/// socket's loop ends, on every path.
struct PageEntry {
    board: presence_board::Shared,
    conn: Option<u64>,
}

impl PageEntry {
    /// A poisoned board is logged, and this socket's reports are not kept.
    fn open(board: presence_board::Shared) -> Self {
        let conn = match board.lock() {
            Ok(mut b) => Some(b.open()),
            Err(e) => {
                tracing::warn!(error = %e, "the presence board is poisoned");
                None
            }
        };
        PageEntry { board, conn }
    }

    /// Put a `presence.report` from the page on the board. Anything else
    /// the page sends is ignored.
    fn take(&self, incoming: &Option<Result<Message, axum::Error>>) {
        let (Some(conn), Some(Ok(Message::Binary(bytes)))) = (self.conn, incoming) else {
            return;
        };
        let Ok(Frame::Command(command)) = protocol::decode(bytes) else {
            return;
        };
        if command.verb != "presence.report" {
            return;
        }
        if let Ok(mut board) = self.board.lock() {
            board.report(conn, &command.payload, std::time::SystemTime::now());
        }
    }
}

impl Drop for PageEntry {
    fn drop(&mut self) {
        let Some(conn) = self.conn else {
            return;
        };
        match self.board.lock() {
            Ok(mut board) => board.close(conn),
            Err(e) => tracing::warn!(error = %e, "the presence board is poisoned"),
        }
    }
}

/// What one channel arm of [`ws_presence_loop`] tells the loop to do next.
enum Next {
    /// Wait for the next event.
    Go,
    /// The channel is closed: turn its arm off and go on.
    CloseArm,
    /// A send failed: end the loop.
    Stop,
}

impl Next {
    /// Whether the loop ends; a closed arm sets its `open` flag to false.
    fn stops(self, open: &mut bool) -> bool {
        match self {
            Next::Go => false,
            Next::CloseArm => {
                *open = false;
                false
            }
            Next::Stop => true,
        }
    }

    /// `Go` when the send worked, `Stop` when it failed.
    fn sent(ok: bool) -> Next {
        if ok {
            Next::Go
        } else {
            Next::Stop
        }
    }
}

/// The sessions arm: a change, or a lag, is one `sessions.dirty`.
async fn on_sessions(
    socket: &mut WebSocket,
    changed: Result<(), broadcast::error::RecvError>,
) -> Next {
    match changed {
        Ok(()) | Err(broadcast::error::RecvError::Lagged(_)) => {
            Next::sent(send_push(socket, "sessions.dirty", serde_json::json!({})).await)
        }
        Err(broadcast::error::RecvError::Closed) => Next::CloseArm,
    }
}

/// The pushes arm: a push is sent as its verb; a lag sends every verb once.
async fn on_push(
    socket: &mut WebSocket,
    pushed: Result<Push, broadcast::error::RecvError>,
) -> Next {
    match pushed {
        Ok(p) => Next::sent(send_push(socket, p.verb(), p.payload()).await),
        Err(broadcast::error::RecvError::Lagged(_)) => Next::sent(send_every_push(socket).await),
        Err(broadcast::error::RecvError::Closed) => Next::CloseArm,
    }
}

/// `sessions.dirty` and every [`Push`] once; no send after the first one that
/// fails. `false` when a send failed.
async fn send_every_push(socket: &mut WebSocket) -> bool {
    let mut sent = send_push(socket, "sessions.dirty", serde_json::json!({})).await;
    for p in Push::all() {
        sent = sent && send_push(socket, p.verb(), p.payload()).await;
    }
    sent
}

/// Send one presence heartbeat. `false` when the send failed.
async fn send_presence(
    socket: &mut WebSocket,
    identity: Option<&identity::Identity>,
    start: Instant,
) -> bool {
    let frame = build_presence(identity, start.elapsed());
    socket
        .send(Message::Binary(protocol::encode(&frame).into()))
        .await
        .is_ok()
}

/// Whether a message from the client ends the loop. None (stream closed), a
/// Close frame, or a recv error all end it; the socket drops when the loop's
/// task returns.
fn ends_the_loop(incoming: &Option<Result<Message, axum::Error>>) -> bool {
    match incoming {
        Some(Ok(Message::Close(_))) | None => true,
        Some(Err(_)) => true,
        Some(Ok(_)) => false,
    }
}

/// `/ws`: the presence socket.
pub(crate) fn presence_routes(s: &RouterShared) -> Router {
    let identity = s.identity.clone();
    let start = s.start;
    let shutdown = s.shutdown.clone();
    let sessions = s.sessions.clone();
    let pushes = s.pushes.clone();
    let board = s.presence.clone();
    Router::new().route(
        "/ws",
        get(move |ws: WebSocketUpgrade| {
            let id = identity.clone();
            let shutdown = shutdown.clone();
            let receivers = (sessions.subscribe_changes(), pushes.subscribe());
            let board = board.clone();
            async move {
                // The page sends only its presence reports here.
                ws.max_message_size(presence_board::MAX_REPORT_BYTES)
                    .max_frame_size(presence_board::MAX_REPORT_BYTES)
                    .on_upgrade(move |socket| {
                        ws_presence_loop(socket, id, start, shutdown, receivers, board)
                    })
            }
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stamp_changes_when_the_store_changes() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("repos.toml");
        std::fs::write(&file, "a = 1\n").unwrap();
        let first = store_stamp(&file);
        assert_eq!(
            store_stamp(&file),
            first,
            "an unchanged file keeps its stamp"
        );
        std::fs::write(&file, "a = 1\nb = 2\n").unwrap();
        assert_ne!(store_stamp(&file), first, "a rewrite changes the stamp");

        let peers = dir.path().join("peers");
        let empty = peers_stamp(&peers);
        std::fs::create_dir(&peers).unwrap();
        std::fs::write(peers.join("x.toml"), "x").unwrap();
        let one = peers_stamp(&peers);
        assert_ne!(one, empty, "a new descriptor changes the stamp");
        assert_eq!(peers_stamp(&peers), one);
    }

    type Ws = tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >;

    /// Run one `ws_presence_loop` on `sessions` and `pushes` behind a loopback
    /// `/ws`, and connect to it. The returned sender holds the shutdown open.
    async fn connect(
        sessions: broadcast::Receiver<()>,
        pushes: broadcast::Receiver<Push>,
    ) -> (Ws, tokio::sync::watch::Sender<bool>) {
        connect_on(sessions, pushes, presence_board::Shared::default()).await
    }

    /// [`connect`], with the presence board `board`.
    async fn connect_on(
        sessions: broadcast::Receiver<()>,
        pushes: broadcast::Receiver<Push>,
        board: presence_board::Shared,
    ) -> (Ws, tokio::sync::watch::Sender<bool>) {
        let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
        let receivers = std::sync::Arc::new(std::sync::Mutex::new(Some((sessions, pushes))));
        let app = Router::new().route(
            "/ws",
            get(move |ws: WebSocketUpgrade| {
                let taken = receivers.lock().unwrap().take();
                let stop_rx = stop_rx.clone();
                let board = board.clone();
                async move {
                    let pair = taken.expect("the test connects once");
                    ws.on_upgrade(move |socket| {
                        ws_presence_loop(socket, None, Instant::now(), stop_rx, pair, board)
                    })
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            if let Err(e) = axum::serve(listener, app).await {
                panic!("the test /ws stopped: {e}");
            }
        });
        let (ws, _resp) = tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{port}/ws"))
            .await
            .unwrap();
        (ws, stop_tx)
    }

    /// What the socket carried within `within`: the verbs of the commands,
    /// the number of presence frames, and whether the socket ended.
    async fn read_for(ws: &mut Ws, within: Duration) -> (Vec<Command>, usize, bool) {
        use futures_util::StreamExt;
        use tokio_tungstenite::tungstenite::Message as Wire;
        let mut commands = Vec::new();
        let mut beats = 0;
        let deadline = tokio::time::Instant::now() + within;
        loop {
            let msg = match tokio::time::timeout_at(deadline, ws.next()).await {
                Err(_) => return (commands, beats, false),
                Ok(msg) => msg,
            };
            match msg {
                Some(Ok(Wire::Binary(bytes))) => match protocol::decode(&bytes).unwrap() {
                    Frame::Command(c) => commands.push(c),
                    Frame::Presence(_) => beats += 1,
                    other => panic!("an unexpected frame: {other:?}"),
                },
                None | Some(Ok(Wire::Close(_))) | Some(Err(_)) => return (commands, beats, true),
                Some(Ok(_)) => {}
            }
        }
    }

    fn count(commands: &[Command], verb: &str) -> usize {
        commands.iter().filter(|c| c.verb == verb).count()
    }

    /// A pushes receiver that lagged sends `sessions.dirty` and every push
    /// once, then goes on with what the bus still holds.
    #[tokio::test]
    async fn a_lagged_push_receiver_sends_every_push_once() {
        let (_sessions_tx, sessions) = broadcast::channel(4);
        let (pushes_tx, pushes) = broadcast::channel(1);
        let tab = Some("t1".to_string());
        // Two pushes on a bus that holds one: the receiver lags by one.
        for _ in 0..2 {
            pushes_tx.send(Push::Desk { tab: tab.clone() }).unwrap();
        }
        let (mut ws, _stop) = connect(sessions, pushes).await;
        let (commands, _, ended) = read_for(&mut ws, Duration::from_millis(1500)).await;
        assert!(!ended, "the socket stays open");
        let verbs: Vec<&str> = commands.iter().map(|c| c.verb.as_str()).collect();
        assert_eq!(count(&commands, "sessions.dirty"), 1, "got: {verbs:?}");
        assert_eq!(count(&commands, "repos.dirty"), 1, "got: {verbs:?}");
        assert_eq!(count(&commands, "peers.dirty"), 1, "got: {verbs:?}");
        assert_eq!(count(&commands, "attention.dirty"), 1, "got: {verbs:?}");
        let desk_tabs: Vec<&serde_json::Value> = commands
            .iter()
            .filter(|c| c.verb == "desk.dirty")
            .map(|c| &c.payload["tab"])
            .collect();
        assert_eq!(
            desk_tabs,
            [&serde_json::Value::Null, &serde_json::json!("t1")],
            "the lag sends a desk push for every tab, then the push the bus kept"
        );
    }

    /// A channel whose sender is gone turns its arm off: the socket stays
    /// open, the heartbeat goes on, and nothing is sent for that channel.
    async fn a_closed_channel_keeps_the_heartbeat(close_sessions: bool) {
        let (sessions_tx, sessions) = broadcast::channel::<()>(4);
        let (pushes_tx, pushes) = broadcast::channel::<Push>(4);
        let _held = if close_sessions {
            drop(sessions_tx);
            (None, Some(pushes_tx))
        } else {
            drop(pushes_tx);
            (Some(sessions_tx), None)
        };
        let (mut ws, _stop) = connect(sessions, pushes).await;
        let (commands, beats, ended) = read_for(&mut ws, Duration::from_millis(2600)).await;
        assert!(!ended, "a closed channel does not close the socket");
        assert!(beats >= 2, "the heartbeat goes on: {beats} beat(s)");
        let verbs: Vec<&str> = commands.iter().map(|c| c.verb.as_str()).collect();
        assert!(
            verbs.is_empty(),
            "a closed channel sends nothing: {verbs:?}"
        );
    }

    #[tokio::test]
    async fn a_closed_sessions_channel_keeps_the_heartbeat() {
        a_closed_channel_keeps_the_heartbeat(true).await;
    }

    #[tokio::test]
    async fn a_closed_pushes_channel_keeps_the_heartbeat() {
        a_closed_channel_keeps_the_heartbeat(false).await;
    }

    /// The board's answer within `within`: whether the operator is present.
    async fn present_within(board: &presence_board::Shared, want: bool, within: Duration) -> bool {
        let deadline = tokio::time::Instant::now() + within;
        loop {
            let now = std::time::SystemTime::now();
            if board.lock().unwrap().present(now) == want {
                return true;
            }
            if tokio::time::Instant::now() > deadline {
                return false;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    #[tokio::test]
    async fn a_page_report_on_the_presence_socket_sets_presence() {
        use futures_util::SinkExt;
        use tokio_tungstenite::tungstenite::Message as Wire;
        let (_sessions_tx, sessions) = broadcast::channel(4);
        let (_pushes_tx, pushes) = broadcast::channel(4);
        let board = presence_board::Shared::default();
        let (mut ws, _stop) = connect_on(sessions, pushes, board.clone()).await;
        let frame = |payload: serde_json::Value| {
            Wire::Binary(
                protocol::encode(&Frame::Command(Command {
                    id: 0,
                    verb: "presence.report".to_string(),
                    payload,
                }))
                .into(),
            )
        };
        // A frame that is no report is ignored, and the socket stays open.
        ws.send(Wire::Binary(vec![0x02, b'{'].into()))
            .await
            .unwrap();
        ws.send(frame(serde_json::json!({ "visible": true })))
            .await
            .unwrap();
        let now = 1_000_000.0;
        ws.send(frame(serde_json::json!({
            "visible": true, "lastInput": now, "now": now, "origin": "http://127.0.0.1:8765"
        })))
        .await
        .unwrap();
        assert!(
            present_within(&board, true, Duration::from_secs(5)).await,
            "the report makes the operator present"
        );
        assert_eq!(
            board.lock().unwrap().last_origin().as_deref(),
            Some("http://127.0.0.1:8765")
        );

        ws.close(None).await.unwrap();
        drop(ws);
        assert!(
            present_within(&board, false, Duration::from_secs(5)).await,
            "closing the socket ends the presence"
        );
    }
}
