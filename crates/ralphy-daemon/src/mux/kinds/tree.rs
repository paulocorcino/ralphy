//! The `tree` and `runs` kinds. Not served on a channel yet: each open is
//! refused.

use tokio::sync::broadcast;

use super::{Channel, CloseReason, Opening};
use crate::routes::TreeHost;

/// Serve a `tree` channel.
pub(super) fn open(
    opening: Opening,
    _host: TreeHost,
    _run_exits: broadcast::Sender<String>,
) -> Result<Channel, CloseReason> {
    Ok(Channel::spawn(move |_frames| {
        opening.end(CloseReason::Refused)
    }))
}

/// Serve a `runs` channel.
pub(super) fn open_runs(
    opening: Opening,
    _host: TreeHost,
    _run_exits: broadcast::Sender<String>,
) -> Result<Channel, CloseReason> {
    Ok(Channel::spawn(move |_frames| {
        opening.end(CloseReason::Refused)
    }))
}
