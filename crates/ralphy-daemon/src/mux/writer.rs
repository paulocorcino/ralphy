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
//! fails, when it has sent a Close, when [`Writer::abort`] stops it, or when
//! every [`Writer`] handle is dropped and no queue is left. Every sender
//! then reports [`WriterClosed`].

use std::collections::VecDeque;
use std::task::{Context, Poll};

use axum::extract::ws::{Message, WebSocket};
use futures_util::stream::SplitSink;
use futures_util::SinkExt;
use tokio::sync::mpsc::error::{SendError, TryRecvError};
use tokio::sync::{mpsc, oneshot};
use tokio::task::AbortHandle;

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

/// One item of a channel's queue.
#[derive(Debug)]
pub(crate) enum Out {
    /// An inner frame; [`Mode::Channels`] cuts it into pieces.
    Frame(Vec<u8>),
    /// A WebSocket Ping with this payload, sent whole.
    Ping(Vec<u8>),
    /// A WebSocket Pong with this payload, sent whole.
    Pong(Vec<u8>),
    /// A WebSocket Close. The writer sends nothing after it and ends.
    Close,
    /// Sends nothing. The writer answers on this when every item queued
    /// before it has been sent: see [`ChannelSender::sent`].
    Sent(oneshot::Sender<()>),
}

/// The socket's writer has ended: the socket is closed or failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("the socket writer is closed")]
pub(crate) struct WriterClosed;

/// The handle that opens channels on one socket's writer.
#[derive(Debug, Clone)]
pub(crate) struct Writer {
    opens: mpsc::UnboundedSender<Queue>,
    task: AbortHandle,
}

/// Sends the inner frames of one channel to the writer.
#[derive(Debug, Clone)]
pub(crate) struct ChannelSender {
    frames: mpsc::Sender<Out>,
}

/// Room for one item in a channel's queue, held until it is used.
#[derive(Debug)]
pub(crate) struct Permit(mpsc::OwnedPermit<Out>);

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
        let task = tokio::spawn(task.run()).abort_handle();
        Writer { opens, task }
    }

    /// Open the queue of `channel`. The caller opens each id once.
    pub(crate) fn channel(&self, channel: u32) -> ChannelSender {
        self.channel_with_depth(channel, QUEUE_FRAMES)
    }

    /// Open the queue of `channel`, holding at most `depth` items (at least
    /// 1) before its sender waits. The caller opens each id once.
    pub(crate) fn channel_with_depth(&self, channel: u32, depth: usize) -> ChannelSender {
        let (frames, rx) = mpsc::channel(depth.max(1));
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

    /// Wait until the writer task has ended.
    pub(crate) async fn ended(&self) {
        // The task holds the receiver of `opens` while any handle lives, and
        // drops it when it ends.
        self.opens.closed().await;
    }

    /// Stop the writer task now. The items still queued are not sent, and
    /// the sink half of the socket is dropped.
    pub(crate) fn abort(&self) {
        self.task.abort();
    }
}

impl ChannelSender {
    /// Queue one inner frame; waits while the channel's queue is full.
    pub(crate) async fn send(&self, inner: Vec<u8>) -> Result<(), WriterClosed> {
        self.put(Out::Frame(inner)).await
    }

    /// Wait until the writer has sent every item queued on this channel so
    /// far. A frame queued on another channel may leave before or after.
    pub(crate) async fn sent(&self) -> Result<(), WriterClosed> {
        let (answer, sent) = oneshot::channel();
        self.put(Out::Sent(answer)).await?;
        // The answer is dropped unsent when the writer ends first.
        sent.await
            .map_err(|_ended: oneshot::error::RecvError| WriterClosed)
    }

    /// Queue one item; waits while the channel's queue is full.
    pub(crate) async fn put(&self, out: Out) -> Result<(), WriterClosed> {
        // The error carries only the item back; the writer has ended.
        self.frames
            .send(out)
            .await
            .map_err(|_unsent: SendError<Out>| WriterClosed)
    }

    /// Wait for room for one item. Cancel safe: a wait that is dropped
    /// takes no room.
    pub(crate) async fn reserve(&self) -> Result<Permit, WriterClosed> {
        // An owned permit borrows nothing, so the caller may change its own
        // state while it holds one.
        self.frames
            .clone()
            .reserve_owned()
            .await
            .map(Permit)
            .map_err(|_closed: SendError<()>| WriterClosed)
    }
}

impl Permit {
    /// Queue `out` in the room this permit holds.
    pub(crate) fn put(self, out: Out) {
        self.0.send(out);
    }
}

/// One channel's queue inside the writer task.
#[derive(Debug)]
struct Queue {
    channel: u32,
    frames: mpsc::Receiver<Out>,
    /// The wire messages of the item being sent, in order.
    pieces: VecDeque<Message>,
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

    fn load(&mut self, out: Out, mode: Mode) {
        self.pieces = match out {
            Out::Frame(inner) => self.frame_messages(inner, mode),
            Out::Ping(payload) => VecDeque::from([Message::Ping(payload.into())]),
            Out::Pong(payload) => VecDeque::from([Message::Pong(payload.into())]),
            Out::Close => VecDeque::from([Message::Close(None)]),
            // The item is loaded only when the messages before it are sent.
            Out::Sent(answer) => {
                if answer.send(()).is_err() {
                    tracing::debug!("a sent-wait was dropped before the answer");
                }
                VecDeque::new()
            }
        };
    }

    fn frame_messages(&self, inner: Vec<u8>, mode: Mode) -> VecDeque<Message> {
        match mode {
            Mode::Channels if self.channel != 0 => protocol::cut(self.channel, &inner)
                .iter()
                .map(|piece| Message::Binary(protocol::encode(piece).into()))
                .collect(),
            Mode::Plain | Mode::Channels => VecDeque::from([Message::Binary(inner.into())]),
        }
    }

    fn finished(&self) -> bool {
        self.closed && self.pieces.is_empty()
    }
}

/// What one turn of the writer did.
enum Turn {
    /// Every queue was empty.
    Idle,
    /// At least one message was sent.
    Sent,
    /// A Close was sent: the writer ends.
    Closed,
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
                Ok(Turn::Sent) => {}
                Ok(Turn::Closed) => return,
                Ok(Turn::Idle) if self.opened.is_none() && self.queues.is_empty() => return,
                Ok(Turn::Idle) => std::future::poll_fn(|cx| self.poll_wake(cx)).await,
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

    /// Send at most one wire message per channel; a finished queue is freed.
    async fn turn(&mut self) -> Result<Turn, axum::Error> {
        let mut turn = Turn::Idle;
        for queue in &mut self.queues {
            queue.refill(self.mode);
            if let Some(message) = queue.pieces.pop_front() {
                let close = matches!(message, Message::Close(_));
                self.sink.send(message).await?;
                if close {
                    return Ok(Turn::Closed);
                }
                turn = Turn::Sent;
            }
        }
        self.queues.retain(|queue| !queue.finished());
        Ok(turn)
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
