//! The live file-tree watcher (ADR-0036 §4, issue #196): the daemon-side half of
//! the workbench's live tree. A [`WatcherManager`] owns one debounced OS watcher
//! per repo, watching only the directories a browser has actually expanded (a
//! refcounted, lazy watch-set bounded by the screen, not the repo). Each settled
//! event storm is mapped to the watched directory it touched and broadcast as one
//! `tree.dirty` nudge; the `/ws/tree` handler ([`crate::lib`]) turns a nudge into
//! a client push, and the browser re-reads the changed subtree via the existing
//! Observe [`crate::tree::list`] path.
//!
//! Concurrency bridge (mirrors `session.rs`): the notify/debouncer thread forwards
//! raw `Vec<DebouncedEvent>` batches over an `mpsc::unbounded_channel`; a tokio
//! PUMP task maps each event path's parent to a watched rel dir, drops
//! [`crate::tree::HARD_EXCLUDE`] / gitignored noise, dedups per batch, and
//! `broadcast::send`s one nudge per distinct dir. The async stack stays confined
//! to this crate.
//!
//! When the watched-dir count crosses `max_watches`, a repo's debouncer is rebuilt
//! on a [`notify::PollWatcher`] (degrade, never fail); the broadcast `Sender` is
//! preserved across the rebuild, so existing subscribers keep receiving.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result};
use ignore::gitignore::Gitignore;
use notify::{PollWatcher, RecursiveMode};
use notify_debouncer_full::{
    new_debouncer, new_debouncer_opt, DebounceEventResult, DebouncedEvent, Debouncer,
    RecommendedCache,
};
use tokio::sync::{broadcast, mpsc};

use crate::tree::HARD_EXCLUDE;

/// Default cap on the total watched-dir count before a repo degrades to polling.
/// Bounded by the screen (the expanded set), so the native path comfortably fits.
pub const MAX_WATCHES: usize = 512;

/// After this quiet gap the debouncer emits a settled batch — short so the suite
/// stays fast, long enough to coalesce an event storm into few nudges.
const DEBOUNCE_TIMEOUT: Duration = Duration::from_millis(300);

/// Poll cadence once a repo degrades to [`notify::PollWatcher`].
const POLL_INTERVAL: Duration = Duration::from_millis(500);

/// Broadcast backlog per repo. A nudge is a tiny `(repo, rel)` pair; a slow
/// subscriber that lags just re-reads once it catches up (idempotent).
const BROADCAST_CAP: usize = 256;

/// A `tree.dirty` receiver: `(repo slug, rel dir)` — the dir whose one-level
/// listing may have changed and should be re-fetched.
pub type DirtyRx = broadcast::Receiver<(String, String)>;

/// Whether a repo's watcher is the native OS backend or the polling fallback.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatchMode {
    Native,
    Poll,
}

/// Either backend behind one repo's debouncer. `notify::PollWatcher` is a drop-in
/// `Watcher`, so degrade-not-fail is one enum flip; `watch`/`unwatch` dispatch.
enum AnyDebouncer {
    Native(Debouncer<notify::RecommendedWatcher, RecommendedCache>),
    Poll(Debouncer<PollWatcher, RecommendedCache>),
}

impl AnyDebouncer {
    fn watch(&mut self, path: &Path, mode: RecursiveMode) -> notify::Result<()> {
        match self {
            AnyDebouncer::Native(d) => d.watch(path, mode),
            AnyDebouncer::Poll(d) => d.watch(path, mode),
        }
    }

    fn unwatch(&mut self, path: &Path) -> notify::Result<()> {
        match self {
            AnyDebouncer::Native(d) => d.unwatch(path),
            AnyDebouncer::Poll(d) => d.unwatch(path),
        }
    }
}

/// One repo's watcher: its debouncer, canonical root, refcounted watch-set, the
/// nudge broadcaster, and its current mode. `watches` (refcounts) is manager-owned
/// under the outer lock; `watch_set` mirrors its keys for the pump to read without
/// touching the outer lock.
struct RepoWatcher {
    repo: String,
    root: PathBuf,
    debouncer: AnyDebouncer,
    watches: BTreeMap<String, u32>,
    watch_set: Arc<Mutex<BTreeSet<String>>>,
    gitignore: Arc<Gitignore>,
    tx: broadcast::Sender<(String, String)>,
    mode: WatchMode,
}

impl RepoWatcher {
    fn new(repo: &str, canon_root: PathBuf) -> Result<Self> {
        // Build the gitignore matcher from the repo's `.gitignore` (parent = root),
        // shared read-only with the pump.
        let (gi, _) = Gitignore::new(canon_root.join(".gitignore"));
        let gitignore = Arc::new(gi);
        let (tx, _rx0) = broadcast::channel(BROADCAST_CAP);
        let watch_set = Arc::new(Mutex::new(BTreeSet::new()));
        let debouncer = spawn_backend(
            WatchMode::Native,
            repo.to_string(),
            canon_root.clone(),
            watch_set.clone(),
            gitignore.clone(),
            tx.clone(),
        )?;
        Ok(Self {
            repo: repo.to_string(),
            root: canon_root,
            debouncer,
            watches: BTreeMap::new(),
            watch_set,
            gitignore,
            tx,
            mode: WatchMode::Native,
        })
    }

    /// Absolute path of a watched rel dir (`""` → the root).
    fn target(&self, rel: &str) -> PathBuf {
        if rel.is_empty() {
            self.root.clone()
        } else {
            self.root.join(rel)
        }
    }

    /// Rebuild this repo's debouncer on [`notify::PollWatcher`] and re-add every
    /// watched dir. The broadcast `Sender` is REUSED, so existing subscribers keep
    /// receiving; the old debouncer (and its pump) tear down as it drops.
    fn degrade_to_poll(&mut self) -> Result<()> {
        if self.mode == WatchMode::Poll {
            return Ok(());
        }
        let mut new_deb = spawn_backend(
            WatchMode::Poll,
            self.repo.clone(),
            self.root.clone(),
            self.watch_set.clone(),
            self.gitignore.clone(),
            self.tx.clone(),
        )?;
        let dirs: Vec<String> = self.watches.keys().cloned().collect();
        for rel in &dirs {
            let target = self.target(rel);
            new_deb
                .watch(&target, RecursiveMode::NonRecursive)
                .with_context(|| format!("re-watching {} under poll", target.display()))?;
        }
        self.debouncer = new_deb;
        self.mode = WatchMode::Poll;
        Ok(())
    }
}

/// Shared, refcounted registry of live per-repo watchers. Cheap to clone via
/// `Arc` at the router; every `/ws/tree` connection watches/unwatches through it.
pub struct WatcherManager {
    repos: Mutex<BTreeMap<String, RepoWatcher>>,
    max_watches: usize,
}

impl WatcherManager {
    pub fn new(max_watches: usize) -> Self {
        Self {
            repos: Mutex::new(BTreeMap::new()),
            max_watches: max_watches.max(1),
        }
    }

    /// Subscribe to `repo`'s `tree.dirty` nudges and watch `rel` (`""` → the repo
    /// root). Creates the repo watcher on first use and the underlying OS watch on
    /// the rel dir's first subscriber; a repeat `watch` of the same dir just bumps
    /// its refcount. Crossing `max_watches` (total watched dirs) degrades this
    /// repo to polling. `root` need not be canonical — it is canonicalized here so
    /// event paths (which notify reports canonicalized) compare cleanly.
    pub fn watch(&self, repo: &str, root: &Path, rel: &str) -> Result<DirtyRx> {
        let rel = norm_rel(rel);
        // Reject traversal LEXICALLY (like `confine`): a `..` component would make
        // `root.join(rel)` establish an OS watch OUTSIDE the repo root. Nudges for
        // such paths are dropped downstream, but the out-of-root watch itself is
        // resource abuse — refuse it before touching the debouncer.
        if rel.split('/').any(|c| c == "..") {
            anyhow::bail!("watch path escapes the repo root: {rel}");
        }
        let canon_root = std::fs::canonicalize(root)
            .with_context(|| format!("canonicalizing watch root {}", root.display()))?;
        let mut repos = self.repos.lock().unwrap();
        if !repos.contains_key(repo) {
            repos.insert(repo.to_string(), RepoWatcher::new(repo, canon_root)?);
        }
        let rx = {
            let rw = repos.get_mut(repo).expect("just inserted");
            if rw.watches.get(&rel).copied().unwrap_or(0) == 0 {
                let target = rw.target(&rel);
                rw.debouncer
                    .watch(&target, RecursiveMode::NonRecursive)
                    .with_context(|| format!("watching {}", target.display()))?;
                rw.watch_set.lock().unwrap().insert(rel.clone());
            }
            *rw.watches.entry(rel.clone()).or_insert(0) += 1;
            rw.tx.subscribe()
        };
        let total: usize = repos.values().map(|r| r.watches.len()).sum();
        if total > self.max_watches {
            repos.get_mut(repo).expect("present").degrade_to_poll()?;
        }
        Ok(rx)
    }

    /// Release one subscriber's hold on `repo`'s `rel` dir. On the last release the
    /// underlying OS watch is dropped; when a repo has no watched dirs left its
    /// whole watcher (debouncer + pump) tears down. Unknown repo/dir is a no-op.
    pub fn unwatch(&self, repo: &str, rel: &str) {
        let rel = norm_rel(rel);
        let mut repos = self.repos.lock().unwrap();
        let Some(rw) = repos.get_mut(repo) else {
            return;
        };
        let Some(count) = rw.watches.get_mut(&rel) else {
            return;
        };
        *count -= 1;
        if *count == 0 {
            rw.watches.remove(&rel);
            rw.watch_set.lock().unwrap().remove(&rel);
            let target = rw.target(&rel);
            let _ = rw.debouncer.unwatch(&target);
        }
        if rw.watches.is_empty() {
            repos.remove(repo);
        }
    }

    #[cfg(test)]
    fn watch_refcount(&self, repo: &str, rel: &str) -> u32 {
        let rel = norm_rel(rel);
        self.repos
            .lock()
            .unwrap()
            .get(repo)
            .and_then(|r| r.watches.get(&rel).copied())
            .unwrap_or(0)
    }

    #[cfg(test)]
    fn repo_active(&self, repo: &str) -> bool {
        self.repos.lock().unwrap().contains_key(repo)
    }

    #[cfg(test)]
    fn mode(&self, repo: &str) -> Option<WatchMode> {
        self.repos.lock().unwrap().get(repo).map(|r| r.mode)
    }
}

/// Spawn a debouncer of the given `mode` plus its pump task, wired so settled
/// batches flow debouncer-thread → mpsc → pump → broadcast. Must be called within
/// a tokio runtime (the pump is a `tokio::spawn`).
fn spawn_backend(
    mode: WatchMode,
    repo: String,
    root: PathBuf,
    watch_set: Arc<Mutex<BTreeSet<String>>>,
    gitignore: Arc<Gitignore>,
    tx: broadcast::Sender<(String, String)>,
) -> Result<AnyDebouncer> {
    let (evt_tx, evt_rx) = mpsc::unbounded_channel::<Vec<DebouncedEvent>>();
    tokio::spawn(pump(repo, root, watch_set, gitignore, evt_rx, tx));
    // The handler runs on the debouncer's own std thread; forward the batch and let
    // the pump do the tokio-side work. A closed mpsc (pump gone) just drops it.
    let handler = move |res: DebounceEventResult| {
        if let Ok(events) = res {
            let _ = evt_tx.send(events);
        }
    };
    let deb = match mode {
        WatchMode::Native => AnyDebouncer::Native(
            new_debouncer(DEBOUNCE_TIMEOUT, None, handler)
                .context("building the native debouncer")?,
        ),
        WatchMode::Poll => AnyDebouncer::Poll(
            new_debouncer_opt::<_, PollWatcher, RecommendedCache>(
                DEBOUNCE_TIMEOUT,
                None,
                handler,
                RecommendedCache::new(),
                notify::Config::default().with_poll_interval(POLL_INTERVAL),
            )
            .context("building the poll debouncer")?,
        ),
    };
    Ok(deb)
}

/// The tokio pump: map each settled batch to the distinct watched dirs it touched
/// and broadcast one nudge per dir. Ends when the debouncer (its mpsc sender) is
/// dropped — the natural teardown when a `RepoWatcher` is removed or rebuilt.
async fn pump(
    repo: String,
    root: PathBuf,
    watch_set: Arc<Mutex<BTreeSet<String>>>,
    gitignore: Arc<Gitignore>,
    mut evt_rx: mpsc::UnboundedReceiver<Vec<DebouncedEvent>>,
    tx: broadcast::Sender<(String, String)>,
) {
    while let Some(batch) = evt_rx.recv().await {
        let mut dirs: BTreeSet<String> = BTreeSet::new();
        for event in &batch {
            for path in &event.paths {
                if let Some(rel) = map_to_watched_dir(&root, path, &watch_set, &gitignore) {
                    dirs.insert(rel);
                }
            }
        }
        for rel in dirs {
            // No subscribers → Err; ignored (a nudge with no listener is a no-op).
            let _ = tx.send((repo.clone(), rel));
        }
    }
}

/// Map one event path to the watched rel dir it belongs to, or `None` to drop it.
/// Drops: paths outside the canonical root, [`HARD_EXCLUDE`] / gitignored children
/// (a `NonRecursive` root watch still fires a Modify on a child dir like
/// `node_modules`), and any parent dir not in the current watch-set.
fn map_to_watched_dir(
    root: &Path,
    path: &Path,
    watch_set: &Mutex<BTreeSet<String>>,
    gitignore: &Gitignore,
) -> Option<String> {
    let rel = path.strip_prefix(root).ok()?;
    let child = rel.file_name()?.to_str()?;
    if HARD_EXCLUDE.contains(&child) {
        return None;
    }
    // is_dir is a best-effort read: a removed path reads false, which only makes the
    // gitignore check less aggressive (a removed gitignored dir is rare and harmless).
    let is_dir = path.is_dir();
    if gitignore
        .matched_path_or_any_parents(rel, is_dir)
        .is_ignore()
    {
        return None;
    }
    let parent = rel_to_slug(rel.parent()?);
    let set = watch_set.lock().unwrap();
    set.contains(&parent).then_some(parent)
}

/// A repo-relative path as a `/`-joined slug (the watch-set / wire form), so the
/// comparison never trips on Windows `\` vs the `/` the browser sends. Empty for
/// the root.
fn rel_to_slug(rel: &Path) -> String {
    rel.components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

/// Normalize an incoming rel dir to the watch-set form: `/`-separated, no leading
/// or trailing slash; `""` stays the root. Public so the `/ws/tree` handler stores
/// a connection's watched dirs in the SAME form the nudges carry, so its
/// per-connection filter matches.
pub fn norm_rel(rel: &str) -> String {
    rel.replace('\\', "/").trim_matches('/').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// A generous window: the debounce settle plus slack for a slow CI host.
    fn window() -> Duration {
        DEBOUNCE_TIMEOUT + Duration::from_secs(2)
    }

    async fn recv_in(rx: &mut DirtyRx, dur: Duration) -> Option<(String, String)> {
        tokio::time::timeout(dur, rx.recv())
            .await
            .ok()
            .and_then(Result::ok)
    }

    #[tokio::test]
    async fn watch_root_emits_dirty_on_create() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = WatcherManager::new(MAX_WATCHES);
        let mut rx = mgr.watch("owner/repo", dir.path(), "").unwrap();

        fs::write(dir.path().join("f.txt"), b"x").unwrap();

        let got = recv_in(&mut rx, window()).await;
        assert_eq!(got, Some(("owner/repo".to_string(), String::new())));
    }

    #[tokio::test]
    async fn storm_coalesces_to_few_nudges() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = WatcherManager::new(MAX_WATCHES);
        let mut rx = mgr.watch("owner/repo", dir.path(), "").unwrap();

        for i in 0..20 {
            fs::write(dir.path().join(format!("f{i}.txt")), b"x").unwrap();
        }

        // Drain until a quiet gap; a settled storm is one dir per batch, not 20.
        let mut count = 0;
        while recv_in(&mut rx, window()).await.is_some() {
            count += 1;
        }
        assert!((1..=3).contains(&count), "coalesced count = {count}");
    }

    #[tokio::test]
    async fn unwatched_and_gitignored_child_emit_nothing() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(".gitignore"), b"ignored/\n").unwrap();
        fs::create_dir(dir.path().join("ignored")).unwrap();
        fs::create_dir(dir.path().join("node_modules")).unwrap();

        let mgr = WatcherManager::new(MAX_WATCHES);
        let mut rx = mgr.watch("owner/repo", dir.path(), "").unwrap();

        fs::write(dir.path().join("node_modules/x"), b"x").unwrap();
        fs::write(dir.path().join("ignored/y"), b"y").unwrap();
        assert!(
            recv_in(&mut rx, window()).await.is_none(),
            "noise/gitignored children must not nudge"
        );

        fs::write(dir.path().join("visible.txt"), b"v").unwrap();
        assert_eq!(
            recv_in(&mut rx, window()).await,
            Some(("owner/repo".to_string(), String::new())),
            "a real child create nudges the root"
        );
    }

    #[tokio::test]
    async fn refcount_shares_and_tears_down() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = WatcherManager::new(MAX_WATCHES);
        let _rx1 = mgr.watch("owner/repo", dir.path(), "").unwrap();
        let _rx2 = mgr.watch("owner/repo", dir.path(), "").unwrap();
        assert_eq!(
            mgr.watch_refcount("owner/repo", ""),
            2,
            "two share one watch"
        );

        mgr.unwatch("owner/repo", "");
        assert_eq!(mgr.watch_refcount("owner/repo", ""), 1);
        assert!(mgr.repo_active("owner/repo"), "one hold keeps the watcher");

        mgr.unwatch("owner/repo", "");
        assert!(
            !mgr.repo_active("owner/repo"),
            "the last release tears the repo watcher down"
        );
    }

    #[tokio::test]
    async fn watch_rejects_parent_traversal() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = WatcherManager::new(MAX_WATCHES);
        assert!(
            mgr.watch("owner/repo", dir.path(), "../escape").is_err(),
            "a `..` path must be refused, never watched out-of-root"
        );
        assert!(
            !mgr.repo_active("owner/repo"),
            "a rejected watch creates no repo watcher"
        );
    }

    #[tokio::test]
    async fn over_cap_degrades_to_poll() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("a")).unwrap();
        fs::create_dir(dir.path().join("b")).unwrap();

        let mgr = WatcherManager::new(1);
        let _rx_a = mgr.watch("owner/repo", dir.path(), "a").unwrap();
        let mut rx_b = mgr.watch("owner/repo", dir.path(), "b").unwrap();
        assert_eq!(
            mgr.mode("owner/repo"),
            Some(WatchMode::Poll),
            "a 2nd dir over max_watches=1 degrades to poll"
        );

        fs::write(dir.path().join("b/f.txt"), b"x").unwrap();
        let got = recv_in(&mut rx_b, POLL_INTERVAL + Duration::from_secs(2)).await;
        assert_eq!(
            got,
            Some(("owner/repo".to_string(), "b".to_string())),
            "poll still emits on a create"
        );
    }
}
