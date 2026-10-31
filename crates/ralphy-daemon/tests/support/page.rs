//! The page socket as a test client: one channel on `/ws`, read and written as
//! a whole socket. Included by path from several test binaries, so it needs
//! nothing beyond `tokio-tungstenite`, `futures-util`, `serde_json`, `tokio`
//! and `ralphy_daemon::protocol`.
//!
//! A [`Channel`] is one page socket with one channel, id 1. The frames the
//! test sends go on that channel, cut into pieces; the frames it reads are
//! the whole inner frames of that channel; the daemon's `channel.closed`
//! reads as the end of the socket.

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use ralphy_daemon::protocol::{self, Command, Frame, Joiner};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_tungstenite::tungstenite::{
    self, handshake::client::Request, protocol::CloseFrame, Message,
};

/// A WebSocket client stream on a loopback TCP connection.
pub type Ws =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// The id of the one channel a [`Channel`] opens.
const CHANNEL: u32 = 1;

/// How long [`refused`] and [`post_command`] wait for the daemon.
const WITHIN: Duration = Duration::from_secs(10);

/// The URL of the page socket of a daemon on `127.0.0.1:<port>`.
pub fn url(port: u16) -> String {
    format!("ws://127.0.0.1:{port}/ws")
}

/// Connect a page socket to the daemon on `127.0.0.1:<port>`.
pub async fn connect(port: u16) -> Ws {
    let (ws, _resp) = tokio_tungstenite::connect_async(url(port))
        .await
        .expect("connect a page socket");
    ws
}

/// Connect a page socket with a request the test built for [`url`], with its
/// own headers.
pub async fn connect_request(request: Request) -> Ws {
    let (ws, _resp) = tokio_tungstenite::connect_async(request)
        .await
        .expect("connect a page socket");
    ws
}

/// A bare frame on channel 0, as a message.
fn bare(frame: &Frame) -> Message {
    Message::Binary(protocol::encode(frame).into())
}

/// The bare `channel.open` of channel 1 of `kind`. The keys of `fields`, a
/// JSON object, go into the payload beside `channel` and `kind`.
fn open_frame(kind: &str, fields: serde_json::Value) -> Message {
    let mut payload = serde_json::json!({ "channel": CHANNEL, "kind": kind });
    if let (Some(all), serde_json::Value::Object(extra)) = (payload.as_object_mut(), fields) {
        all.extend(extra);
    }
    bare(&Frame::Command(Command {
        id: 0,
        verb: "channel.open".to_string(),
        payload,
    }))
}

/// One channel, id 1, on its own page socket.
pub struct Channel {
    ws: Ws,
    /// A `spawn` channel whose open waits for the first frame the test sends.
    open_on_send: bool,
    joiner: Joiner,
    /// The `reason` of the daemon's `channel.closed` for channel 1.
    closed: Option<String>,
    /// The end was read: `next` returns `None` from now on.
    ended: bool,
}

impl Channel {
    fn on(ws: Ws, open_on_send: bool) -> Channel {
        Channel {
            ws,
            open_on_send,
            joiner: Joiner::default(),
            closed: None,
            ended: false,
        }
    }

    /// Open channel 1 of `kind` on `ws`. The keys of `fields`, a JSON object,
    /// go into the open payload beside `channel` and `kind`.
    pub async fn open(mut ws: Ws, kind: &str, fields: serde_json::Value) -> Channel {
        ws.send(open_frame(kind, fields))
            .await
            .expect("send channel.open");
        Channel::on(ws, false)
    }

    /// A `spawn` channel on `ws` whose open waits for the first `send`. A
    /// binary frame that decodes as a command opens it with
    /// `command: {id, verb, payload}`; any other frame opens it with no
    /// command. That first frame goes in the open only.
    pub fn spawn_on(ws: Ws) -> Channel {
        Channel::on(ws, true)
    }

    /// Send one message. A binary message is one inner frame, cut into
    /// pieces on channel 1; any other message goes on the socket as it is.
    pub async fn send(&mut self, message: Message) -> Result<(), tungstenite::Error> {
        if self.open_on_send {
            self.open_on_send = false;
            return self.ws.send(spawn_open(&message)).await;
        }
        let Message::Binary(inner) = message else {
            return self.ws.send(message).await;
        };
        for piece in protocol::cut(CHANNEL, &inner) {
            self.ws.send(bare(&piece)).await?;
        }
        Ok(())
    }

    /// The next message of channel 1: one `Message::Binary` per whole inner
    /// frame, and the Pings and Pongs of the socket. The frames of channel 0
    /// and of other channels are skipped. The daemon's `channel.closed` for
    /// channel 1 reads as one `Message::Close(None)`, then `None`.
    pub async fn next(&mut self) -> Option<Result<Message, tungstenite::Error>> {
        if self.ended {
            return None;
        }
        loop {
            let message = match self.ws.next().await? {
                Ok(message) => message,
                Err(error) => return Some(Err(error)),
            };
            match message {
                Message::Binary(bytes) => {
                    if let Some(message) = self.take(&bytes) {
                        return Some(Ok(message));
                    }
                }
                Message::Ping(_) | Message::Pong(_) => return Some(Ok(message)),
                Message::Close(frame) => {
                    self.ended = true;
                    return Some(Ok(Message::Close(frame)));
                }
                Message::Text(_) | Message::Frame(_) => {}
            }
        }
    }

    /// What one binary message from the daemon means for channel 1.
    fn take(&mut self, bytes: &[u8]) -> Option<Message> {
        match protocol::decode(bytes) {
            Ok(Frame::Channel {
                channel: CHANNEL,
                more,
                piece,
            }) => {
                let inner = self
                    .joiner
                    .push(more, &piece)
                    .expect("the daemon sends an inner frame within the cap")?;
                Some(Message::Binary(inner.into()))
            }
            Ok(Frame::Command(command)) => {
                let reason = closed_reason_of(&command)?;
                self.closed = Some(reason);
                self.ended = true;
                Some(Message::Close(None))
            }
            Ok(Frame::Channel { .. } | Frame::Terminal { .. } | Frame::Presence(_)) | Err(_) => {
                None
            }
        }
    }

    /// Close channel 1 with `channel.close`, then the socket with `frame`.
    pub async fn close(&mut self, frame: Option<CloseFrame>) -> Result<(), tungstenite::Error> {
        self.close_channel().await?;
        self.ws.close(frame).await
    }

    /// Close channel 1 with `channel.close`; the socket stays open.
    pub async fn close_channel(&mut self) -> Result<(), tungstenite::Error> {
        if self.open_on_send {
            return Ok(());
        }
        let payload = serde_json::json!({ "channel": CHANNEL });
        let close = bare(&Frame::Command(Command {
            id: 0,
            verb: "channel.close".to_string(),
            payload,
        }));
        self.ws.send(close).await
    }

    /// The `reason` of the daemon's `channel.closed` for channel 1, once
    /// [`Channel::next`] read it.
    pub fn closed_reason(&self) -> Option<&str> {
        self.closed.as_deref()
    }
}

/// The `channel.open` of a `spawn` channel whose command is `first`, when
/// `first` is a binary command frame, and with no command otherwise.
fn spawn_open(first: &Message) -> Message {
    let command = match first {
        Message::Binary(bytes) => match protocol::decode(bytes) {
            Ok(Frame::Command(command)) => Some(command),
            Ok(_) | Err(_) => None,
        },
        Message::Text(_)
        | Message::Ping(_)
        | Message::Pong(_)
        | Message::Close(_)
        | Message::Frame(_) => None,
    };
    match command {
        Some(command) => open_frame("spawn", serde_json::json!({ "command": command })),
        None => open_frame("spawn", serde_json::json!({})),
    }
}

/// The `reason` of `command` when it is the `channel.closed` of channel 1.
fn closed_reason_of(command: &Command) -> Option<String> {
    let ours = command.verb == "channel.closed"
        && command.payload["channel"].as_u64() == Some(u64::from(CHANNEL));
    if !ours {
        return None;
    }
    Some(command.payload["reason"].as_str()?.to_string())
}

/// A `console` channel with the open `fields` (the console to launch,
/// reattach or watch).
pub async fn console(port: u16, fields: serde_json::Value) -> Channel {
    Channel::open(connect(port).await, "console", fields).await
}

/// A `spawn` channel whose first `send` names its command.
pub async fn command(port: u16) -> Channel {
    Channel::spawn_on(connect(port).await)
}

/// A `spawn` channel opened with `command`.
pub async fn spawn(port: u16, command: &Command) -> Channel {
    let fields = serde_json::json!({ "command": command });
    Channel::open(connect(port).await, "spawn", fields).await
}

/// A `tree` channel: file-tree, HEAD, run and peer watches, and their pushes.
pub async fn tree(port: u16) -> Channel {
    Channel::open(connect(port).await, "tree", serde_json::json!({})).await
}

/// A `runs` channel: the same verbs and pushes as a `tree` channel.
pub async fn runs(port: u16) -> Channel {
    Channel::open(connect(port).await, "runs", serde_json::json!({})).await
}

/// A `changes` channel: a `changes.dirty` push for each run exit.
pub async fn changes(port: u16) -> Channel {
    Channel::open(connect(port).await, "changes", serde_json::json!({})).await
}

/// Open a new console launch the daemon must refuse, and return the payload
/// of its refusal: exactly one `session-end` with `reason: "refused"` and a
/// string `message`, then `channel.closed` with reason `refused`. Any other
/// frame panics.
pub async fn refused(port: u16, fields: serde_json::Value) -> serde_json::Value {
    let mut channel = console(port, fields).await;
    let mut payload = None;
    tokio::time::timeout(WITHIN, async {
        while let Some(message) = channel.next().await {
            match message.expect("the page socket reads") {
                Message::Binary(bytes) => {
                    let frame = protocol::decode(&bytes).expect("a well-formed frame");
                    let Frame::Command(end) = frame else {
                        panic!("a refusal carries no other frame; got {frame:?}");
                    };
                    assert_eq!(end.verb, "session-end", "the only frame is the end");
                    assert_eq!(end.payload["reason"], "refused", "{end:?}");
                    assert!(end.payload["message"].is_string(), "{end:?}");
                    assert!(payload.is_none(), "exactly one refusal frame");
                    payload = Some(end.payload);
                }
                Message::Close(_) => break,
                Message::Text(_) | Message::Ping(_) | Message::Pong(_) | Message::Frame(_) => {}
            }
        }
    })
    .await
    .expect("the daemon ends a refused launch");
    assert_eq!(
        channel.closed_reason(),
        Some("refused"),
        "the channel of a refused launch ends as refused"
    );
    payload.expect("a session-end frame")
}

/// `POST /api/command` with `command` over a plain HTTP/1.1 connection, and
/// the reply payload the daemon answers.
pub async fn post_command(port: u16, command: &Command) -> serde_json::Value {
    let body = serde_json::to_string(command).expect("a command serializes to JSON");
    let request = format!(
        "POST /api/command HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\
         Content-Type: application/json\r\nContent-Length: {}\r\n\
         Connection: close\r\n\r\n{body}",
        body.len()
    );
    let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .expect("connect to the daemon");
    stream
        .write_all(request.as_bytes())
        .await
        .expect("write the request");
    let mut reply = String::new();
    tokio::time::timeout(WITHIN, stream.read_to_string(&mut reply))
        .await
        .expect("an answer in time")
        .expect("read the answer");
    let (head, json) = reply.split_once("\r\n\r\n").unwrap_or((&reply, ""));
    assert!(
        head.starts_with("HTTP/1.1 200"),
        "the daemon answers the command: {reply}"
    );
    serde_json::from_str(json).expect("the answer is JSON")
}
