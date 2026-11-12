use super::*;
use crate::mux::Writer;
use crate::routes::ws_session::outbound::SESSION_SIZE;
use crate::routes::ws_session::traffic::{Tab, SESSION_OPEN};
use std::sync::{Arc, Mutex};

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use futures_util::stream::SplitStream;
use futures_util::StreamExt;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite;

mod input;
mod session_end;
mod watcher;

/// The channel of the page socket the console is served on.
const CHANNEL: u32 = 1;

/// The writer queue depth the page socket gives a console channel.
const CONSOLE_QUEUE: usize = 1;

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
            "~".to_owned(),
            "console".to_owned(),
            "console".to_owned(),
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

/// Serve the channel bridge for one client on `listener`, on channel
/// [`CHANNEL`] of a page socket's writer; `gzip` is what the client's tab
/// asked for.
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
                ws.on_upgrade(move |socket| async move {
                    let (sink, stream) = socket.split();
                    let writer = Writer::start(sink);
                    let out = writer.channel_with_depth(CHANNEL, CONSOLE_QUEUE);
                    let (inbound, mut frames) = mpsc::channel(64);
                    let tab = Tab {
                        gzip,
                        ..Tab::default()
                    };
                    let bridge =
                        channel::attached(attach, id, tab).bridge(&out, &mut frames, shutdown);
                    tokio::pin!(bridge);
                    tokio::select! {
                        () = &mut bridge => {}
                        () = join_pieces(stream, inbound) => bridge.await,
                    }
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

/// Join the pieces the client sends on [`CHANNEL`] into inner frames for
/// the bridge, until the client leaves; the dropped `inbound` then tells the
/// bridge so.
async fn join_pieces(mut stream: SplitStream<WebSocket>, inbound: mpsc::Sender<Vec<u8>>) {
    let mut joiner = protocol::Joiner::default();
    while let Some(Ok(message)) = stream.next().await {
        let Message::Binary(bytes) = message else {
            continue;
        };
        let Ok(Frame::Channel {
            channel: CHANNEL,
            more,
            piece,
        }) = protocol::decode(&bytes)
        else {
            panic!("the client sends only pieces of channel {CHANNEL}");
        };
        let whole = joiner.push(more, &piece).expect("a frame within the cap");
        if let Some(inner) = whole {
            if inbound.send(inner).await.is_err() {
                return;
            }
        }
    }
}

/// The wire messages that carry `frame` on [`CHANNEL`].
fn channel_messages(frame: &Frame) -> Vec<tungstenite::Message> {
    protocol::cut(CHANNEL, &protocol::encode(frame))
        .iter()
        .map(|piece| tungstenite::Message::Binary(protocol::encode(piece).into()))
        .collect()
}

/// The next inner frame the client gets on [`CHANNEL`], joined from its
/// pieces, or `None` when the socket ends.
async fn next_inner(
    client: &mut (impl futures_util::Stream<Item = Result<tungstenite::Message, tungstenite::Error>>
              + Unpin),
) -> Option<Vec<u8>> {
    let mut joiner = protocol::Joiner::default();
    while let Some(Ok(message)) = client.next().await {
        let tungstenite::Message::Binary(bytes) = message else {
            continue;
        };
        match protocol::decode(&bytes).expect("a well-formed frame") {
            Frame::Channel {
                channel: CHANNEL,
                more,
                piece,
            } => {
                if let Some(inner) = joiner.push(more, &piece).expect("a frame within the cap") {
                    return Some(inner);
                }
            }
            other @ (Frame::Terminal { .. }
            | Frame::Command(_)
            | Frame::Presence(_)
            | Frame::Channel { .. }) => {
                panic!("the bridge sends only pieces of channel {CHANNEL}: {other:?}")
            }
        }
    }
    None
}

/// What a client saw: the output, and where `session-end` came in it.
struct Seen {
    received: Vec<u8>,
    ended_after: Option<usize>,
}

/// Read until `session-end`, pausing `pause` after every inner frame. A
/// channel has no Close: the end is the last frame the bridge sends on it.
async fn read_until_end(
    client: &mut (impl futures_util::Stream<Item = Result<tungstenite::Message, tungstenite::Error>>
              + Unpin),
    pause: Duration,
) -> Seen {
    let mut seen = Seen {
        received: Vec::new(),
        ended_after: None,
    };
    while let Some(inner) = next_inner(client).await {
        match protocol::decode(&inner).expect("a well-formed inner frame") {
            Frame::Terminal { data, .. } => seen.received.extend_from_slice(&data),
            Frame::Command(cmd) if cmd.verb == "session-end" => {
                assert_eq!(cmd.payload["reason"], "child-exited");
                seen.ended_after = Some(seen.received.len());
                break;
            }
            Frame::Command(_) | Frame::Presence(_) | Frame::Channel { .. } => {}
        }
        tokio::time::sleep(pause).await;
    }
    seen
}

fn resize_command(payload: serde_json::Value) -> Command {
    Command {
        id: 0,
        verb: "resize".to_owned(),
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
