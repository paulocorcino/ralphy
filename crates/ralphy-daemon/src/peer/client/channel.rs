//! One channel on a peer's page socket `/ws`: the probe and the dial of the
//! socket, the bare `channel.open`, and the frames of that one channel.
//!
//! Each channel has a socket of its own and is always channel 1 on it, so
//! a peer that fails is diagnosed for each relay alone. The peer's channel 0
//! (its heartbeat, its pushes) is read and dropped here: only the
//! `channel.closed` that ends channel 1 means anything to the caller.

use futures_util::{SinkExt, StreamExt};
use tokio::io::AsyncWriteExt;
use tokio_tungstenite::tungstenite::{Error as WsError, Message};

use super::{websocket, PeerSocket, PeerStatus, SelfRef, SocketError};
use crate::peer::PeerDescriptor;
use crate::protocol::{self, Command, Frame, Joiner};

/// The id of the one channel this client opens on each socket.
const CHANNEL: u32 = 1;

/// A channel open on a peer's page socket.
pub struct PeerChannel {
    socket: PeerSocket,
    joiner: Joiner,
}

/// What [`PeerChannel::next`] read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PeerEvent {
    /// One whole inner frame the peer sent on the channel.
    Frame(Vec<u8>),
    /// The peer ended the channel with `channel.closed`; `reason` is its
    /// reason as the peer wrote it.
    Closed { reason: String },
    /// The socket ended, or failed.
    Gone,
}

/// Probe the peer, dial its `/ws`, and open channel 1 of `kind` on it. The
/// keys of `fields`, a JSON object, go into the open beside `channel` and
/// `kind`. A refusal of the open comes later, from [`PeerChannel::next`].
pub async fn open_channel(
    d: &PeerDescriptor,
    me: SelfRef<'_>,
    kind: &str,
    fields: serde_json::Value,
) -> Result<PeerChannel, SocketError> {
    let mut socket = websocket(d, "/ws", "page socket", me).await?;
    let mut payload = serde_json::json!({ "channel": CHANNEL, "kind": kind });
    if let (Some(all), serde_json::Value::Object(extra)) = (payload.as_object_mut(), fields) {
        all.extend(extra);
    }
    socket
        .send(bare("channel.open", payload))
        .await
        .map_err(|e| {
            SocketError::Peer(PeerStatus::Unreachable {
                why: format!("opening a channel on the peer page socket: {e}"),
            })
        })?;
    Ok(PeerChannel {
        socket,
        joiner: Joiner::default(),
    })
}

/// A bare control verb on channel 0.
fn bare(verb: &str, payload: serde_json::Value) -> Message {
    let command = Command {
        id: 0,
        verb: verb.to_string(),
        payload,
    };
    Message::Binary(protocol::encode(&Frame::Command(command)).into())
}

/// What one frame of the socket means for channel 1.
enum Read {
    Event(PeerEvent),
    /// A piece of an inner frame still due, or a frame of channel 0.
    Nothing,
}

impl PeerChannel {
    /// Send one inner frame on the channel, cut into pieces. Not cancel safe:
    /// a send stopped between two pieces leaves the frame cut short.
    pub async fn send(&mut self, inner: &[u8]) -> Result<(), WsError> {
        for piece in protocol::cut(CHANNEL, inner) {
            self.socket
                .feed(Message::Binary(protocol::encode(&piece).into()))
                .await?;
        }
        self.socket.flush().await
    }

    /// The next event of the channel. A ping of the peer is answered here.
    ///
    /// Cancel safe: each await is a read of the socket or the flush of a
    /// pong, and the pieces joined so far stay in `self`. A flush stopped
    /// part way leaves the pong queued, and the next read sends it.
    pub async fn next(&mut self) -> PeerEvent {
        loop {
            let message = match self.socket.next().await {
                Some(Ok(message)) => message,
                Some(Err(error)) => {
                    tracing::debug!(%error, "a peer page socket failed");
                    return PeerEvent::Gone;
                }
                None => return PeerEvent::Gone,
            };
            let read = match message {
                Message::Binary(bytes) => self.take(&bytes),
                Message::Ping(_) => self.pong().await,
                Message::Close(_) => Read::Event(PeerEvent::Gone),
                Message::Pong(_) | Message::Text(_) | Message::Frame(_) => Read::Nothing,
            };
            if let Read::Event(event) = read {
                return event;
            }
        }
    }

    /// Send the pong the socket queued when it read the ping, now rather
    /// than with the next frame for the peer.
    async fn pong(&mut self) -> Read {
        match self.socket.flush().await {
            Ok(()) => Read::Nothing,
            Err(error) => {
                tracing::debug!(%error, "a peer page socket could not answer a ping");
                Read::Event(PeerEvent::Gone)
            }
        }
    }

    /// Read one binary message of the socket.
    fn take(&mut self, bytes: &[u8]) -> Read {
        match protocol::decode(bytes) {
            Ok(Frame::Channel {
                channel: CHANNEL,
                more,
                piece,
            }) => match self.joiner.push(more, &piece) {
                Ok(Some(inner)) => Read::Event(PeerEvent::Frame(inner)),
                Ok(None) => Read::Nothing,
                Err(error) => {
                    tracing::debug!(%error, "a peer page socket sent a frame past the cap");
                    Read::Event(PeerEvent::Gone)
                }
            },
            Ok(Frame::Command(command)) => closed(&command).map_or(Read::Nothing, Read::Event),
            // Another channel, the heartbeat, or a frame this daemon cannot
            // read: none of them is channel 1's.
            Ok(Frame::Channel { .. } | Frame::Terminal { .. } | Frame::Presence(_)) | Err(_) => {
                Read::Nothing
            }
        }
    }

    /// End the channel and the socket: `channel.close`, the socket's Close,
    /// then the TCP shutdown. A peer already gone is not an error.
    pub async fn close(&mut self) {
        let channel = serde_json::json!({ "channel": CHANNEL });
        if let Err(error) = self.socket.send(bare("channel.close", channel)).await {
            tracing::debug!(%error, "a peer page socket was gone before its channel closed");
        }
        if let Err(error) = self.socket.send(Message::Close(None)).await {
            tracing::debug!(%error, "a peer page socket was gone before its close");
        }
        if let tokio_tungstenite::MaybeTlsStream::Plain(stream) = self.socket.get_mut() {
            if let Err(error) = stream.shutdown().await {
                tracing::debug!(%error, "a peer page socket was gone before its shutdown");
            }
        }
    }
}

/// The end of channel 1 that a bare `channel.closed` names, if it names it.
fn closed(command: &Command) -> Option<PeerEvent> {
    let channel = command.payload.get("channel")?.as_u64()?;
    if command.verb != "channel.closed" || channel != u64::from(CHANNEL) {
        return None;
    }
    let reason = command
        .payload
        .get("reason")
        .and_then(serde_json::Value::as_str);
    Some(PeerEvent::Closed {
        reason: reason.unwrap_or_default().to_string(),
    })
}
