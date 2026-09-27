//! The peer tree poller: the long poll a `/ws/tree` connection runs against
//! a peer daemon for a repo that lives there (ADR-0052 §2).

use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::peer;

/// The watch set one peer poller asks about, shared with the `/ws/tree`
/// connection that owns it. The three fields are one fact — which dirs this
/// subscription wants nudges for — and they move together, which is why the bell
/// lives here beside them rather than being remembered to ring.
#[derive(Clone)]
pub(crate) struct PeerWatchSet {
    pub(crate) paths: Arc<std::sync::Mutex<std::collections::BTreeSet<String>>>,
    pub(crate) runs: Arc<std::sync::atomic::AtomicBool>,
    /// Rung whenever the set moves, so the poll in flight — which names the OLD
    /// set — is abandoned and re-posted with the new one.
    pub(crate) changed: Arc<tokio::sync::Notify>,
}

impl PeerWatchSet {
    /// Seeded with the dir that caused the poller to exist. Seeding BEFORE the
    /// task starts is load-bearing: a first poll with an empty set is answered at
    /// once, and a fast empty answer is what the pacing exists to prevent.
    pub(crate) fn new(rel: &str, runs: bool) -> Self {
        Self {
            paths: Arc::new(std::sync::Mutex::new(std::collections::BTreeSet::from([
                rel.to_string(),
            ]))),
            runs: Arc::new(std::sync::atomic::AtomicBool::new(runs)),
            changed: Arc::new(tokio::sync::Notify::new()),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, std::collections::BTreeSet<String>> {
        self.paths
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub(crate) fn snapshot(&self) -> Vec<String> {
        self.lock().iter().cloned().collect()
    }

    pub(crate) fn runs(&self) -> bool {
        self.runs.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Add `rel` (and optionally raise the runs flag), ringing the bell if either
    /// actually moved.
    pub(crate) fn add(&self, rel: &str, runs: bool) {
        let added = self.lock().insert(rel.to_string());
        if runs {
            self.runs.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        if added || runs {
            self.changed.notify_one();
        }
    }

    pub(super) fn remove(&self, rel: &str, runs_still_held: bool) {
        self.lock().remove(rel);
        self.runs
            .store(runs_still_held, std::sync::atomic::Ordering::Relaxed);
        self.changed.notify_one();
    }
}

pub(crate) struct PeerTreePoller {
    pub(crate) task: tokio::task::JoinHandle<()>,
    pub(crate) set: PeerWatchSet,
    pub(crate) peer: peer::PeerDescriptor,
    pub(crate) sub: String,
}

/// The pause after a peer poll that came back with nothing. A poll is supposed
/// to hold for its 25 s window, so this is normally invisible — it exists for the
/// case where the peer answers at once, because the transport opens a FRESH TCP
/// connection per poll (ADR-0052 §2) and an unpaced loop then spends the host's
/// ephemeral ports at line rate. That is what took the workbench's file tree down
/// against the WSL peer on 2026-09-01: ~14k sockets in `TIME_WAIT` out of a
/// 16k-port pool, and every relayed `tree.list` failing with "address already in
/// use". Shorter than the watcher's own 300 ms debounce, so it costs no latency
/// the operator could see.
pub(crate) const PEER_POLL_QUIET_PAUSE: Duration = Duration::from_millis(250);

/// The pause after a poll that could not be honoured — unreachable peer, non-200,
/// or a body that is not a poll result. A human cadence for something no amount
/// of hurry fixes.
pub(crate) const PEER_POLL_BACKOFF: Duration = Duration::from_secs(3);

/// How long the peer is asked to hold a poll open. The peer clamps it to its own
/// maximum, so this is a request, not a promise.
pub(crate) const PEER_POLL_WINDOW_MS: u64 = 25_000;

/// How long to wait for that poll's ANSWER — the window plus room for the peer's
/// own work before and after the wait.
///
/// The margin is measured, not guessed. A peer holding a punctual 25 000 ms wait
/// answered the client 27.7 s later: its poll route sweeps expired subscriptions
/// first, and dropping the last one tears down a `notify` debouncer (a thread
/// join) before the wait even starts. With a deadline of 27 s that missed by
/// 800 ms — so EVERY poll failed, the poller sat on its backoff, and no
/// `tree.dirty` ever reached the browser. A file created on the peer simply
/// never appeared (2026-09-01).
///
/// Slack is cheap here and a tight budget is not: a peer that is actually gone
/// fails at CONNECT within [`peer::client::PEER_TIMEOUT`], so this deadline is
/// only ever spent on a peer that accepted the request and went quiet.
pub(crate) const PEER_POLL_DEADLINE: Duration = Duration::from_secs(40);

/// What one peer poll produced, and therefore how long to wait before the next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PollCycle {
    /// The peer named changed dirs. Re-poll at once — something is moving, and
    /// the watcher's debounce already bounds how fast that can repeat.
    Changed,
    /// A well-formed answer carrying no change.
    Quiet,
    /// The peer answered 200 with something that is not a poll result (an error
    /// body, or no `dirty` array). Treated as a failure, never as quiet: reading
    /// it as "no changes" is what turned a refused subscription into a hot loop.
    Unreadable,
    /// The peer could not be reached, or answered a non-200.
    Failed,
    /// The watch set moved while the poll was in flight, so the poll in flight is
    /// asking the wrong question. Abandoned and re-posted.
    Restarted,
}

pub(crate) fn next_poll_delay(cycle: PollCycle) -> Duration {
    match cycle {
        PollCycle::Changed => Duration::ZERO,
        // The same short pause as a quiet cycle, doing a second job: a browser
        // opening a project sends its `watch` frames in a burst, and pausing
        // coalesces the burst into ONE re-post instead of one per frame.
        PollCycle::Quiet | PollCycle::Restarted => PEER_POLL_QUIET_PAUSE,
        PollCycle::Unreadable | PollCycle::Failed => PEER_POLL_BACKOFF,
    }
}

pub(crate) fn spawn_peer_tree_poller(
    peer: peer::PeerDescriptor,
    repo_ref: String,
    slug: String,
    sub: String,
    set: PeerWatchSet,
    nudge_tx: tokio::sync::mpsc::UnboundedSender<(String, String)>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        // Counted so a peer that is simply down is reported ONCE rather than
        // every three seconds for as long as the workbench stays open.
        let mut failures: u32 = 0;
        loop {
            let body = serde_json::json!({
                "sub": sub,
                "repo": slug,
                "paths": set.snapshot(),
                "runs": set.runs(),
                "timeout_ms": PEER_POLL_WINDOW_MS,
            });
            let started = Instant::now();
            // A poll names its watch set, so the peer only ever learns about a dir
            // a REQUEST mentioned. A folder expanded while a poll is in flight
            // would otherwise go unwatched until that poll's window ran out — and
            // the first poll of all races the browser's opening burst of `watch`
            // frames, so the tree opened blind for a full window. Abandon the
            // question that is already out of date and ask the new one.
            let request = peer::client::post_json_timeout(
                &peer,
                "/api/peer/tree/poll",
                &body,
                PEER_POLL_DEADLINE,
            );
            tokio::pin!(request);
            let answer = tokio::select! {
                answer = &mut request => answer,
                _ = set.changed.notified() => {
                    tracing::debug!(repo = %repo_ref, sub = %sub, "peer tree poll restarted: the watch set moved");
                    failures = 0;
                    tokio::time::sleep(next_poll_delay(PollCycle::Restarted)).await;
                    continue;
                }
            };
            let cycle = match answer {
                Ok((200, bytes)) => {
                    let parsed = serde_json::from_slice::<serde_json::Value>(&bytes);
                    match parsed
                        .as_ref()
                        .ok()
                        .and_then(|value| value.get("dirty"))
                        .and_then(|dirty| dirty.as_array())
                    {
                        Some(dirty) => {
                            let mut changed = false;
                            for item in dirty {
                                if let Some(path) = item["path"].as_str() {
                                    changed = true;
                                    if nudge_tx.send((repo_ref.clone(), path.to_string())).is_err()
                                    {
                                        return;
                                    }
                                }
                            }
                            if changed {
                                PollCycle::Changed
                            } else {
                                PollCycle::Quiet
                            }
                        }
                        None => {
                            tracing::warn!(
                                repo = %repo_ref,
                                body = %String::from_utf8_lossy(&bytes).chars().take(200).collect::<String>(),
                                "a peer answered a tree poll with something that is not a poll result"
                            );
                            PollCycle::Unreadable
                        }
                    }
                }
                Ok((code, _)) => {
                    tracing::warn!(repo = %repo_ref, status = code, "a peer refused a tree poll");
                    PollCycle::Failed
                }
                Err(error) => {
                    // The first failure of a streak is a WARN because it names the
                    // cause — "address already in use" here is the port pool going
                    // dry, which is otherwise invisible until the whole panel dies.
                    if failures == 0 {
                        tracing::warn!(repo = %repo_ref, error = %format!("{error:#}"), "a peer tree poll failed");
                    } else {
                        tracing::debug!(repo = %repo_ref, failures, error = %format!("{error:#}"), "a peer tree poll failed again");
                    }
                    PollCycle::Failed
                }
            };
            if cycle == PollCycle::Failed || cycle == PollCycle::Unreadable {
                failures = failures.saturating_add(1);
            } else {
                failures = 0;
            }
            tracing::debug!(
                repo = %repo_ref,
                sub = %sub,
                cycle = ?cycle,
                took_ms = started.elapsed().as_millis() as u64,
                "peer tree poll cycle"
            );
            let delay = next_poll_delay(cycle);
            if !delay.is_zero() {
                tokio::time::sleep(delay).await;
            }
        }
    })
}

pub(crate) fn close_peer_tree_poller(poller: PeerTreePoller) {
    poller.task.abort();
    tokio::spawn(async move {
        let body = serde_json::json!({ "sub": poller.sub });
        let _ = peer::client::post_json(&poller.peer, "/api/peer/tree/close", &body).await;
    });
}
