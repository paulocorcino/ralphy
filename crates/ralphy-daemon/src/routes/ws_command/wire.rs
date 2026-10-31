//! Where a command is served once it is read: the queue of its `spawn`
//! channel on the page socket, and the page's frames on that channel. The
//! page closing the channel ends the serving.

use tokio::sync::mpsc;

use super::send_command_on;
use crate::mux::ChannelSender;

/// The sending half of one served command, and what ends the serving.
pub(crate) struct Wire {
    pub(super) out: ChannelSender,
    /// The inner frames the page sends on the channel. The page sends none
    /// that mean anything.
    frames: mpsc::Receiver<Vec<u8>>,
}

impl Wire {
    /// Serve on a spawn channel: `out` is the channel's queue on the page
    /// socket, and `frames` the inner frames the page sends on it.
    pub(crate) fn on_channel(out: ChannelSender, frames: mpsc::Receiver<Vec<u8>>) -> Wire {
        Wire { out, frames }
    }

    /// Ready when serving this wire should stop: the page closed the channel.
    /// Cancel safe. It never ends a run: see the teardown invariant on
    /// `stream_child`.
    pub(super) async fn gone(&mut self) {
        while self.frames.recv().await.is_some() {}
    }

    /// Queue a command frame whose loss only the client's own close explains.
    /// A closed writer means the client left; the caller's next read ends it.
    pub(super) async fn reply(&self, id: u64, verb: &str, payload: serde_json::Value) {
        if send_command_on(&self.out, id, verb, payload).await.is_err() {
            tracing::debug!(verb, "the client left before the command frame was sent");
        }
    }
}
