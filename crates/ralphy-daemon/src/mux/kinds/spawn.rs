//! The `spawn` kind. Not served on a channel yet: each open is refused.

use tokio::sync::broadcast;

use super::{Channel, CloseReason, Opening};
use crate::routes::{CommandCtx, SocketAudit};

/// Serve a `spawn` channel: the command of the open payload.
pub(super) fn open(
    opening: Opening,
    _command: CommandCtx,
    _who: SocketAudit,
    _run_exits: broadcast::Sender<String>,
    _bound_port: u16,
) -> Result<Channel, CloseReason> {
    Ok(Channel::spawn(move |_frames| {
        opening.end(CloseReason::Refused)
    }))
}
