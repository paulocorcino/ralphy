use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::ws::WebSocketUpgrade;
use axum::routing::get;
use axum::Router;
use futures_util::{SinkExt, StreamExt};
use tokio::sync::{broadcast, mpsc};
use tokio_tungstenite::tungstenite::Message as Wire;

use super::*;
use crate::mux::kinds::INBOUND_FRAMES;
use crate::mux::Mode;
use crate::routes::{SessionHost, SocketAudit};

/// The channel whose kind never takes a frame.
const STUCK: u32 = 1;
/// The channel whose kind sends back each frame it takes.
const ECHO: u32 = 2;

type Ws =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// The context of a socket whose kinds serve from a store that does not
/// exist. The test's channels use none of it.
fn ctx() -> Ctx {
    let store = std::path::PathBuf::from("does-not-exist");
    let (_stop, shutdown) = tokio::sync::watch::channel(false);
    Ctx {
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
    }
}

/// Serve one socket whose reader holds `STUCK` and `ECHO` open already.
fn reader_route() -> Router {
    let once = Arc::new(Mutex::new(Some(())));
    Router::new().route(
        "/ws",
        get(move |ws: WebSocketUpgrade| {
            let first = once.lock().unwrap().take();
            async move {
                first.expect("the test connects once");
                ws.on_upgrade(|socket| async move {
                    let (sink, stream) = socket.split();
                    let writer = Writer::start(sink, Mode::Channels);
                    let control = writer.channel(0);
                    let echo = writer.channel(ECHO);
                    let page = PageEntry::open(crate::presence_board::Shared::default());
                    let login = Login::now(crate::auth::AuthState::localhost());
                    let mut reader = Reader::new(writer, control, page, ctx(), login);
                    for (id, channel) in [(STUCK, stuck()), (ECHO, echoing(echo))] {
                        let joiner = Joiner::default();
                        reader.open.insert(id, Open { joiner, channel });
                        reader.used.insert(id);
                    }
                    let (heard, _heard_rx) = watch::channel(Instant::now());
                    read(stream, reader, heard).await;
                })
            }
        }),
    )
}

/// A kind that holds its queue and never reads it.
fn stuck() -> Channel {
    Channel::spawn(|frames: mpsc::Receiver<Vec<u8>>| async move {
        let _held = frames;
        std::future::pending::<()>().await;
    })
}

/// A kind that sends back on `out` each frame it takes.
fn echoing(out: ChannelSender) -> Channel {
    Channel::spawn(|mut frames: mpsc::Receiver<Vec<u8>>| async move {
        while let Some(frame) = frames.recv().await {
            if out.send(frame).await.is_err() {
                return;
            }
        }
    })
}

async fn connect() -> Ws {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move { axum::serve(listener, reader_route()).await });
    let (ws, _resp) = tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{port}/ws"))
        .await
        .unwrap();
    ws
}

async fn send_on(ws: &mut Ws, channel: u32, inner: &[u8]) {
    for piece in protocol::cut(channel, inner) {
        let bytes = protocol::encode(&piece);
        ws.send(Wire::Binary(bytes.into())).await.unwrap();
    }
}

/// What the page reads within 5 s: the reason of the `channel.closed` for
/// `STUCK`, and the frame `ECHO` sent back.
async fn read_both(ws: &mut Ws) -> (Option<String>, Option<Vec<u8>>) {
    let (mut closed, mut echoed) = (None, None);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while closed.is_none() || echoed.is_none() {
        let Ok(Some(Ok(Wire::Binary(bytes)))) = tokio::time::timeout_at(deadline, ws.next()).await
        else {
            break;
        };
        match protocol::decode(&bytes) {
            Ok(Frame::Command(c)) if c.verb == "channel.closed" => {
                assert_eq!(c.payload["channel"], STUCK, "only the stuck channel ends");
                closed = c.payload["reason"].as_str().map(str::to_string);
            }
            Ok(Frame::Channel { channel, piece, .. }) if channel == ECHO => echoed = Some(piece),
            Ok(_) | Err(_) => {}
        }
    }
    (closed, echoed)
}

/// A kind that takes none of the frames the page sends ends with
/// `overflow` at the first frame past its queue, and the reader goes on
/// serving the other channels of the socket.
#[tokio::test]
async fn a_full_inbound_queue_ends_only_its_channel() {
    let mut ws = connect().await;
    for n in 0..=INBOUND_FRAMES {
        send_on(&mut ws, STUCK, format!("frame {n}").as_bytes()).await;
    }
    send_on(&mut ws, ECHO, b"still read").await;

    let (closed, echoed) = read_both(&mut ws).await;
    assert_eq!(closed.as_deref(), Some("overflow"));
    assert_eq!(echoed.as_deref(), Some(&b"still read"[..]));
}
