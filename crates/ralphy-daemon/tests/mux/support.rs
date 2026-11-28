//! The helpers every channel test shares: serve a daemon on a loopback port
//! with a temporary store, connect a page socket to its `/ws`, open and close
//! channels, and send and read the frames of one channel.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use ralphy_daemon::protocol::{self, Command, Frame, Joiner};
use tokio_tungstenite::tungstenite::Message;

pub(crate) type Ws =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// A daemon router served on `127.0.0.1:<port>`, with its store in a
/// temporary directory. It runs until the test ends.
pub(crate) struct Daemon {
    pub port: u16,
    _store: tempfile::TempDir,
}

/// Serve a router whose store is a new temporary directory.
pub(crate) async fn serve() -> Daemon {
    serve_with(|_store| {}).await
}

/// Serve a router whose store is a new temporary directory, after `setup`
/// wrote into that directory what the test needs (`repos.toml`, …).
pub(crate) async fn serve_with(setup: impl FnOnce(&std::path::Path)) -> Daemon {
    let dir = tempfile::tempdir().expect("create a temporary store");
    setup(dir.path());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind a local port");
    serve_on(listener, dir)
}

/// The size of each TCP buffer of a socket that must fill and stay full.
const FIXED_BUFFER: u32 = 64 * 1024;

/// A TCP socket whose buffers keep their size: Linux grows a buffer that has
/// no size of its own, and a writer that nobody reads then sends again.
fn fixed_buffer_socket() -> tokio::net::TcpSocket {
    let socket = tokio::net::TcpSocket::new_v4().expect("create a TCP socket");
    socket
        .set_send_buffer_size(FIXED_BUFFER)
        .expect("set the send buffer size");
    socket
        .set_recv_buffer_size(FIXED_BUFFER)
        .expect("set the receive buffer size");
    socket
}

/// [`serve`], where the accepted sockets have fixed TCP buffers.
pub(crate) async fn serve_with_fixed_buffers() -> Daemon {
    let dir = tempfile::tempdir().expect("create a temporary store");
    let socket = fixed_buffer_socket();
    socket
        .bind(([127, 0, 0, 1], 0).into())
        .expect("bind a local port");
    // An accepted socket takes the buffer sizes of its listener.
    let listener = socket.listen(16).expect("listen on the local port");
    serve_on(listener, dir)
}

/// [`connect`], with fixed TCP buffers on the page's side.
pub(crate) async fn connect_with_fixed_buffers(daemon: &Daemon) -> Ws {
    let stream = fixed_buffer_socket()
        .connect(([127, 0, 0, 1], daemon.port).into())
        .await
        .expect("connect to the daemon");
    let url = format!("ws://127.0.0.1:{}/ws", daemon.port);
    let stream = tokio_tungstenite::MaybeTlsStream::Plain(stream);
    let (ws, _resp) = tokio_tungstenite::client_async(url, stream)
        .await
        .expect("connect to /ws");
    ws
}

/// Serve a router on `listener`, with its store in `dir`.
fn serve_on(listener: tokio::net::TcpListener, dir: tempfile::TempDir) -> Daemon {
    let port = listener
        .local_addr()
        .expect("read the bound address")
        .port();
    let (stop, shutdown) = tokio::sync::watch::channel(false);
    let app = ralphy_daemon::router(
        None,
        dir.path().join("repos.toml"),
        dir.path().join("usage"),
        ralphy_daemon::StorePaths::default(),
        Instant::now(),
        shutdown,
        ralphy_daemon::auth::AuthState::localhost(),
    );
    tokio::spawn(async move {
        // The shutdown stays open while the server runs.
        let _stop = stop;
        axum::serve(listener, app)
            .await
            .expect("serve the test app");
    });
    Daemon { port, _store: dir }
}

/// Connect a page socket to the daemon's `/ws`.
pub(crate) async fn connect(daemon: &Daemon) -> Ws {
    let url = format!("ws://127.0.0.1:{}/ws", daemon.port);
    let (ws, _resp) = tokio_tungstenite::connect_async(url)
        .await
        .expect("connect to /ws");
    ws
}

/// Send one bare frame on channel 0.
pub(crate) async fn send_bare(ws: &mut Ws, frame: &Frame) {
    ws.send(Message::Binary(protocol::encode(frame).into()))
        .await
        .expect("send a frame on the page socket");
}

/// Send one control verb on channel 0.
pub(crate) async fn send_control(ws: &mut Ws, verb: &str, payload: serde_json::Value) {
    let command = Command {
        id: 0,
        verb: verb.to_owned(),
        payload,
    };
    send_bare(ws, &Frame::Command(command)).await;
}

/// Send `channel.open` for `channel` of `kind`. The keys of `fields`, a JSON
/// object, go into the open payload beside `channel` and `kind`.
pub(crate) async fn open(ws: &mut Ws, channel: u32, kind: &str, fields: serde_json::Value) {
    let mut payload = serde_json::json!({ "channel": channel, "kind": kind });
    if let (Some(all), serde_json::Value::Object(extra)) = (payload.as_object_mut(), fields) {
        all.extend(extra);
    }
    send_control(ws, "channel.open", payload).await;
}

/// Send `channel.close` for `channel`.
pub(crate) async fn close(ws: &mut Ws, channel: u32) {
    send_control(
        ws,
        "channel.close",
        serde_json::json!({ "channel": channel }),
    )
    .await;
}

/// Send one inner frame on `channel`, cut into channel frames.
pub(crate) async fn send_on(ws: &mut Ws, channel: u32, inner: &[u8]) {
    for piece in protocol::cut(channel, inner) {
        send_bare(ws, &piece).await;
    }
}

/// The next whole inner frame of `channel` within `within`, or `None` when
/// none arrives or the socket ends. On channel 0 an inner frame is a bare
/// frame. The frames of every other channel are skipped.
pub(crate) async fn next_inner(ws: &mut Ws, channel: u32, within: Duration) -> Option<Vec<u8>> {
    let mut joiner = Joiner::default();
    tokio::time::timeout(within, async {
        while let Some(Ok(message)) = ws.next().await {
            let Message::Binary(bytes) = message else {
                continue;
            };
            match protocol::decode(&bytes) {
                Ok(Frame::Channel {
                    channel: c,
                    more,
                    piece,
                }) if c == channel => {
                    if let Some(inner) = joiner.push(more, &piece).expect("a frame within the cap")
                    {
                        return Some(inner);
                    }
                }
                Ok(Frame::Channel { .. }) => {}
                Ok(_) if channel == 0 => return Some(bytes.to_vec()),
                Ok(_) | Err(_) => {}
            }
        }
        None
    })
    .await
    .ok()
    .flatten()
}

/// A `channel.closed` the daemon sent.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Closed {
    pub channel: u64,
    pub reason: String,
}

/// The next `channel.closed` on channel 0 within `within`, or `None`.
pub(crate) async fn next_closed(ws: &mut Ws, within: Duration) -> Option<Closed> {
    let deadline = tokio::time::Instant::now() + within;
    loop {
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        let inner = next_inner(ws, 0, left).await?;
        let Ok(Frame::Command(command)) = protocol::decode(&inner) else {
            continue;
        };
        if command.verb == "channel.closed" {
            return Some(Closed {
                channel: command.payload.get("channel")?.as_u64()?,
                reason: command.payload.get("reason")?.as_str()?.to_owned(),
            });
        }
    }
}

/// Whether the daemon ends the socket within `within`. The frames that come
/// before the end are skipped.
pub(crate) async fn ends_within(ws: &mut Ws, within: Duration) -> bool {
    let ended = tokio::time::timeout(within, async {
        loop {
            match ws.next().await {
                None | Some(Err(_) | Ok(Message::Close(_))) => return,
                Some(Ok(_)) => {}
            }
        }
    })
    .await;
    ended.is_ok()
}

/// The log lines written while a test runs.
#[derive(Clone, Default)]
pub(crate) struct Written(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for Written {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().expect("a test lock").extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Written {
    type Writer = Written;
    fn make_writer(&'a self) -> Written {
        self.clone()
    }
}

impl Written {
    /// How many of the lines written so far hold `needle`.
    pub(crate) fn lines_with(&self, needle: &str) -> usize {
        let bytes = self.0.lock().expect("a test lock").clone();
        String::from_utf8_lossy(&bytes)
            .lines()
            .filter(|line| line.contains(needle))
            .count()
    }

    /// Wait at most `within` until more than `after` lines hold `needle`.
    /// Returns whether they do.
    pub(crate) async fn more_lines_with(
        &self,
        needle: &str,
        after: usize,
        within: Duration,
    ) -> bool {
        let deadline = tokio::time::Instant::now() + within;
        while self.lines_with(needle) <= after {
            if tokio::time::Instant::now() >= deadline {
                return false;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        true
    }
}
