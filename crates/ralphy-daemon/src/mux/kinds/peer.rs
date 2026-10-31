//! The `console` channel of a console a peer hosts. Not served on a
//! channel yet: the peer socket is closed and the channel is refused.

use tokio::sync::mpsc;

use super::{CloseReason, Opening};
use crate::routes::{close_peer_session, PeerRelay};

/// Serve a `console` channel whose open the peer of `relay` accepted.
/// `frames` are the inner frames the page sends on the channel.
pub(super) async fn open(
    opening: Opening,
    relay: Box<PeerRelay>,
    _frames: mpsc::Receiver<Vec<u8>>,
) {
    let PeerRelay { mut peer, traffic } = *relay;
    close_peer_session(&mut peer).await;
    traffic.log(
        Some(CloseReason::Refused.as_wire()),
        std::time::Instant::now(),
    );
    opening.end(CloseReason::Refused).await;
}
