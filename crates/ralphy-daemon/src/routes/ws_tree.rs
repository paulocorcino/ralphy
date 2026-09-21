//! `/ws/tree`: the live file-tree socket, its peer poller and the run-exit
//! nudge relay (#196, #310).

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::extract::ws::{Message, WebSocket};

use super::{read_peer_store, send_command};
use crate::protocol::Frame;
use crate::{checkout, fleet, peer, protocol, registry, watch};

/// `GET /ws/tree`: the persistent live-tree subscription socket (#196, ADR-0036
/// §4). A client `Frame::Command{verb:"watch", payload:{repo,path}}` starts
/// watching that repo dir (subscribing to the repo's nudge broadcast on the first
/// watch); `verb:"unwatch"` releases it. A settled change on a watched dir is
/// pushed back as `Frame::Command{verb:"tree.dirty", payload:{repo,path}}`, and
/// the browser re-reads that subtree via the Observe `tree.list` path.
///
/// The socket carries a SECOND subscription kind (#300, ADR-0047 §9): `runs.watch`
/// / `runs.unwatch` hold [`watch::RUNSTATE_REL`], the repo's run-snapshot dir, and
/// a change there pushes `runs.dirty {repo}` — the browser re-reads `runs.list`.
/// Both kinds live in the SAME `watched` list, so the teardown below releases them
/// identically.
///
/// A THIRD push kind (#310, ADR-0036 amendment) is NOT watcher-fed: `changes.dirty
/// {repo}` relays the daemon-wide run-exit broadcast, so it has no subscription
/// verb, no entry in `watched`, and nothing to release — the browser filters it by
/// the repo it has open.
///
/// TEARDOWN INVARIANT: on EVERY exit path — daemon shutdown OR client close/error
/// — the connection releases EVERY dir it watched (tracked in `watched`) so the
/// last release tears the repo watcher down, and aborts its forwarder tasks. A
/// leaked watch would keep an OS watcher (and its debouncer thread) alive forever.
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

    fn remove(&self, rel: &str, runs_still_held: bool) {
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

/// The `/ws/tree` subscription socket (ADR-0036 §4): `watch`/`unwatch` a repo
/// dir and receive `tree.dirty` nudges; `runs.watch` for the runstate dir. The
/// optional `checkout` argument (ADR-0063 §2) is applied LEXICALLY — the shape
/// gate only, no pointer-file read: a watch has no reply frame to say `unknown
/// checkout` in, and the `tree.list` of the same level that precedes every
/// watch does. The watched rel is PREFIXED under the same root (so the manager,
/// `watched`, the refcount and the teardown see only prefixed rels, unchanged),
/// and a per-connection alias map turns the nudge back into the operator's
/// coordinates: `tree.dirty { repo, path: <operator rel>, checkout: <name> }`.
pub(crate) async fn tree_ws(
    mut socket: WebSocket,
    watchers: Arc<watch::WatcherManager>,
    registry_path: PathBuf,
    peers_dir: PathBuf,
    daemon_id: Option<String>,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
    run_exits: tokio::sync::broadcast::Sender<String>,
) {
    // Fan-in: one forwarder task per subscribed repo pipes that repo's broadcast
    // into this shared channel, so the select! loop watches ONE receiver regardless
    // of how many repos/dirs the connection holds. Keyed by repo so it is torn down
    // when this connection releases the repo's LAST dir — and re-spawned (on the
    // fresh broadcast the manager rebuilds) if the same repo is watched again.
    let (nudge_tx, mut nudge_rx) = tokio::sync::mpsc::unbounded_channel::<(String, String)>();
    let mut forwarders: std::collections::BTreeMap<String, tokio::task::JoinHandle<()>> =
        std::collections::BTreeMap::new();
    let mut peer_pollers: std::collections::BTreeMap<String, PeerTreePoller> =
        std::collections::BTreeMap::new();
    // The (repo, rel) dirs THIS connection holds, in normalized form — held at most
    // once each (a duplicate `watch` is a no-op, so the manager refcount this
    // connection contributes stays 1 per dir and teardown releases it exactly once).
    // Doubles as the per-connection push filter (a repo's broadcast carries every
    // dir, including ones other connections watch).
    let mut watched: Vec<(String, String)> = Vec::new();
    // `(repo, prefixed rel)` → `(operator rel, checkout name)` for the dirs this
    // connection watches under a `checkout`. A per-connection VIEW only: the
    // manager never reads it, and `watched` keeps the prefixed rel so the
    // refcount/teardown logic above is byte-for-byte the no-checkout one.
    let mut aliases: std::collections::BTreeMap<(String, String), (String, String)> =
        std::collections::BTreeMap::new();
    // The run-completion nudge bus (#310): daemon-wide, held by NO watch, so it
    // needs no subscription verb and adds nothing to the teardown below. Every
    // connection relays every nudge; the browser filters by its open repo.
    let mut run_exits_rx = run_exits.subscribe();
    let mut exits_open = true;

    loop {
        tokio::select! {
            _ = shutdown.changed() => break,
            incoming = socket.recv() => match incoming {
                Some(Ok(Message::Binary(bytes))) => {
                    let Ok(Frame::Command(cmd)) = protocol::decode(&bytes) else {
                        continue;
                    };
                    let repo_ref = cmd
                        .payload
                        .get("repo")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let rel = watch::norm_rel(
                        cmd.payload.get("path").and_then(|v| v.as_str()).unwrap_or(""),
                    );
                    // `checkout` prefixes the rel under the same root (shape gate
                    // only — see the fn doc); a malformed name drops the frame the
                    // way an unknown route does. `runs.*` ignore it: the runstate
                    // dir is the primary's (ADR-0063 §7).
                    let runs = cmd.verb == "runs.watch" || cmd.verb == "runs.unwatch";
                    // Same door as `checkout::from_payload`: absent or `null` is
                    // the primary; anything else must pass the gate or the frame
                    // is dropped — a non-string or `""` never silently holds a
                    // PRIMARY watch the client believes is the worktree's.
                    let checkout = match cmd.payload.get("checkout") {
                        None | Some(serde_json::Value::Null) => None,
                        Some(_) if runs => None,
                        Some(serde_json::Value::String(name)) => Some(checkout::lexical(name)),
                        Some(_) => Some(None),
                    };
                    let (rel, alias) = match checkout {
                        None => (rel, None),
                        Some(Some(c)) => (c.prefix(&rel), Some((rel, c.name().to_string()))),
                        Some(None) => continue,
                    };
                    match cmd.verb.as_str() {
                        "watch" | "runs.watch" => {
                            if repo_ref.is_empty() {
                                continue;
                            }
                            // The runs subscription ignores the payload path: its dir is
                            // fixed (ADR-0047 §9), so a client cannot aim it elsewhere.
                            let rel = if runs { watch::RUNSTATE_REL.to_string() } else { rel };
                            let (descriptors, _) = read_peer_store(peers_dir.clone()).await;
                            let repo = match fleet::route(
                                &repo_ref,
                                daemon_id.as_deref().unwrap_or(""),
                                &descriptors,
                            ) {
                                fleet::Route::Local { slug } => slug.to_string(),
                                fleet::Route::UnknownDaemon { .. } => continue,
                                fleet::Route::Peer { peer, slug } => {
                                    let key = (repo_ref.clone(), rel.clone());
                                    if watched.contains(&key) {
                                        // Held already: refresh the alias so a re-watch of the
                                        // same dir under a `checkout` is pushed in its form.
                                        if let Some(a) = alias {
                                            aliases.insert(key, a);
                                        }
                                        continue;
                                    }
                                    let poller = peer_pollers.entry(repo_ref.clone()).or_insert_with(|| {
                                        let set = PeerWatchSet::new(&rel, runs);
                                        let sub = format!(
                                            "{}-{}",
                                            daemon_id.as_deref().unwrap_or("daemon"),
                                            ulid::Ulid::new()
                                        );
                                        let task = spawn_peer_tree_poller(
                                            peer.clone(),
                                            repo_ref.clone(),
                                            slug.to_string(),
                                            sub.clone(),
                                            set.clone(),
                                            nudge_tx.clone(),
                                        );
                                        PeerTreePoller {
                                            task,
                                            set,
                                            peer: peer.clone(),
                                            sub,
                                        }
                                    });
                                    poller.set.add(&rel, runs);
                                    if let Some(a) = alias {
                                        aliases.insert(key.clone(), a);
                                    }
                                    watched.push(key);
                                    continue;
                                }
                            };
                            // Idempotent per connection: a repeat watch must NOT take a
                            // second manager refcount this teardown would never release.
                            let key = (repo.clone(), rel.clone());
                            if watched.contains(&key) {
                                if let Some(a) = alias {
                                    aliases.insert(key, a);
                                }
                                continue;
                            }
                            let root = match registry::load_from(&registry_path) {
                                Ok(store) => store.entry(&repo).map(|e| PathBuf::from(&e.path)),
                                Err(e) => {
                                    tracing::warn!(error = %e, "tree watch: registry unreadable");
                                    None
                                }
                            };
                            let Some(root) = root else { continue };
                            if runs {
                                // `notify` errors on a missing path, and a repo where
                                // `ralphy run` never ran has no snapshot dir — without
                                // this, a first run started while the panel is open would
                                // stay invisible until reopen (ADR-0036 §4 amendment).
                                if let Err(e) = std::fs::create_dir_all(root.join(watch::RUNSTATE_REL))
                                {
                                    tracing::warn!(error = %e, "runs watch: creating the runstate dir");
                                }
                            }
                            match watchers.watch(&repo, &root, &rel) {
                                Ok(rx) => {
                                    // First dir for this repo on this connection → spawn its
                                    // forwarder on the rx the manager just handed us.
                                    forwarders
                                        .entry(repo.clone())
                                        .or_insert_with(|| spawn_nudge_forwarder(rx, nudge_tx.clone()));
                                    if let Some(a) = alias {
                                        aliases.insert(key.clone(), a);
                                    }
                                    watched.push(key);
                                }
                                Err(e) => tracing::warn!(error = %e, "tree watch failed"),
                            }
                        }
                        "unwatch" | "runs.unwatch" => {
                            let rel = if runs {
                                watch::RUNSTATE_REL.to_string()
                            } else {
                                rel
                            };
                            if let Some(poller) = peer_pollers.get(&repo_ref) {
                                let key = (repo_ref.clone(), rel.clone());
                                if !watched.contains(&key) {
                                    continue;
                                }
                                aliases.remove(&key);
                                watched.retain(|held| held != &key);
                                poller.set.remove(
                                    &rel,
                                    watched.iter().any(|(repo, path)| {
                                        repo == &repo_ref && path == watch::RUNSTATE_REL
                                    }),
                                );
                                if !watched.iter().any(|(repo, _)| repo == &repo_ref) {
                                    if let Some(poller) = peer_pollers.remove(&repo_ref) {
                                        close_peer_tree_poller(poller);
                                    }
                                }
                                continue;
                            }
                            let repo = repo_ref.clone();
                            let key = (repo.clone(), rel.clone());
                            if !watched.contains(&key) {
                                continue; // not held → nothing to release (no double-unwatch)
                            }
                            watchers.unwatch(&repo, &rel);
                            aliases.remove(&key);
                            watched.retain(|k| k != &key);
                            // Last dir of this repo released → stop its forwarder so a later
                            // re-watch re-subscribes to the rebuilt broadcast.
                            if !watched.iter().any(|(r, _)| r == &repo) {
                                if let Some(f) = forwarders.remove(&repo) {
                                    f.abort();
                                }
                            }
                        }
                        _ => {}
                    }
                }
                Some(Ok(Message::Close(_))) | None => break,
                Some(Ok(_)) => {}
                Some(Err(_)) => break,
            },
            nudge = nudge_rx.recv() => {
                // A repo's broadcast carries every watched dir; push only the ones
                // THIS connection subscribed to.
                if let Some((repo, rel)) = nudge {
                    if watched.iter().any(|(r, p)| r == &repo && p == &rel) {
                        // Discriminated by REL, not by the verb that subscribed, so
                        // both subscription kinds share ONE `watched` list (and one
                        // exactly-once teardown). The browser's two consumers react
                        // differently: re-read a subtree vs re-read `runs.list`.
                        if rel == watch::RUNSTATE_REL {
                            send_command(
                                &mut socket,
                                0,
                                "runs.dirty",
                                serde_json::json!({ "repo": repo }),
                            )
                            .await;
                        } else {
                            // A checkout watch pushes the OPERATOR's rel plus the
                            // name, never the prefixed dir it is held under.
                            let payload = match aliases.get(&(repo.clone(), rel.clone())) {
                                Some((shown, name)) => serde_json::json!({
                                    "repo": repo, "path": shown, "checkout": name,
                                }),
                                None => serde_json::json!({ "repo": repo, "path": rel }),
                            };
                            send_command(&mut socket, 0, "tree.dirty", payload).await;
                        }
                    }
                }
            }
            exited = run_exits_rx.recv(), if exits_open => match exited {
                Ok(repo) => {
                    send_command(
                        &mut socket,
                        0,
                        "changes.dirty",
                        serde_json::json!({ "repo": repo }),
                    )
                    .await;
                }
                // Skipped nudges are free: the browser's re-read is idempotent.
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                // Load-bearing: a closed broadcast makes `recv()` return
                // immediately forever, which would spin this loop.
                Err(tokio::sync::broadcast::error::RecvError::Closed) => exits_open = false,
            },
        }
    }

    // TEARDOWN: release every held dir (the last release tears the watcher down)
    // and abort the forwarders (their broadcast receivers may otherwise outlive us
    // if another connection keeps the repo alive).
    for (repo, rel) in &watched {
        if !peer_pollers.contains_key(repo) {
            watchers.unwatch(repo, rel);
        }
    }
    for (_repo, forwarder) in forwarders {
        forwarder.abort();
    }
    for (_repo, poller) in peer_pollers {
        close_peer_tree_poller(poller);
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
