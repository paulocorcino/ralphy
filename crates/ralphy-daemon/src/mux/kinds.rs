//! The kinds of channel a page opens on `/ws`: the closed set, the reading of
//! a `channel.open`, and the one match that hands an open to its kind.
//!
//! A kind serves its channel in a task of its own ([`Channel::spawn`]). The
//! task gets the inner frames the page sends on the channel, sends its own
//! frames on the channel's queue, and ends the channel with
//! [`Opening::end`], which tells the page with `channel.closed`.

mod changes;
mod console;
mod peer;
mod spawn;
mod tree;

use std::future::Future;
use std::sync::Arc;

use tokio::sync::{broadcast, mpsc};
use tokio::task::AbortHandle;

use super::ChannelSender;
use crate::protocol::{self, Command, Frame};
use crate::routes::{CommandCtx, SessionHost, SocketAudit, TreeHost};
use crate::watch::WatcherManager;

/// How many inner frames from the page one channel may hold before the
/// socket's reader waits for the kind to take them.
const INBOUND_FRAMES: usize = 64;

/// The kinds of channel, a closed set (ADR-0082 amendment 2026-10-30).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ChannelKind {
    Console,
    Tree,
    Runs,
    Changes,
    Spawn,
}

impl ChannelKind {
    /// The kind a `channel.open` names, or `None` for a name no kind has.
    pub(crate) fn parse(name: &str) -> Option<ChannelKind> {
        match name {
            "console" => Some(ChannelKind::Console),
            "tree" => Some(ChannelKind::Tree),
            "runs" => Some(ChannelKind::Runs),
            "changes" => Some(ChannelKind::Changes),
            "spawn" => Some(ChannelKind::Spawn),
            _ => None,
        }
    }
}

/// Why the daemon ends a channel: the `reason` of its `channel.closed`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CloseReason {
    /// A duplicate or unknown id, a kind the daemon does not know, or an open
    /// its kind cannot serve.
    Refused,
    /// The pieces the page sent join to an inner frame past the cap.
    TooLarge,
    /// The kind has said all it has to say: a spawned command that ended.
    Done,
}

impl CloseReason {
    pub(crate) fn as_wire(self) -> &'static str {
        match self {
            CloseReason::Refused => "refused",
            CloseReason::TooLarge => "too-large",
            CloseReason::Done => "done",
        }
    }
}

/// The bare `channel.closed` frame that ends `channel`.
pub(crate) fn closed_frame(channel: u32, reason: CloseReason) -> Vec<u8> {
    protocol::encode(&Frame::Command(Command {
        id: 0,
        verb: "channel.closed".to_string(),
        payload: serde_json::json!({ "channel": channel, "reason": reason.as_wire() }),
    }))
}

/// One `channel.open`, read: the id the page picked, the kind it named
/// (`None` for a name no kind has), and the whole payload.
#[derive(Debug)]
pub(crate) struct Request {
    pub(crate) channel: u32,
    pub(crate) kind: Option<ChannelKind>,
    pub(crate) payload: serde_json::Value,
}

impl Request {
    /// Read a `channel.open` payload. `None` when it names no id of 1 or
    /// more that fits a u32: there is no channel to answer then.
    pub(crate) fn parse(payload: serde_json::Value) -> Option<Request> {
        let channel = channel_id(&payload)?;
        let kind = payload
            .get("kind")
            .and_then(serde_json::Value::as_str)
            .and_then(ChannelKind::parse);
        Some(Request {
            channel,
            kind,
            payload,
        })
    }
}

/// The channel id of a `channel.open` or `channel.close` payload: a u32 of
/// 1 or more. Channel 0 is the socket itself.
pub(crate) fn channel_id(payload: &serde_json::Value) -> Option<u32> {
    let id = payload.get("channel")?.as_u64()?;
    u32::try_from(id).ok().filter(|id| *id >= 1)
}

/// What the channels of one socket share: who opened the socket and the
/// daemon state the kinds serve from.
pub(crate) struct Ctx {
    pub(crate) who: SocketAudit,
    /// Whether the request that opened the socket may carry a secret.
    pub(crate) secret_ok: bool,
    pub(crate) host: SessionHost,
    pub(crate) daemon_id: Option<String>,
    pub(crate) watchers: Arc<WatcherManager>,
    pub(crate) run_exits: broadcast::Sender<String>,
}

impl Ctx {
    fn tree_host(&self) -> TreeHost {
        TreeHost {
            watchers: self.watchers.clone(),
            registry_path: self.host.registry_path.clone(),
            peers_dir: self.host.peers_dir.clone(),
            daemon_id: self.daemon_id.clone(),
        }
    }

    fn command_ctx(&self) -> CommandCtx {
        CommandCtx {
            registry_path: self.host.registry_path.clone(),
            peers_dir: self.host.peers_dir.clone(),
            daemon_id: self.daemon_id.clone(),
            sessions: self.host.sessions.clone(),
            secret_ok: self.secret_ok,
        }
    }
}

/// One open a kind is asked to serve.
pub(crate) struct Opening {
    pub(crate) channel: u32,
    pub(crate) payload: serde_json::Value,
    /// The channel's own queue on the socket's writer.
    pub(crate) out: ChannelSender,
    /// Channel 0, where the channel's `channel.closed` goes.
    pub(crate) control: ChannelSender,
}

impl Opening {
    /// End the channel: its queue closes, and `channel.closed` with `reason`
    /// goes on channel 0. A socket that is gone hears nothing.
    pub(crate) async fn end(self, reason: CloseReason) {
        let Opening {
            channel,
            payload,
            out,
            control,
        } = self;
        drop(out);
        let kind = payload.get("kind").and_then(serde_json::Value::as_str);
        tracing::debug!(channel, ?kind, reason = reason.as_wire(), "a channel ended");
        if control.send(closed_frame(channel, reason)).await.is_err() {
            // The socket is gone, and the page with it.
        }
    }
}

/// A channel its kind serves. Dropping it stops the kind's task.
#[derive(Debug)]
pub(crate) struct Channel {
    inbound: mpsc::Sender<Vec<u8>>,
    task: AbortHandle,
}

impl Channel {
    /// Run `serve` as the channel's task. `serve` gets the inner frames the
    /// page sends on the channel, joined, in order.
    pub(crate) fn spawn<F>(serve: impl FnOnce(mpsc::Receiver<Vec<u8>>) -> F) -> Channel
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let (inbound, frames) = mpsc::channel(INBOUND_FRAMES);
        let task = tokio::spawn(serve(frames)).abort_handle();
        Channel { inbound, task }
    }

    /// Hand one inner frame from the page to the kind; waits while the
    /// kind's queue is full. `false` when the kind's task has ended.
    pub(crate) async fn take(&self, inner: Vec<u8>) -> bool {
        self.inbound.send(inner).await.is_ok()
    }

    /// Whether the kind's task has ended.
    pub(crate) fn ended(&self) -> bool {
        self.inbound.is_closed()
    }
}

impl Drop for Channel {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Hand an open to its kind. `Err` refuses it at once, with the reason for
/// its `channel.closed`.
pub(crate) fn open(kind: ChannelKind, opening: Opening, ctx: &Ctx) -> Result<Channel, CloseReason> {
    match kind {
        ChannelKind::Console => console::open(opening, ctx.host.clone(), ctx.who.clone()),
        ChannelKind::Tree => tree::open(opening, ctx.tree_host(), ctx.run_exits.clone()),
        ChannelKind::Runs => tree::open_runs(opening, ctx.tree_host(), ctx.run_exits.clone()),
        ChannelKind::Changes => changes::open(opening, ctx.run_exits.subscribe()),
        ChannelKind::Spawn => spawn::open(
            opening,
            ctx.command_ctx(),
            ctx.who.clone(),
            ctx.run_exits.clone(),
            ctx.host.bound_port,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_kind_is_read_from_its_name() {
        // (name, kind)
        let rows = [
            ("console", Some(ChannelKind::Console)),
            ("tree", Some(ChannelKind::Tree)),
            ("runs", Some(ChannelKind::Runs)),
            ("changes", Some(ChannelKind::Changes)),
            ("spawn", Some(ChannelKind::Spawn)),
            ("Tree", None),
            ("", None),
        ];
        for (name, kind) in rows {
            assert_eq!(ChannelKind::parse(name), kind, "{name:?}");
        }
    }

    #[test]
    fn an_open_names_an_id_of_one_or_more() {
        // (case, payload, the id read)
        let rows = [
            ("one", serde_json::json!({ "channel": 1 }), Some(1)),
            (
                "max",
                serde_json::json!({ "channel": u32::MAX }),
                Some(u32::MAX),
            ),
            ("zero", serde_json::json!({ "channel": 0 }), None),
            (
                "past u32",
                serde_json::json!({ "channel": u64::from(u32::MAX) + 1 }),
                None,
            ),
            ("text", serde_json::json!({ "channel": "1" }), None),
            ("missing", serde_json::json!({}), None),
        ];
        for (case, payload, id) in rows {
            assert_eq!(channel_id(&payload), id, "{case}");
        }
    }
}
