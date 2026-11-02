//! The live file-tree watcher (ADR-0036 §4, issue #196): the daemon-side half of
//! the workbench's live tree. A [`WatcherManager`] owns one debounced OS watcher
//! per repo, watching only the directories a browser has actually expanded (a
//! refcounted, lazy watch-set bounded by the screen, not the repo). Each settled
//! event storm is mapped to the watched directory it touched and broadcast as one
//! `tree.dirty` nudge; a tree channel of the page socket
//! (`routes::ws_tree::TreeConn`) turns a nudge into a client push, and the
//! browser re-reads the changed subtree via the existing Observe
//! [`crate::tree::list`] path.
//!
//! Concurrency bridge (mirrors `session.rs`): the notify/debouncer thread forwards
//! raw `Vec<DebouncedEvent>` batches over an `mpsc::unbounded_channel`; a tokio
//! PUMP task maps each event path's parent to a watched rel dir, drops
//! `crate::tree::HARD_EXCLUDE` noise, dedups per batch, and
//! `broadcast::send`s one nudge per distinct dir. The async stack stays confined
//! to this crate.
//!
//! When the watched-dir count crosses `max_watches`, a repo's debouncer is rebuilt
//! on a [`notify::PollWatcher`] (degrade, never fail); the broadcast `Sender` is
//! preserved across the rebuild, so existing subscribers keep receiving.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use anyhow::{Context, Result};
use notify::event::ModifyKind;
use notify::{EventKind, PollWatcher, RecursiveMode};
use notify_debouncer_full::{
    new_debouncer, new_debouncer_opt, DebounceEventResult, DebouncedEvent, Debouncer,
    RecommendedCache,
};
use tokio::sync::{broadcast, mpsc};

use crate::tree::HARD_EXCLUDE;

/// Default cap on the total watched-dir count before a repo degrades to polling.
/// Bounded by the screen (the expanded set), so the native path comfortably fits.
pub const MAX_WATCHES: usize = 512;

/// The run-snapshot directory (ADR-0047 §9), named by the `runs.watch` /
/// `runs.unwatch` subscription. It once needed an exemption from the pump's
/// gitignore filter (`.ralphy/` is gitignored in every repo ralphy touches);
/// that filter is gone (ADR-0036, amendment 2026-07-26) and this is now an
/// ordinary watch target.
pub const RUNSTATE_REL: &str = ".ralphy/runstate";

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
    tx: broadcast::Sender<(String, String)>,
    mode: WatchMode,
}

impl RepoWatcher {
    fn new(repo: &str, canon_root: PathBuf) -> Result<Self> {
        let (tx, _rx0) = broadcast::channel(BROADCAST_CAP);
        let watch_set = Arc::new(Mutex::new(BTreeSet::new()));
        let debouncer = spawn_backend(
            WatchMode::Native,
            repo.to_string(),
            canon_root.clone(),
            watch_set.clone(),
            tx.clone(),
        )?;
        Ok(Self {
            repo: repo.to_string(),
            root: canon_root,
            debouncer,
            watches: BTreeMap::new(),
            watch_set,
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
/// `Arc` at the router; every tree connection watches/unwatches through it.
pub struct WatcherManager {
    repos: Mutex<BTreeMap<String, RepoWatcher>>,
    max_watches: usize,
    // notify's `PollWatcher::watch` never returns an error (notify 8.2
    // `poll.rs`), so a test makes the next degrade fail by hand.
    #[cfg(test)]
    fail_next_degrade: std::sync::atomic::AtomicBool,
}

impl WatcherManager {
    pub fn new(max_watches: usize) -> Self {
        Self {
            repos: Mutex::new(BTreeMap::new()),
            max_watches: max_watches.max(1),
            #[cfg(test)]
            fail_next_degrade: std::sync::atomic::AtomicBool::new(false),
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
        // `root.join(rel)` must stay inside the root, or the OS watch reports
        // changes outside it (ADR-0036 §5). A `..` is refused by its text first, so
        // a missing path that climbs out is not read as a plain miss; then the path
        // is resolved, so a folder that links out of the root is refused too.
        if rel.split('/').any(|c| c == "..") {
            anyhow::bail!("watch path escapes the repo root: {rel}");
        }
        crate::confine::confine(root, &rel)
            .with_context(|| format!("watch path is not inside the repo root: {rel}"))?;
        let canon_root = std::fs::canonicalize(root)
            .with_context(|| format!("canonicalizing watch root {}", root.display()))?;
        let mut repos = lock(&self.repos);
        if !repos.contains_key(repo) {
            repos.insert(repo.to_string(), RepoWatcher::new(repo, canon_root)?);
        }
        // Every failure below leaves the registry as it was: no raised count,
        // and no empty repo watcher.
        let rx = hold(&mut repos, repo, &rel)?;
        self.degrade_past_cap(&mut repos, repo, &rel)?;
        Ok(rx)
    }

    /// Past `max_watches` watched dirs in total, move `repo` to polling. A
    /// failed move releases the hold `watch` just took on `rel`.
    fn degrade_past_cap(
        &self,
        repos: &mut BTreeMap<String, RepoWatcher>,
        repo: &str,
        rel: &str,
    ) -> Result<()> {
        let total: usize = repos.values().map(|r| r.watches.len()).sum();
        if total <= self.max_watches {
            return Ok(());
        }
        #[cfg(test)]
        let degraded = if self
            .fail_next_degrade
            .swap(false, std::sync::atomic::Ordering::SeqCst)
        {
            Err(anyhow::anyhow!("an injected degrade failure"))
        } else {
            repos.get_mut(repo).expect("present").degrade_to_poll()
        };
        #[cfg(not(test))]
        let degraded = repos.get_mut(repo).expect("present").degrade_to_poll();
        if let Err(e) = degraded {
            release(repos, repo, rel);
            return Err(e).context("degrading to polling");
        }
        Ok(())
    }

    /// Release one subscriber's hold on `repo`'s `rel` dir. On the last release the
    /// underlying OS watch is dropped; when a repo has no watched dirs left its
    /// whole watcher (debouncer + pump) tears down. Unknown repo/dir is a no-op.
    pub fn unwatch(&self, repo: &str, rel: &str) {
        let rel = norm_rel(rel);
        release(&mut lock(&self.repos), repo, &rel);
    }

    #[cfg(test)]
    pub(crate) fn watch_refcount(&self, repo: &str, rel: &str) -> u32 {
        let rel = norm_rel(rel);
        lock(&self.repos)
            .get(repo)
            .and_then(|r| r.watches.get(&rel).copied())
            .unwrap_or(0)
    }

    #[cfg(test)]
    pub(crate) fn repo_active(&self, repo: &str) -> bool {
        lock(&self.repos).contains_key(repo)
    }

    #[cfg(test)]
    fn mode(&self, repo: &str) -> Option<WatchMode> {
        lock(&self.repos).get(repo).map(|r| r.mode)
    }
}

/// Take one hold on `repo`'s normalized `rel` dir, under the caller's lock,
/// and subscribe to the repo's nudges. The first hold on a dir adds its OS
/// watch; when that fails, a repo watcher left with no dir goes.
fn hold(repos: &mut BTreeMap<String, RepoWatcher>, repo: &str, rel: &str) -> Result<DirtyRx> {
    let rw = repos.get_mut(repo).expect("just inserted");
    if rw.watches.get(rel).copied().unwrap_or(0) == 0 {
        let target = rw.target(rel);
        if let Err(e) = rw.debouncer.watch(&target, RecursiveMode::NonRecursive) {
            if rw.watches.is_empty() {
                repos.remove(repo);
            }
            return Err(e).with_context(|| format!("watching {}", target.display()));
        }
        lock(&rw.watch_set).insert(rel.to_string());
    }
    *rw.watches.entry(rel.to_string()).or_insert(0) += 1;
    Ok(rw.tx.subscribe())
}

/// Lock `mutex` even after a thread panicked while it held it. Each change under
/// these locks is one map or set call, so the data stays whole, and refusing
/// every later lock would stop the tree nudges of every repo.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Drop one hold on `repo`'s normalized `rel` dir, under the caller's lock (see
/// [`WatcherManager::unwatch`]).
fn release(repos: &mut BTreeMap<String, RepoWatcher>, repo: &str, rel: &str) {
    let Some(rw) = repos.get_mut(repo) else {
        return;
    };
    let Some(count) = rw.watches.get_mut(rel) else {
        return;
    };
    *count -= 1;
    if *count == 0 {
        rw.watches.remove(rel);
        lock(&rw.watch_set).remove(rel);
        let target = rw.target(rel);
        // A dir deleted while watched has already lost its OS watch, so this
        // failure is common and changes nothing: it is logged, not returned.
        if let Err(e) = rw.debouncer.unwatch(&target) {
            tracing::debug!(
                repo = %repo, dir = %rel, error = %e,
                "unwatch of a released dir failed"
            );
        }
    }
    if rw.watches.is_empty() {
        repos.remove(repo);
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
    tx: broadcast::Sender<(String, String)>,
) -> Result<AnyDebouncer> {
    let (evt_tx, evt_rx) = mpsc::unbounded_channel::<Vec<DebouncedEvent>>();
    tokio::spawn(pump(repo, root, watch_set, evt_rx, tx));
    // The handler runs on the debouncer's own std thread; forward the batch and let
    // the pump do the tokio-side work. A closed mpsc (pump gone) just drops it.
    let handler = move |res: DebounceEventResult| {
        if let Ok(events) = res {
            #[expect(
                clippy::let_underscore_must_use,
                reason = "a closed channel means the pump is gone, and the batch has no reader"
            )]
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
    mut evt_rx: mpsc::UnboundedReceiver<Vec<DebouncedEvent>>,
    tx: broadcast::Sender<(String, String)>,
) {
    while let Some(batch) = evt_rx.recv().await {
        for rel in touched_dirs(&batch, &root, &watch_set) {
            // No subscribers → Err; ignored (a nudge with no listener is a no-op).
            #[expect(
                clippy::let_underscore_must_use,
                reason = "a broadcast with no subscriber is an error, and a nudge to no one is fine"
            )]
            let _ = tx.send((repo.clone(), rel));
        }
    }
}

/// The distinct watched dirs one settled batch touched, in sorted order.
/// Sync on purpose: the watch-set lock is never held across an `.await`.
fn touched_dirs(
    batch: &[DebouncedEvent],
    root: &Path,
    watch_set: &Mutex<BTreeSet<String>>,
) -> BTreeSet<String> {
    let mut dirs: BTreeSet<String> = BTreeSet::new();
    for event in batch {
        if changes_no_listing(event) {
            continue;
        }
        for path in &event.paths {
            if let Some(rel) = map_to_watched_dir(root, path, watch_set) {
                dirs.insert(rel);
            }
        }
    }
    dirs
}

/// Whether an event can change no listing, so it nudges nothing.
fn changes_no_listing(event: &notify::Event) -> bool {
    // A READ is not a change. Linux inotify reports `IN_OPEN` /
    // `IN_CLOSE_NOWRITE` on a watched dir, and `notify`'s inotify backend
    // subscribes to them, so every `tree.list` (which opens the dir and
    // each child dir) arrived here as an `Access` event and became a
    // nudge — and the browser answers a nudge with another `tree.list`.
    // On a Linux daemon that was a self-sustaining loop (~25 nudges/s on
    // an idle repo). Windows never emits these, which is why it hid.
    if matches!(event.kind, EventKind::Access(_)) {
        return true;
    }
    // A directory's OWN modify — its mtime moved because a child was
    // written, which Windows and FSEvents both bubble to the parent's
    // watch — changes no listing: a listing is names and the dir
    // flag, and the names under the watched dir are what they were.
    // A rename (`Modify(Name)`) does change one and stays. Measured
    // on the macOS runner (2026-09-17): `unwatched/y` nudged the root
    // through `unwatched` itself.
    matches!(
        event.kind,
        EventKind::Modify(ModifyKind::Any | ModifyKind::Data(_) | ModifyKind::Metadata(_))
    ) && !event.paths.is_empty()
        && event.paths.iter().all(|p| p.is_dir())
}

/// Map one event path to the watched rel dir it belongs to, or `None` to drop it.
/// Drops: paths outside the canonical root, [`HARD_EXCLUDE`] children (a
/// `NonRecursive` root watch still fires a Modify on a child dir like
/// `node_modules`), and any parent dir not in the current watch-set. Gitignored
/// paths nudge like any other (ADR-0036, amendment 2026-07-26) — the tree lists
/// them, so it must also refresh them.
fn map_to_watched_dir(
    root: &Path,
    path: &Path,
    watch_set: &Mutex<BTreeSet<String>>,
) -> Option<String> {
    let rel = path.strip_prefix(root).ok()?;
    let child = rel.file_name()?.to_str()?;
    let parent = rel_to_slug(rel.parent()?);
    if HARD_EXCLUDE.contains(&child) {
        return None;
    }
    // A HEAD rel nudges on its `HEAD` child alone. `git status` rewrites the
    // `index` beside it, and the nudge is answered with a `git status`, so any
    // other child would loop.
    if is_head_rel(&parent) && child != "HEAD" {
        return None;
    }
    let set = lock(watch_set);
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
/// or trailing slash; `""` stays the root. Public so a tree connection stores
/// a connection's watched dirs in the SAME form the nudges carry, so its
/// per-connection filter matches.
pub fn norm_rel(rel: &str) -> String {
    rel.replace('\\', "/").trim_matches('/').to_string()
}

/// Whether `rel` is a dir the HEAD watch holds: a gitdir in the standard
/// layout (`.git`, `.git/worktrees/<name>`) or its `logs/`. Lexical only.
pub fn is_head_rel(rel: &str) -> bool {
    let gitdir = rel.strip_suffix("/logs").unwrap_or(rel);
    if gitdir == ".git" {
        return true;
    }
    gitdir
        .strip_prefix(".git/worktrees/")
        .is_some_and(|name| !name.is_empty() && !name.contains('/'))
}

#[cfg(test)]
mod tests;
