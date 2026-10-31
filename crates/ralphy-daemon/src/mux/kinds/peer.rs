//! The `console` channel of a console a peer hosts: this daemon relays the
//! peer's session socket on the channel.
//!
//! The peer's frames go to the page whole, and the socket's writer cuts each
//! one into pieces. The page's frames go to the peer unchanged. The page
//! socket has its own ping, so the peer's pings are answered here and never
//! reach the page (ADR-0082 amendment 2026-10-30, liveness).

use std::ops::ControlFlow;
use std::time::Instant;

use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::{self, Message};

use super::{CloseReason, Opening};
use crate::mux::Out;
use crate::peer::client::PeerSocket;
use crate::protocol::{self, Frame};
use crate::routes::{close_peer_session, hand_on, PeerRelay, Traffic, PEER_CLOSED};

/// Whether the relay goes on. `Break(true)`: the peer has gone, and the
/// channel ends with `channel.closed`; `Break(false)`: the page socket has
/// gone, and nobody is left to tell.
type Flow = ControlFlow<bool>;

/// Serve a `console` channel whose open the peer of `relay` accepted.
/// `frames` are the inner frames the page sends on the channel.
pub(super) async fn open(
    opening: Opening,
    relay: Box<PeerRelay>,
    mut frames: mpsc::Receiver<Vec<u8>>,
) {
    let PeerRelay { peer, traffic } = *relay;
    let mut relay = Relay {
        peer: Some(*peer),
        traffic,
        end: None,
    };
    let told = relay.run(&opening, &mut frames).await;
    drop(relay);
    if told {
        opening.end(CloseReason::Done).await;
    }
}

/// The relay of one channel. Dropped, it logs its traffic line and closes
/// the peer socket: the kind's task can be stopped at any await, when the
/// page closes the channel or the page socket ends.
struct Relay {
    /// `None` only once the peer socket is closed.
    peer: Option<PeerSocket>,
    traffic: Traffic,
    /// The end this daemon decided, if it decided one.
    end: Option<&'static str>,
}

impl Relay {
    async fn run(&mut self, opening: &Opening, frames: &mut mpsc::Receiver<Vec<u8>>) -> bool {
        let Some(peer) = self.peer.as_mut() else {
            return false;
        };
        // A frame from the peer waits here for room on the channel's queue,
        // and the peer is not read while it waits: a slow page slows the peer.
        let mut held: Option<Out> = None;
        loop {
            let flow = tokio::select! {
                frame = frames.recv() => to_peer(frame, peer, &mut self.traffic).await,
                permit = opening.out.reserve(), if held.is_some() => hand_on(permit, &mut held),
                incoming = peer.next(), if held.is_none() => {
                    from_peer(incoming, peer, &mut held, &mut self.traffic).await
                }
            };
            if let ControlFlow::Break(peer_gone) = flow {
                if peer_gone {
                    self.end = Some(PEER_CLOSED);
                }
                return peer_gone;
            }
        }
    }
}

impl Drop for Relay {
    fn drop(&mut self) {
        self.traffic.log(self.end, Instant::now());
        let Some(mut peer) = self.peer.take() else {
            return;
        };
        // The close is sent in a task of its own: a drop cannot wait.
        match tokio::runtime::Handle::try_current() {
            Ok(runtime) => {
                runtime.spawn(async move { close_peer_session(&mut peer).await });
            }
            Err(_no_runtime) => tracing::debug!("a peer socket dropped without its close"),
        }
    }
}

/// Send one inner frame from the page to the peer, unchanged.
async fn to_peer(frame: Option<Vec<u8>>, peer: &mut PeerSocket, traffic: &mut Traffic) -> Flow {
    let Some(bytes) = frame else {
        // The page socket has dropped the channel.
        return ControlFlow::Break(false);
    };
    traffic.inbound(bytes.len());
    // Read only for the log; the bytes go on unchanged.
    if let Ok(Frame::Command(command)) = protocol::decode(&bytes) {
        traffic.client_command(&command);
    }
    match peer.send(Message::Binary(bytes.into())).await {
        Ok(()) => ControlFlow::Continue(()),
        Err(error) => {
            tracing::debug!(%error, "a relayed console could not reach its peer");
            ControlFlow::Break(true)
        }
    }
}

/// Take one read of the peer socket: a frame for the page into `held`, a
/// ping answered at once.
async fn from_peer(
    incoming: Option<Result<Message, tungstenite::Error>>,
    peer: &mut PeerSocket,
    held: &mut Option<Out>,
    traffic: &mut Traffic,
) -> Flow {
    let Some(Ok(message)) = incoming else {
        return ControlFlow::Break(true);
    };
    match message {
        Message::Binary(bytes) => {
            traffic.peer_frame(&bytes);
            *held = Some(Out::Frame(Vec::from(bytes)));
            ControlFlow::Continue(())
        }
        // The socket queued the pong when it read the ping; the flush sends
        // it now rather than with the next frame for the peer.
        Message::Ping(_) => match peer.flush().await {
            Ok(()) => ControlFlow::Continue(()),
            Err(error) => {
                tracing::debug!(%error, "a relayed console could not answer its peer");
                ControlFlow::Break(true)
            }
        },
        Message::Close(_) => ControlFlow::Break(true),
        Message::Pong(_) | Message::Text(_) | Message::Frame(_) => ControlFlow::Continue(()),
    }
}
