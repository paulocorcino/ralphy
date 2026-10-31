//! The `changes` kind. Not served on a channel yet: each open is refused.

use tokio::sync::broadcast;

use super::{Channel, CloseReason, Opening};

/// Serve a `changes` channel from the run exits `run_exits` hears.
pub(super) fn open(
    opening: Opening,
    _run_exits: broadcast::Receiver<String>,
) -> Result<Channel, CloseReason> {
    Ok(Channel::spawn(move |_frames| {
        opening.end(CloseReason::Refused)
    }))
}
