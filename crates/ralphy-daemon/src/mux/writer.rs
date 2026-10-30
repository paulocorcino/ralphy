//! The writer of one socket: one task owns the sink half and sends the
//! frames of every channel, one bounded queue per channel, in turn.
//!
//! Each turn visits the open channels in the order they were opened and
//! sends at most one wire message per channel. In [`Mode::Plain`] a message
//! is a whole inner frame, the bytes the old routes send today. In
//! [`Mode::Channels`] an inner frame is cut into channel frames of at most
//! [`protocol::MAX_PIECE`] bytes, so a large frame delays another channel by
//! one piece. Channel 0 is the socket itself and its frames leave whole and
//! bare in both modes.
//!
//! A dropped [`ChannelSender`] closes its queue: the frames already in it
//! are still sent, then the queue is freed. The task ends when the socket
//! fails, or when every [`Writer`] handle is dropped and no queue is left.
//! Every sender then reports [`WriterClosed`].

use std::collections::VecDeque;
use std::task::{Context, Poll};

use axum::extract::ws::{Message, WebSocket};
use futures_util::stream::SplitSink;
use futures_util::SinkExt;
use tokio::sync::mpsc;
use tokio::sync::mpsc::error::{SendError, TryRecvError};

use crate::protocol;

/// How many inner frames one channel may queue before its sender waits.
const QUEUE_FRAMES: usize = 64;

/// The wire form of the frames the writer sends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mode {
    /// Each inner frame is one Binary message, as the old routes send it.
    Plain,
    /// Each inner frame of a channel above 0 is cut into channel frames.
    Channels,
}

/// The socket's writer has ended: the socket is closed or failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct WriterClosed;

impl std::fmt::Display for WriterClosed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the socket writer is closed")
    }
}

impl std::error::Error for WriterClosed {}

/// The handle that opens channels on one socket's writer.
#[derive(Debug, Clone)]
pub(crate) struct Writer {
    opens: mpsc::UnboundedSender<Queue>,
}

/// Sends the inner frames of one channel to the writer.
#[derive(Debug, Clone)]
pub(crate) struct ChannelSender {
    frames: mpsc::Sender<Vec<u8>>,
}

impl Writer {
    /// Start the writer task on `sink`.
    pub(crate) fn start(sink: SplitSink<WebSocket, Message>, mode: Mode) -> Writer {
        let (opens, opened) = mpsc::unbounded_channel();
        let task = Task {
            sink,
            mode,
            opened: Some(opened),
            queues: Vec::new(),
        };
        tokio::spawn(task.run());
        Writer { opens }
    }

    /// Open the queue of `channel`. The caller opens each id once.
    pub(crate) fn channel(&self, channel: u32) -> ChannelSender {
        let (frames, rx) = mpsc::channel(QUEUE_FRAMES);
        let queue = Queue {
            channel,
            frames: rx,
            pieces: VecDeque::new(),
            closed: false,
        };
        // When the writer has ended, the queue comes back in the error and is
        // dropped here, so the new sender reports closed on its first send.
        if let Err(SendError(queue)) = self.opens.send(queue) {
            drop(queue);
        }
        ChannelSender { frames }
    }
}

impl ChannelSender {
    /// Queue one inner frame; waits while the channel's queue is full.
    pub(crate) async fn send(&self, inner: Vec<u8>) -> Result<(), WriterClosed> {
        // The error carries only the frame back; the writer has ended.
        self.frames
            .send(inner)
            .await
            .map_err(|_unsent: SendError<Vec<u8>>| WriterClosed)
    }
}

/// One channel's queue inside the writer task.
#[derive(Debug)]
struct Queue {
    channel: u32,
    frames: mpsc::Receiver<Vec<u8>>,
    /// The wire messages of the inner frame being sent, in order.
    pieces: VecDeque<Vec<u8>>,
    /// Every sender of this channel is dropped.
    closed: bool,
}

impl Queue {
    /// Take the next inner frame when no piece of the last one is due.
    fn refill(&mut self, mode: Mode) {
        if !self.pieces.is_empty() || self.closed {
            return;
        }
        match self.frames.try_recv() {
            Ok(inner) => self.load(inner, mode),
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => self.closed = true,
        }
    }

    /// Wait for the next inner frame, or for the last sender to drop.
    fn poll_refill(&mut self, cx: &mut Context<'_>, mode: Mode) -> Poll<()> {
        match self.frames.poll_recv(cx) {
            Poll::Ready(Some(inner)) => {
                self.load(inner, mode);
                Poll::Ready(())
            }
            Poll::Ready(None) => {
                self.closed = true;
                Poll::Ready(())
            }
            Poll::Pending => Poll::Pending,
        }
    }

    fn load(&mut self, inner: Vec<u8>, mode: Mode) {
        self.pieces = match mode {
            Mode::Channels if self.channel != 0 => protocol::cut(self.channel, &inner)
                .iter()
                .map(protocol::encode)
                .collect(),
            Mode::Plain | Mode::Channels => VecDeque::from([inner]),
        };
    }

    fn finished(&self) -> bool {
        self.closed && self.pieces.is_empty()
    }
}

struct Task {
    sink: SplitSink<WebSocket, Message>,
    mode: Mode,
    /// New queues; `None` once every [`Writer`] handle is dropped.
    opened: Option<mpsc::UnboundedReceiver<Queue>>,
    queues: Vec<Queue>,
}

impl Task {
    async fn run(mut self) {
        loop {
            self.take_opened();
            match self.turn().await {
                Ok(true) => {}
                Ok(false) if self.opened.is_none() && self.queues.is_empty() => return,
                Ok(false) => std::future::poll_fn(|cx| self.poll_wake(cx)).await,
                Err(error) => {
                    tracing::debug!(%error, "socket writer stopped: the socket failed");
                    return;
                }
            }
        }
    }

    fn take_opened(&mut self) {
        let Some(opened) = &mut self.opened else {
            return;
        };
        loop {
            match opened.try_recv() {
                Ok(queue) => self.queues.push(queue),
                Err(TryRecvError::Empty) => return,
                Err(TryRecvError::Disconnected) => {
                    self.opened = None;
                    return;
                }
            }
        }
    }

    /// Send at most one wire message per channel. Returns whether anything
    /// was sent; a finished queue is freed.
    async fn turn(&mut self) -> Result<bool, axum::Error> {
        let mut sent = false;
        for queue in &mut self.queues {
            queue.refill(self.mode);
            if let Some(piece) = queue.pieces.pop_front() {
                self.sink.send(Message::Binary(piece.into())).await?;
                sent = true;
            }
        }
        self.queues.retain(|queue| !queue.finished());
        Ok(sent)
    }

    /// Ready when a queue got a frame or closed, or a queue was opened, or
    /// the last handle dropped. Called only when every queue is empty.
    fn poll_wake(&mut self, cx: &mut Context<'_>) -> Poll<()> {
        let mut woke = false;
        for queue in &mut self.queues {
            woke |= queue.poll_refill(cx, self.mode).is_ready();
        }
        if let Some(opened) = &mut self.opened {
            match opened.poll_recv(cx) {
                Poll::Ready(Some(queue)) => {
                    self.queues.push(queue);
                    woke = true;
                }
                Poll::Ready(None) => {
                    self.opened = None;
                    woke = true;
                }
                Poll::Pending => {}
            }
        }
        if woke {
            Poll::Ready(())
        } else {
            Poll::Pending
        }
    }
}

#[cfg(test)]
mod tests;
