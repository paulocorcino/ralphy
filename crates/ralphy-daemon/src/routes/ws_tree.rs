//! The connection of a `tree` or `runs` channel of the page socket, its peer
//! poller and the run-exit nudge relay (#196, #310, ADR-0036 §4).
//!
//! A client `Frame::Command{verb:"watch", payload:{repo,path}}` starts
//! watching that repo dir (subscribing to the repo's nudge broadcast on the
//! first watch); `verb:"unwatch"` releases it. A settled change on a watched
//! dir is pushed back as `Frame::Command{verb:"tree.dirty",
//! payload:{repo,path}}`, and the browser re-reads that subtree via the
//! Observe `tree.list` path.
//!
//! A SECOND subscription kind (#300, ADR-0047 §9): `runs.watch` /
//! `runs.unwatch` hold [`watch::RUNSTATE_REL`], the repo's run-snapshot dir,
//! and a change there pushes `runs.dirty {repo}`; the browser re-reads
//! `runs.list`. Both kinds live in the SAME `watched` list, so the teardown
//! releases them identically.
//!
//! A THIRD push kind (#310, ADR-0036 amendment) is NOT watcher-fed:
//! `changes.dirty {repo}` relays the daemon-wide run-exit broadcast
//! ([`relay_run_exit`], on a `changes` channel), so it has no subscription
//! verb, no entry in `watched`, and nothing to release; the browser filters
//! it by the repo it has open.
//!
//! A FOURTH kind, `head.watch { repo, checkout? }`, holds the checkout's
//! gitdir and its `logs/` (`checkout::head_rels`); the watcher lets only
//! their `HEAD` child through, and a move pushes `head.dirty { repo,
//! checkout? }`. Local repos only. It has no unwatch verb: the browser holds
//! it on the tree's own channel, which it closes when the checkout or the
//! project changes.
//!
//! TEARDOWN INVARIANT: on EVERY exit path, the connection releases EVERY dir
//! it watched (tracked in `watched`) so the last release tears the repo
//! watcher down, and aborts its forwarder tasks. A leaked watch would keep an
//! OS watcher (and its debouncer thread) alive forever.
//!
//! The optional `checkout` argument (ADR-0063 §2) is applied LEXICALLY (the
//! shape gate only, no pointer-file read): a watch has no reply frame to say
//! `unknown checkout` in, and the `tree.list` of the same level that precedes
//! every watch does. The watched rel is PREFIXED under the same root (so the
//! manager, `watched`, the refcount and the teardown see only prefixed rels,
//! unchanged), and a per-connection alias map turns the nudge back into the
//! operator's coordinates: `tree.dirty { repo, path: <operator rel>,
//! checkout: <name> }`.

use std::path::PathBuf;
use std::sync::Arc;

use super::send_command_on;
use crate::mux::ChannelSender;
use crate::watch;

mod conn;
mod frame;
mod peer_poll;
pub(crate) use conn::TreeConn;
pub(crate) use peer_poll::*;

/// What the daemon lends to one tree connection: the watchers, the stores
/// it reads and this daemon's identity.
pub(crate) struct TreeHost {
    pub(crate) watchers: Arc<watch::WatcherManager>,
    pub(crate) registry_path: PathBuf,
    pub(crate) peers_dir: PathBuf,
    pub(crate) daemon_id: Option<String>,
}

/// Queue one push on the connection's channel. A closed writer means the
/// socket is gone, and the loop's read side ends the connection.
async fn push_command(out: &ChannelSender, verb: &str, payload: serde_json::Value) {
    if send_command_on(out, 0, verb, payload).await.is_err() {
        tracing::debug!(verb, "tree push dropped: the socket writer is closed");
    }
}

/// Relay one run-exit nudge as `changes.dirty`. Returns whether the broadcast is
/// still open.
pub(crate) async fn relay_run_exit(
    out: &ChannelSender,
    exited: Result<String, tokio::sync::broadcast::error::RecvError>,
) -> bool {
    match exited {
        Ok(repo) => {
            push_command(out, "changes.dirty", serde_json::json!({ "repo": repo })).await;
            true
        }
        // Skipped nudges are free: the browser's re-read is idempotent.
        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => true,
        // Load-bearing: a closed broadcast makes `recv()` return
        // immediately forever, which would spin the loop.
        Err(tokio::sync::broadcast::error::RecvError::Closed) => false,
    }
}

/// Pipe one repo's `tree.dirty` broadcast into the connection's fan-in channel.
/// A lag just skips ahead (the browser re-reads idempotently); a closed broadcast
/// (the repo watcher torn down) or a dropped fan-in ends the task.
pub(crate) fn spawn_nudge_forwarder(
    mut rx: watch::DirtyRx,
    tx: tokio::sync::mpsc::UnboundedSender<(String, String)>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(item) => {
                    if tx.send(item).is_err() {
                        break;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    })
}
