//! The reader of a page socket: it reads every message the page sends, keeps
//! the table of open channels, and hands each channel the inner frames the
//! page sends on it.
//!
//! The page picks each channel id, a u32 of 1 or more, and never uses one
//! twice on a socket. An open of an id already used, a piece on an id never
//! opened, an unknown kind and a piece with an unknown flag are all answered
//! with `channel.closed {reason: "refused"}`; the socket stays open.

use std::collections::{HashMap, HashSet};

use axum::extract::ws::{Message, WebSocket};
use futures_util::stream::SplitStream;
use futures_util::StreamExt;
use tokio::sync::watch;
use tokio::time::Instant;

use super::page::PageEntry;
use crate::mux::kinds::{self, Channel, CloseReason, Ctx, Opening, Request};
use crate::mux::{ChannelSender, Writer};
use crate::presence_board::MAX_REPORT_BYTES;
use crate::protocol::{self, ChannelError, Command, Frame, FrameError, Joiner};

/// The cap of a bare `channel.open`. A console open carries the fields of a
/// `/ws/session` query, and a repo ref or a startup command can pass the
/// cap of a presence report.
const MAX_OPEN_BYTES: usize = 4 * 1024;

/// One open channel: the pieces of the inner frame still due, and its kind.
struct Open {
    joiner: Joiner,
    channel: Channel,
}

/// The reading side of one page socket.
pub(super) struct Reader {
    writer: Writer,
    /// Channel 0.
    control: ChannelSender,
    page: PageEntry,
    ctx: Ctx,
    open: HashMap<u32, Open>,
    /// Every id an open, a close or a refusal named on this socket.
    used: HashSet<u32>,
}

/// Read the page until it closes the socket, the socket fails, or a frame
/// ends it. Each message, of any kind, is sent on `heard`.
pub(super) async fn read(
    mut stream: SplitStream<WebSocket>,
    mut reader: Reader,
    heard: watch::Sender<Instant>,
) {
    while let Some(Ok(message)) = stream.next().await {
        heard.send_replace(Instant::now());
        let go = match message {
            Message::Binary(bytes) => reader.take(&bytes).await,
            Message::Close(_) => false,
            // The socket answers a ping itself; a pong only counts as heard.
            Message::Text(_) | Message::Ping(_) | Message::Pong(_) => true,
        };
        if !go {
            return;
        }
    }
}

/// End the socket for a bare frame past its cap. Always `false`.
fn past_cap(len: usize) -> bool {
    tracing::debug!(len, "page socket ended: a bare frame past its cap");
    false
}

/// The id of a channel frame whose flags were refused, read from its header.
fn channel_of(bytes: &[u8]) -> Option<u32> {
    let id: [u8; 4] = bytes.get(1..5)?.try_into().ok()?;
    Some(u32::from_be_bytes(id))
}

impl Reader {
    pub(super) fn new(writer: Writer, control: ChannelSender, page: PageEntry, ctx: Ctx) -> Reader {
        Reader {
            writer,
            control,
            page,
            ctx,
            open: HashMap::new(),
            used: HashSet::new(),
        }
    }

    /// Take one binary message. `false` ends the socket.
    async fn take(&mut self, bytes: &[u8]) -> bool {
        if bytes.first() == Some(&protocol::TAG_CHANNEL) {
            return self.take_piece(bytes).await;
        }
        // A bare frame is a presence report or a control verb, both small.
        if bytes.len() > MAX_OPEN_BYTES {
            return past_cap(bytes.len());
        }
        match protocol::decode(bytes) {
            Ok(Frame::Command(command)) => {
                if bytes.len() > MAX_REPORT_BYTES && command.verb != "channel.open" {
                    return past_cap(bytes.len());
                }
                self.take_command(command).await
            }
            // Nothing else on channel 0 carries a meaning from the page.
            Ok(Frame::Terminal { .. } | Frame::Presence(_) | Frame::Channel { .. }) | Err(_) => {
                bytes.len() <= MAX_REPORT_BYTES || past_cap(bytes.len())
            }
        }
    }

    async fn take_command(&mut self, command: Command) -> bool {
        match command.verb.as_str() {
            "presence.report" => {
                self.page.report(&command.payload);
                true
            }
            "channel.open" => self.on_open(command.payload).await,
            "channel.close" => {
                self.on_close(&command.payload);
                true
            }
            // Another verb is ignored, and the socket stays open.
            _ => true,
        }
    }

    async fn take_piece(&mut self, bytes: &[u8]) -> bool {
        match protocol::decode(bytes) {
            Ok(Frame::Channel {
                channel,
                more,
                piece,
            }) => self.on_piece(channel, more, &piece).await,
            Err(FrameError::Channel(ChannelError::UnknownFlags(_))) => {
                match channel_of(bytes).filter(|id| *id >= 1) {
                    Some(id) => self.end(id, CloseReason::Refused).await,
                    None => true,
                }
            }
            // A channel frame too short for its header names no channel.
            Ok(Frame::Terminal { .. } | Frame::Command(_) | Frame::Presence(_)) | Err(_) => true,
        }
    }

    async fn on_open(&mut self, payload: serde_json::Value) -> bool {
        let Some(request) = Request::parse(payload) else {
            return true;
        };
        let id = request.channel;
        self.prune();
        if !self.used.insert(id) {
            return self.answer(id, CloseReason::Refused).await;
        }
        let Some(kind) = request.kind else {
            return self.answer(id, CloseReason::Refused).await;
        };
        let out = match kind.queue_depth() {
            Some(depth) => self.writer.channel_with_depth(id, depth),
            None => self.writer.channel(id),
        };
        let opening = Opening {
            channel: id,
            payload: request.payload,
            out,
            control: self.control.clone(),
        };
        match kinds::open(kind, opening, &self.ctx) {
            Ok(channel) => {
                let joiner = Joiner::default();
                self.open.insert(id, Open { joiner, channel });
                true
            }
            Err(reason) => self.answer(id, reason).await,
        }
    }

    /// The page closed a channel: its kind stops, and the id stays used.
    fn on_close(&mut self, payload: &serde_json::Value) {
        if let Some(id) = kinds::channel_id(payload) {
            self.open.remove(&id);
            self.used.insert(id);
        }
    }

    async fn on_piece(&mut self, id: u32, more: bool, piece: &[u8]) -> bool {
        if id == 0 {
            // Channel 0 takes bare frames only.
            return true;
        }
        self.prune();
        let Some(open) = self.open.get_mut(&id) else {
            // An id never used is unknown. An id that ended already was
            // told so with `channel.closed`, and its late pieces are dropped.
            if self.used.insert(id) {
                return self.answer(id, CloseReason::Refused).await;
            }
            return true;
        };
        match open.joiner.push(more, piece) {
            Ok(None) => true,
            Ok(Some(inner)) => {
                if !open.channel.take(inner).await {
                    self.open.remove(&id);
                }
                true
            }
            Err(ChannelError::OverCap | ChannelError::UnknownFlags(_)) => {
                self.end(id, CloseReason::TooLarge).await
            }
        }
    }

    /// End a channel the daemon refuses: its kind stops, the id stays used,
    /// and the page gets `channel.closed`.
    async fn end(&mut self, id: u32, reason: CloseReason) -> bool {
        self.open.remove(&id);
        self.used.insert(id);
        self.answer(id, reason).await
    }

    /// Send `channel.closed` for `id`. `false` when the socket is gone.
    async fn answer(&self, id: u32, reason: CloseReason) -> bool {
        self.control
            .send(kinds::closed_frame(id, reason))
            .await
            .is_ok()
    }

    /// Forget the channels whose kind ended by itself.
    fn prune(&mut self) {
        self.open.retain(|_, open| !open.channel.ended());
    }
}
