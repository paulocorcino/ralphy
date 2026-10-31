//! The `console` channel of a console a peer hosts. Not served on a
//! channel yet: each open is refused.

use super::{Channel, CloseReason, Opening};
use crate::routes::{SessionHost, SocketAudit};

/// Serve a `console` channel whose console a peer hosts.
#[expect(
    dead_code,
    reason = "the console kind calls it once it serves a console a peer hosts"
)]
pub(super) fn open(
    opening: Opening,
    _host: SessionHost,
    _who: SocketAudit,
) -> Result<Channel, CloseReason> {
    Ok(Channel::spawn(move |_frames| {
        opening.end(CloseReason::Refused)
    }))
}
