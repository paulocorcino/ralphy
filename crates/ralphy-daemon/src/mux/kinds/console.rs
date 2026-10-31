//! The `console` kind: the console the open payload names, launched,
//! reattached or watched as `/ws/session` does it, then served on the
//! channel. A console a peer accepted goes to the peer arm.

use tokio::sync::mpsc;

use super::{peer, Channel, CloseReason, Opening};
use crate::routes::{serve_console, ConsoleEnd, SessionHost, SessionQuery, SocketAudit};

/// Serve a `console` channel. The open payload carries the fields of the
/// `/ws/session` query, with their types and their rules; a payload that
/// does not read as one is refused at once.
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
