//! The `console` kind. Not served on a channel yet: each open is refused.

use super::{Channel, CloseReason, Opening};
use crate::routes::{SessionHost, SocketAudit};

/// Serve a `console` channel: the console the open payload names.
pub(super) fn open(
    opening: Opening,
    _host: SessionHost,
    _who: SocketAudit,
) -> Result<Channel, CloseReason> {
    Ok(Channel::spawn(move |_frames| {
        opening.end(CloseReason::Refused)
    }))
}
