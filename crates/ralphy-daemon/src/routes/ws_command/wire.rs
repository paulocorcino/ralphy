//! The command socket once its first command is read: one writer channel
//! that sends to the client, the half that reads the client, and the
//! daemon's shutdown signal.

use std::time::Duration;

use axum::extract::ws::WebSocket;
use futures_util::stream::SplitStream;
use futures_util::StreamExt;
use tokio::sync::watch;

use super::send_command_on;
use crate::mux::{ChannelSender, Mode, Out, Writer, WriterClosed};

/// How many frames the socket's writer queue holds. One keeps the send
/// deadline of a run's output near what a blocking send had: a client that
/// does not read holds the run's relay at its next frame.
const SEND_QUEUE: usize = 1;

/// How long [`Wire::finish`] waits for the client to take the Close. Past it
/// the writer is stopped and the socket dropped.
const CLOSE_WINDOW: Duration = Duration::from_secs(5);

/// The sending and reading halves of one command socket.
pub(crate) struct Wire {
    pub(super) out: ChannelSender,
    pub(super) from_browser: SplitStream<WebSocket>,
    pub(super) shutdown: watch::Receiver<bool>,
    writer: Writer,
}

impl Wire {
    /// Split `socket` and start its writer.
    pub(super) fn start(socket: WebSocket, shutdown: watch::Receiver<bool>) -> Wire {
        let (sink, from_browser) = socket.split();
        let writer = Writer::start(sink, Mode::Plain);
        Wire {
            out: writer.channel_with_depth(0, SEND_QUEUE),
            from_browser,
            shutdown,
            writer,
        }
    }

    /// Queue a command frame whose loss only the client's own close explains.
    /// A closed writer means the client left; the caller's next read ends it.
    pub(super) async fn reply(&self, id: u64, verb: &str, payload: serde_json::Value) {
        if send_command_on(&self.out, id, verb, payload).await.is_err() {
            tracing::debug!(verb, "the client left before the command frame was sent");
        }
    }

    /// Send the client a Close after the frames already queued, and wait for
    /// the writer to finish, for [`CLOSE_WINDOW`] at most. Then stop the writer.
    pub(super) async fn finish(self) {
        let closed = async {
            self.out.put(Out::Close).await?;
            self.writer.ended().await;
            Ok(())
        };
        match tokio::time::timeout(CLOSE_WINDOW, closed).await {
            // Closed, or the client was already gone.
            Ok(Ok(()) | Err(WriterClosed)) => {}
            Err(_) => tracing::debug!("the client did not take the close in time"),
        }
        self.writer.abort();
    }
}
