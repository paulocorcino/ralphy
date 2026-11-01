//! The state one tree connection holds, and what each client command
//! does to it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::sync::mpsc::UnboundedSender;
use tokio::task::JoinHandle;

use super::frame::{
    dirty_push, failed_payload, local_checkout, parse_request, Alias, Request, Verb,
};
use super::{
    close_peer_tree_poller, push_command, spawn_nudge_forwarder, spawn_peer_tree_poller,
    PeerTreePoller, PeerWatchSet, TreeHost,
};
use crate::mux::ChannelSender;
use crate::peer::read_peer_store;
use crate::protocol::{self, Frame};
use crate::{checkout, fleet, peer, registry, watch};

type Key = (String, String);

pub(crate) struct TreeConn {
    watchers: Arc<watch::WatcherManager>,
    registry_path: PathBuf,
    peers_dir: PathBuf,
    daemon_id: Option<String>,
    /// The shared fan-in channel: one forwarder task per subscribed repo pipes
    /// that repo's broadcast into it, so the connection's loop watches ONE
    /// receiver regardless of how many repos/dirs it holds.
    nudge_tx: UnboundedSender<(String, String)>,
    /// Channel 0 of the socket's writer: where this connection's pushes go.
    out: ChannelSender,
    /// Keyed by repo so it is torn down when this connection releases the
    /// repo's LAST dir — and re-spawned (on the fresh broadcast the manager
    /// rebuilds) if the same repo is watched again.
    forwarders: BTreeMap<String, JoinHandle<()>>,
    peer_pollers: BTreeMap<String, PeerTreePoller>,
    /// The (repo, rel) dirs THIS connection holds, in normalized form — held at
    /// most once each (a duplicate `watch` is a no-op, so the manager refcount
    /// this connection contributes stays 1 per dir and teardown releases it
    /// exactly once). Doubles as the per-connection push filter (a repo's
    /// broadcast carries every dir, including ones other connections watch).
    watched: Vec<Key>,
    /// `(repo, prefixed rel)` → alias for the dirs this connection watches under
    /// a `checkout`. A per-connection VIEW only: the manager never reads it, and
    /// `watched` keeps the prefixed rel so the refcount/teardown logic is
    /// byte-for-byte the no-checkout one.
    aliases: BTreeMap<Key, Alias>,
}

impl TreeConn {
    pub(crate) fn new(
        host: TreeHost,
        nudge_tx: UnboundedSender<(String, String)>,
        out: ChannelSender,
    ) -> Self {
        let TreeHost {
            watchers,
            registry_path,
            peers_dir,
            daemon_id,
        } = host;
        Self {
            watchers,
            registry_path,
            peers_dir,
            daemon_id,
            nudge_tx,
            out,
            forwarders: BTreeMap::new(),
            peer_pollers: BTreeMap::new(),
            watched: Vec::new(),
            aliases: BTreeMap::new(),
        }
    }

    pub(crate) async fn on_frame(&mut self, bytes: &[u8]) {
        let Ok(Frame::Command(cmd)) = protocol::decode(bytes) else {
            return;
        };
        let Some(req) = parse_request(&cmd) else {
            return;
        };
        match req.verb {
            Verb::Watch | Verb::RunsWatch | Verb::HeadWatch => self.watch(&req).await,
            Verb::Unwatch | Verb::RunsUnwatch => self.unwatch(&req),
        }
    }

    /// A repo's broadcast carries every watched dir; push only the ones THIS
    /// connection subscribed to.
    pub(crate) async fn push_nudge(&self, nudge: Option<(String, String)>) {
        let Some((repo, rel)) = nudge else {
            return;
        };
        if !self.holds_dir(&repo, &rel) {
            return;
        }
        let alias = self.aliases.get(&(repo.clone(), rel.clone()));
        let (verb, payload) = dirty_push(&repo, &rel, alias);
        push_command(&self.out, verb, payload).await;
    }

    /// TEARDOWN: release every held dir (the last release tears the watcher
    /// down) and abort the forwarders (their broadcast receivers may otherwise
    /// outlive us if another connection keeps the repo alive). Calling it
    /// again releases nothing: the connection drops what it releases.
    pub(crate) fn teardown(&mut self) {
        let watched = std::mem::take(&mut self.watched);
        for (repo, rel) in &watched {
            if !self.peer_pollers.contains_key(repo) {
                self.watchers.unwatch(repo, rel);
            }
        }
        for forwarder in std::mem::take(&mut self.forwarders).into_values() {
            forwarder.abort();
        }
        for poller in std::mem::take(&mut self.peer_pollers).into_values() {
            close_peer_tree_poller(poller);
        }
    }

    fn holds_dir(&self, repo: &str, rel: &str) -> bool {
        self.watched.iter().any(|(r, p)| r == repo && p == rel)
    }

    fn holds_repo(&self, repo: &str) -> bool {
        self.watched.iter().any(|(r, _)| r == repo)
    }

    fn refresh_alias(&mut self, key: Key, alias: Option<&Alias>) {
        if let Some(a) = alias {
            self.aliases.insert(key, a.clone());
        }
    }

    async fn watch(&mut self, req: &Request) {
        if req.repo_ref.is_empty() {
            return;
        }
        let (descriptors, _) = read_peer_store(self.peers_dir.clone()).await;
        let local_id = self.daemon_id.clone().unwrap_or_default();
        match fleet::route(&req.repo_ref, &local_id, &descriptors) {
            fleet::Route::Local { slug } => self.watch_local(slug, req).await,
            fleet::Route::UnknownDaemon { .. } => {}
            // Local only: a peer on an older build would hold the gitdir without
            // the `HEAD`-only filter, and its `index` writes would loop against
            // the answering `git status`.
            fleet::Route::Peer { .. } if req.verb.head() => {}
            fleet::Route::Peer { peer, slug } => self.watch_peer(peer, slug, req),
        }
    }

    fn watch_peer(&mut self, peer: &peer::PeerDescriptor, slug: &str, req: &Request) {
        let key = (req.repo_ref.clone(), req.rel.clone());
        if self.watched.contains(&key) {
            // Held already: refresh the alias so a re-watch of the same dir under
            // a `checkout` is pushed in its form.
            self.refresh_alias(key, req.alias.as_ref());
            return;
        }
        let daemon_id = self.daemon_id.as_deref();
        let nudge_tx = &self.nudge_tx;
        let poller = self
            .peer_pollers
            .entry(req.repo_ref.clone())
            .or_insert_with(|| start_peer_poller(peer, slug, req, daemon_id, nudge_tx));
        poller.set.add(&req.rel, req.verb.runs());
        self.refresh_alias(key.clone(), req.alias.as_ref());
        self.watched.push(key);
    }

    async fn watch_local(&mut self, repo: &str, req: &Request) {
        let Some(root) = registry_root(&self.registry_path, repo) else {
            return;
        };
        if req.verb.runs() {
            ensure_runstate_dir(&root);
        }
        let Ok(confined) = local_checkout(req, &root) else {
            return;
        };
        let rels = if req.verb.head() {
            checkout::head_rels(&root, confined.as_ref())
        } else {
            vec![req.rel.clone()]
        };
        for rel in rels {
            self.watch_dir(&root, repo, rel, req).await;
        }
    }

    async fn watch_dir(&mut self, root: &Path, repo: &str, rel: String, req: &Request) {
        // Idempotent per connection: a repeat watch must NOT take a second
        // manager refcount this teardown would never release.
        let key = (repo.to_string(), rel.clone());
        if self.watched.contains(&key) {
            self.refresh_alias(key, req.alias.as_ref());
            return;
        }
        match self.watchers.watch(repo, root, &rel) {
            Ok(rx) => {
                // First dir for this repo on this connection → spawn its
                // forwarder on the rx the manager just handed us.
                let nudge_tx = &self.nudge_tx;
                self.forwarders
                    .entry(repo.to_string())
                    .or_insert_with(|| spawn_nudge_forwarder(rx, nudge_tx.clone()));
                self.refresh_alias(key.clone(), req.alias.as_ref());
                self.watched.push(key);
            }
            Err(e) => {
                let reason = format!("{e:#}");
                tracing::warn!(error = %reason, "tree watch failed");
                report_failure(&self.out, repo, &rel, req, &reason).await;
            }
        }
    }

    fn unwatch(&mut self, req: &Request) {
        if self.peer_pollers.contains_key(&req.repo_ref) {
            self.unwatch_peer(req);
        } else {
            self.unwatch_local(req);
        }
    }

    fn unwatch_peer(&mut self, req: &Request) {
        let key = (req.repo_ref.clone(), req.rel.clone());
        if !self.watched.contains(&key) {
            return;
        }
        self.aliases.remove(&key);
        self.watched.retain(|held| held != &key);
        let runs_held = self.holds_dir(&req.repo_ref, watch::RUNSTATE_REL);
        if let Some(poller) = self.peer_pollers.get(&req.repo_ref) {
            poller.set.remove(&req.rel, runs_held);
        }
        if !self.holds_repo(&req.repo_ref) {
            if let Some(poller) = self.peer_pollers.remove(&req.repo_ref) {
                close_peer_tree_poller(poller);
            }
        }
    }

    fn unwatch_local(&mut self, req: &Request) {
        let key = (req.repo_ref.clone(), req.rel.clone());
        if !self.watched.contains(&key) {
            return; // not held → nothing to release (no double-unwatch)
        }
        self.watchers.unwatch(&req.repo_ref, &req.rel);
        self.aliases.remove(&key);
        self.watched.retain(|k| k != &key);
        // Last dir of this repo released → stop its forwarder so a later
        // re-watch re-subscribes to the rebuilt broadcast.
        if !self.holds_repo(&req.repo_ref) {
            if let Some(f) = self.forwarders.remove(&req.repo_ref) {
                f.abort();
            }
        }
    }
}

/// A connection whose task is aborted, as a channel's is when the page closes
/// it, still releases what it holds.
impl Drop for TreeConn {
    fn drop(&mut self) {
        self.teardown();
    }
}

fn start_peer_poller(
    peer: &peer::PeerDescriptor,
    slug: &str,
    req: &Request,
    daemon_id: Option<&str>,
    nudge_tx: &UnboundedSender<(String, String)>,
) -> PeerTreePoller {
    let set = PeerWatchSet::new(&req.rel, req.verb.runs());
    let sub = format!("{}-{}", daemon_id.unwrap_or("daemon"), ulid::Ulid::new());
    let task = spawn_peer_tree_poller(
        peer.clone(),
        req.repo_ref.clone(),
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
}

fn registry_root(registry_path: &Path, repo: &str) -> Option<PathBuf> {
    match registry::load_from(registry_path) {
        Ok(store) => store.entry_or_former(repo).map(|e| PathBuf::from(&e.path)),
        Err(e) => {
            tracing::warn!(error = %e, "tree watch: registry unreadable");
            None
        }
    }
}

// `notify` errors on a missing path, and a repo where `ralphy run` never ran has
// no snapshot dir — without this, a first run started while the panel is open
// would stay invisible until reopen (ADR-0036 §4 amendment).
fn ensure_runstate_dir(root: &Path) {
    if let Err(e) = std::fs::create_dir_all(root.join(watch::RUNSTATE_REL)) {
        tracing::warn!(error = %e, "runs watch: creating the runstate dir");
    }
}

// The tree shows it no longer updates by itself (ADR-0070 D3). Paths are pushed
// like `tree.dirty`; the runs list has no line to show it, so it stays a log
// line.
async fn report_failure(out: &ChannelSender, repo: &str, rel: &str, req: &Request, reason: &str) {
    if req.verb.runs() {
        return;
    }
    let payload = failed_payload(repo, rel, req.alias.as_ref(), req.verb.head(), reason);
    push_command(out, "tree.failed", payload).await;
}
