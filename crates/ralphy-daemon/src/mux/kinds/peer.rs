//! The `console` channel of a console a peer hosts: this daemon relays the
//! channel to a `console` channel open on the peer's page socket.
//!
//! The peer's frames go to the page whole, and the socket's writer cuts each
//! one into pieces. The page's frames go to the peer unchanged. The page
//! socket has its own ping, so the peer's pings are answered here and never
//! reach the page (ADR-0082 amendment 2026-10-30, liveness).

use std::ops::ControlFlow;
use std::time::Instant;

use tokio::sync::mpsc;

use super::{CloseReason, Opening};
use crate::mux::Out;
use crate::peer::client::{PeerChannel, PeerEvent};
use crate::protocol::{self, Frame};
use crate::routes::{hand_on, PeerRelay, PendingRecord, Traffic, PEER_CLOSED};

/// Whether the relay goes on. `Break(Some(reason))`: the channel ends with
/// `channel.closed` and `reason`; `Break(None)`: the page socket has gone,
/// and nobody is left to tell.
type Flow = ControlFlow<Option<CloseReason>>;

/// Serve a `console` channel whose open went to the peer of `relay`.
/// `frames` are the inner frames the page sends on the channel.
pub(super) async fn open(
    opening: Opening,
    relay: Box<PeerRelay>,
    mut frames: mpsc::Receiver<Vec<u8>>,
) {
    let PeerRelay {
        peer,
        traffic,
        record,
    } = *relay;
    let mut relay = Relay {
        peer: Some(peer),
        answer: Answer {
            traffic,
            record,
            framed: false,
            refused: false,
            end: None,
        },
    };
    let reason = relay.run(&opening, &mut frames).await;
    drop(relay);
    if let Some(reason) = reason {
        opening.end(reason).await;
    }
}

/// The relay of one channel. Dropped, it logs its traffic line and closes
/// the peer's channel: the kind's task can be stopped at any await, when
/// the page closes the channel or the page socket ends.
struct Relay {
    /// `None` only once the peer's channel is closed.
    peer: Option<PeerChannel>,
    answer: Answer,
}

/// What the relay learned from the peer so far.
struct Answer {
    traffic: Traffic,
    /// The audit line still due: it is written on the peer's first frame.
    record: Option<PendingRecord>,
    /// Whether the peer sent a frame on the channel.
    framed: bool,
    /// Whether the peer refused the open.
    refused: bool,
    /// The end this daemon saw, if it saw one.
    end: Option<&'static str>,
}

impl Relay {
    async fn run(
        &mut self,
        opening: &Opening,
        frames: &mut mpsc::Receiver<Vec<u8>>,
    ) -> Option<CloseReason> {
        let peer = self.peer.as_mut()?;
        let answer = &mut self.answer;
        // A frame from the peer waits here for room on the channel's queue,
        // and the peer is not read while it waits: a slow page slows the peer.
        let mut held: Option<Out> = None;
        loop {
            let flow = tokio::select! {
                frame = frames.recv() => to_peer(frame, peer, answer).await,
                permit = opening.out.reserve(), if held.is_some() => {
                    page_flow(hand_on(permit, &mut held))
                }
                event = peer.next(), if held.is_none() => answer.take(event, &mut held),
            };
            if let ControlFlow::Break(reason) = flow {
                return reason;
            }
        }
    }
}

impl Drop for Relay {
    fn drop(&mut self) {
        // A refusal is the peer's to log: the channel carried no console.
        if !self.answer.refused {
            self.answer.traffic.log(self.answer.end, Instant::now());
        }
        let Some(mut peer) = self.peer.take() else {
            return;
        };
        // The close is sent in a task of its own: a drop cannot wait.
        match tokio::runtime::Handle::try_current() {
            Ok(runtime) => {
                runtime.spawn(async move { peer.close().await });
            }
            Err(_no_runtime) => tracing::debug!("a peer channel dropped without its close"),
        }
    }
}

impl Answer {
    /// Take one event of the peer's channel: a frame for the page into
    /// `held`, or the end of the channel.
    fn take(&mut self, event: PeerEvent, held: &mut Option<Out>) -> Flow {
        match event {
            PeerEvent::Frame(bytes) => {
                self.traffic.peer_frame(&bytes);
                self.framed = true;
                if let Some(record) = self.record.take() {
                    record.write();
                }
                *held = Some(Out::Frame(bytes));
                ControlFlow::Continue(())
            }
            PeerEvent::Closed { reason } => ControlFlow::Break(Some(self.ended(&reason))),
            PeerEvent::Gone => ControlFlow::Break(Some(self.ended(""))),
        }
    }

    /// The reason of the page's channel when the peer's ended with `reason`:
    /// a refusal before any frame is a refused open, any other end is done.
    fn ended(&mut self, reason: &str) -> CloseReason {
        self.end = Some(PEER_CLOSED);
        if !self.framed && reason == CloseReason::Refused.as_wire() {
            self.refused = true;
            return CloseReason::Refused;
        }
        CloseReason::Done
    }
}

/// The relay's flow when the hand-on to the page's queue ended it: the page
/// socket is gone.
fn page_flow(flow: ControlFlow<bool>) -> Flow {
    match flow {
        ControlFlow::Continue(()) => ControlFlow::Continue(()),
        ControlFlow::Break(_) => ControlFlow::Break(None),
    }
}

/// Send one inner frame from the page to the peer, unchanged.
async fn to_peer(frame: Option<Vec<u8>>, peer: &mut PeerChannel, answer: &mut Answer) -> Flow {
    let Some(bytes) = frame else {
        // The page socket has dropped the channel.
        return ControlFlow::Break(None);
    };
    answer.traffic.inbound(bytes.len());
    // Read only for the log; the bytes go on unchanged.
    if let Ok(Frame::Command(command)) = protocol::decode(&bytes) {
        answer.traffic.client_command(&command);
    }
    match peer.send(&bytes).await {
        Ok(()) => ControlFlow::Continue(()),
        Err(error) => {
            tracing::debug!(%error, "a relayed console could not reach its peer");
            ControlFlow::Break(Some(answer.ended("")))
        }
    }
}
