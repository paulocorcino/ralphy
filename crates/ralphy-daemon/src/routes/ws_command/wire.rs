//! Where a command is served once it is read: one writer channel that sends
//! to the client, and what ends the serving. On a command socket of its own
//! that is the half that reads the client and the daemon's shutdown signal.
//! On a spawn channel of the page socket it is the page closing the channel.

use std::time::Duration;

use axum::extract::ws::WebSocket;
use futures_util::stream::SplitStream;
use futures_util::StreamExt;
use tokio::sync::{mpsc, watch};

use super::send_command_on;
use crate::mux::{ChannelSender, Mode, Out, Writer, WriterClosed};

/// How many frames the socket's writer queue holds. One keeps the send
/// deadline of a run's output near what a blocking send had: a client that
/// does not read holds the run's relay at its next frame.
const SEND_QUEUE: usize = 1;

/// How long [`Wire::finish`] waits for the client to take the Close. Past it
/// the writer is stopped and the socket dropped.
const CLOSE_WINDOW: Duration = Duration::from_secs(5);

/// The sending half of one served command, and what ends the serving.
pub(crate) struct Wire {
    pub(super) out: ChannelSender,
    client: Client,
}

/// The client of a [`Wire`]: what it reads, and what it sends through.
enum Client {
    /// A command socket of its own.
    Socket {
        from_browser: SplitStream<WebSocket>,
        shutdown: watch::Receiver<bool>,
        writer: Writer,
    },
    /// A spawn channel of the page socket: the inner frames the page sends
    /// on it. The page sends none that mean anything.
    Channel { frames: mpsc::Receiver<Vec<u8>> },
}

impl Wire {
    /// Split `socket` and start its writer.
    pub(super) fn start(socket: WebSocket, shutdown: watch::Receiver<bool>) -> Wire {
        let (sink, from_browser) = socket.split();
        let writer = Writer::start(sink, Mode::Plain);
        Wire {
            out: writer.channel_with_depth(0, SEND_QUEUE),
            client: Client::Socket {
                from_browser,
                shutdown,
                writer,
            },
        }
    }

    /// Serve on a spawn channel: `out` is the channel's queue on the page
    /// socket, and `frames` the inner frames the page sends on it.
    pub(crate) fn on_channel(out: ChannelSender, frames: mpsc::Receiver<Vec<u8>>) -> Wire {
        Wire {
            out,
            client: Client::Channel { frames },
        }
    }

    /// Ready when serving this wire should stop: the client sent or closed
    /// something on its socket, the daemon is shutting down, or the page
    /// closed the channel. Cancel safe. It never ends a run: see the
    /// teardown invariant on `stream_child`.
    pub(super) async fn gone(&mut self) {
        match &mut self.client {
            Client::Socket {
                from_browser,
                shutdown,
                ..
            } => {
                tokio::select! {
                    _ = shutdown.changed() => {}
                    _ = from_browser.next() => {}
                }
            }
            Client::Channel { frames } => while frames.recv().await.is_some() {},
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
    /// A channel has no socket of its own to close: it only stops.
    pub(super) async fn finish(self) {
        let Client::Socket { writer, .. } = self.client else {
            return;
        };
        let closed = async {
            self.out.put(Out::Close).await?;
            writer.ended().await;
            Ok(())
        };
        match tokio::time::timeout(CLOSE_WINDOW, closed).await {
            // Closed, or the client was already gone.
            Ok(Ok(()) | Err(WriterClosed)) => {}
            Err(_) => tracing::debug!("the client did not take the close in time"),
        }
        writer.abort();
    }
}
