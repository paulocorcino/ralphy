//! The `console` kind: the console the open payload names, launched,
//! reattached or watched, then served on the channel. A console a peer
//! accepted goes to the peer arm.

use tokio::sync::mpsc;

use super::{peer, Channel, CloseReason, Opening};
use crate::routes::{serve_console, ConsoleEnd, SessionHost, SessionQuery, SocketAudit};

/// The writer queue depth of a console channel. The bridge keeps the rest of
/// the output in its own outbox, so a slow page slows the reading of output
/// as a blocking send did, and `session-end` waits behind one frame at most,
/// not a deep queue of output.
pub(super) const QUEUE_FRAMES: usize = 1;

/// Serve a `console` channel. The open payload is a [`SessionQuery`], with
/// its types and its rules; a payload that does not read as one is refused
/// at once.
pub(super) fn open(
    opening: Opening,
    host: SessionHost,
    who: SocketAudit,
) -> Result<Channel, CloseReason> {
    let query = match serde_json::from_value::<SessionQuery>(opening.payload.clone()) {
        Ok(query) => query,
        Err(error) => {
            tracing::debug!(%error, "a console open that does not read as a session query");
            return Err(CloseReason::Refused);
        }
    };
    Ok(Channel::spawn(move |frames| {
        serve(opening, query, host, who, frames)
    }))
}

async fn serve(
    opening: Opening,
    query: SessionQuery,
    host: SessionHost,
    who: SocketAudit,
    mut frames: mpsc::Receiver<Vec<u8>>,
) {
    match serve_console(query, host, who, &opening.out, &mut frames).await {
        ConsoleEnd::Refused => opening.end(CloseReason::Refused).await,
        ConsoleEnd::Ended => opening.end(CloseReason::Done).await,
        ConsoleEnd::Relay(relay) => peer::open(opening, relay, frames).await,
    }
}
