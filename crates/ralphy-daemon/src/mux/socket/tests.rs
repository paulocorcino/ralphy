use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::ws::WebSocketUpgrade;
use axum::routing::get;
use axum::Router;

use super::*;
use crate::protocol::{Command, Frame};
use crate::routes::{SessionHost, SocketAudit};

/// A page socket with no identity, whose channels serve from a store that
/// does not exist.
fn page(
    sessions: broadcast::Receiver<()>,
    pushes: broadcast::Receiver<Push>,
    board: presence_board::Shared,
    shutdown: watch::Receiver<bool>,
) -> Page {
    let store = std::path::PathBuf::from("does-not-exist");
    Page {
        identity: None,
        start: std::time::Instant::now(),
        shutdown: shutdown.clone(),
        sessions,
        pushes,
        board,
        ctx: Ctx {
            who: SocketAudit::of(Arc::new(crate::audit::Audit::in_dir(None)), None),
            secret_ok: false,
            host: SessionHost {
                peers_dir: store.join("peers"),
                identity: None,
                environment: "test".to_string(),
                bound_port: 0,
                sessions: Arc::new(crate::session::SessionManager::new()),
                registry_path: store.join("repos.toml"),
                shutdown,
            },
            daemon_id: None,
            watchers: Arc::new(crate::watch::WatcherManager::new(crate::watch::MAX_WATCHES)),
            run_exits: broadcast::channel(4).0,
        },
        login: Login::now(crate::auth::AuthState::localhost()),
    }
}

/// A router whose `/ws` serves `page` once. When the socket ends, the
/// time it ended goes on `ended`, when one is given.
fn one_page(page: Page, ended: Option<tokio::sync::oneshot::Sender<Instant>>) -> Router {
    let slot = Arc::new(Mutex::new(Some((page, ended))));
    Router::new().route(
        "/ws",
        get(move |ws: WebSocketUpgrade| {
            let taken = slot.lock().unwrap().take();
            async move {
                let (page, ended) = taken.expect("the test connects once");
                ws.on_upgrade(move |socket| async move {
                    serve(socket, page).await;
                    if let Some(ended) = ended {
                        ended.send(Instant::now()).unwrap();
                    }
                })
            }
        }),
    )
}

type Ws =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// Serve one page socket on `sessions` and `pushes` behind a loopback
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
    let app = one_page(page(sessions, pushes, board, stop_rx), None);
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
                other @ (Frame::Terminal { .. } | Frame::Channel { .. }) => {
                    panic!("an unexpected frame: {other:?}")
                }
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

/// One in-memory connection, accepted once. The bytes move between tasks
/// without the kernel, so paused time cannot move forward while bytes are
/// still on their way, as it can with a loopback socket.
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

/// A page that sends nothing, not even the pong a browser sends for each
/// ping, is gone: the socket ends 45 s after the page last spoke. The
/// client never reads, so it answers no ping. Time is paused, so the 45 s
/// pass in test time.
#[tokio::test(start_paused = true)]
async fn a_silent_page_socket_is_closed_after_45_s() {
    let (_sessions_tx, sessions) = broadcast::channel(4);
    let (_pushes_tx, pushes) = broadcast::channel(4);
    let (_stop, shutdown) = watch::channel(false);
    let (ended_tx, ended) = tokio::sync::oneshot::channel();
    let app = one_page(
        page(
            sessions,
            pushes,
            presence_board::Shared::default(),
            shutdown,
        ),
        Some(ended_tx),
    );
    let (server_io, client_io) = tokio::io::duplex(1 << 20);
    tokio::spawn(async move { axum::serve(OneConnection(Some(server_io)), app).await });
    let (client, _) = tokio_tungstenite::client_async("ws://localhost/ws", client_io)
        .await
        .expect("the page socket upgrades");
    // The handshake was the last thing the page sent.
    let spoke = Instant::now();

    let ended = tokio::time::timeout(Duration::from_secs(120), ended)
        .await
        .expect("the socket ends within 120 s")
        .expect("the socket task reports its end");
    let silent = ended - spoke;
    assert!(
        silent >= Duration::from_secs(45) && silent < Duration::from_secs(46),
        "the socket ended after {silent:?} of silence"
    );
    drop(client);
}
