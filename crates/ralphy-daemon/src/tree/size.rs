//! The size of one folder: the walk behind the `tree.size` verb (ADR-0036,
//! amendment 2026-10-15). It counts what is on the disk, `.git`, `node_modules`
//! and `target` included: [`super::HARD_EXCLUDE`] is a rule of the listing, not
//! of a size. A link counts as one item and is never followed, so the walk
//! cannot leave the root. The walk stops itself at a [`SizeBudget`], because
//! nothing upstream of an Observe read caps it, and it says `truncated`. One
//! walk runs at a time ([`Slot`]): four threads for five seconds is the most
//! that a forgotten click can cost.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ignore::WalkState;
use serde::Serialize;

use super::ignored::{self, Rules};
use crate::confine;

/// Four is a cap that leaves CPU for the consoles that the same daemon serves.
/// Measured on 2026-10-15: eight threads add nothing on four CPUs (ADR-0036,
/// amendment 2026-10-15).
const THREADS: usize = 4;

/// The clock is read once per this many entries, not once per entry.
const CLOCK_EVERY: usize = 512;

/// The one slot of `tree.size`, for the whole daemon.
pub static SLOT: Slot = Slot::new();

/// Where a walk stops: after `max_entries` entries, or when `deadline` has
/// passed since it began. Either way the reply says `truncated`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SizeBudget {
    pub max_entries: usize,
    pub deadline: Duration,
}

impl Default for SizeBudget {
    /// The wire budget: 200,000 entries, five seconds. A walk for a peer must
    /// fit inside the relay's ceiling of 60 s.
    fn default() -> Self {
        Self {
            max_entries: 200_000,
            deadline: Duration::from_secs(5),
        }
    }
}

/// The answer of a walk. With `truncated` the numbers are a lower bound.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Size {
    /// The sum of the logical lengths of the files, in bytes.
    pub bytes: u64,
    pub files: u64,
    pub dirs: u64,
    /// The part of `bytes` that the repo ignores, by the rules of `tree.list`.
    pub ignored_bytes: u64,
    pub truncated: bool,
}

/// A walk that did not answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SizeError {
    /// Another walk holds the slot.
    Busy,
    /// The path is missing, is not a folder, or is outside the root.
    NotFound,
}

impl SizeError {
    /// The refusal as the wire spells it.
    pub fn reason(self) -> &'static str {
        match self {
            SizeError::Busy => "busy",
            SizeError::NotFound => "not found",
        }
    }
}

/// A place for one walk at a time.
pub struct Slot(AtomicBool);

/// A taken slot, given back when it is dropped, also on a panic.
struct Held<'a>(&'a AtomicBool);

impl Slot {
    pub const fn new() -> Self {
        Self(AtomicBool::new(false))
    }

    fn take(&self) -> Option<Held<'_>> {
        self.0
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .ok()
            .map(|_| Held(&self.0))
    }
}

impl Default for Slot {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for Held<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

/// The size of the confined folder `rel` under `root`, the checkout root the
/// ignore rules are anchored to. `rel` empty is the root itself.
pub fn size(root: &Path, rel: &str, budget: SizeBudget, slot: &Slot) -> Result<Size, SizeError> {
    let _held = slot.take().ok_or(SizeError::Busy)?;
    let base = confine::confine(root, "").map_err(|_| SizeError::NotFound)?;
    let dir = confine::confine(root, rel).map_err(|_| SizeError::NotFound)?;
    if !dir.is_dir() {
        return Err(SizeError::NotFound);
    }
    let parts = ignored::components_below(&base, &dir);
    let rules = Rules::for_dir(&base, &parts);
    Ok(Walk::new(&base, budget).run(&dir, rules))
}

/// The state that the workers of one walk share.
struct Walk<'a> {
    base: &'a Path,
    budget: SizeBudget,
    started: Instant,
    seen: AtomicUsize,
    truncated: AtomicBool,
    bytes: AtomicU64,
    files: AtomicU64,
    dirs: AtomicU64,
    ignored_bytes: AtomicU64,
    /// The rules in force inside each folder met so far. A folder is visited
    /// before its entries, so the rules of a parent are always here.
    scopes: Mutex<HashMap<PathBuf, Arc<Rules>>>,
}

impl<'a> Walk<'a> {
    fn new(base: &'a Path, budget: SizeBudget) -> Self {
        Self {
            base,
            budget,
            started: Instant::now(),
            seen: AtomicUsize::new(0),
            truncated: AtomicBool::new(false),
            bytes: AtomicU64::new(0),
            files: AtomicU64::new(0),
            dirs: AtomicU64::new(0),
            ignored_bytes: AtomicU64::new(0),
            scopes: Mutex::new(HashMap::new()),
        }
    }

    fn run(self, dir: &Path, rules: Rules) -> Size {
        self.remember(dir, Arc::new(rules));
        let mut builder = ignore::WalkBuilder::new(dir);
        builder
            .standard_filters(false)
            .hidden(false)
            .follow_links(false)
            .threads(THREADS);
        let walk = &self;
        builder.build_parallel().run(|| {
            Box::new(|found| match found {
                Ok(entry) => walk.visit(&entry),
                // An entry that cannot be read counts as nothing: the size is
                // what the daemon could read.
                Err(_) => WalkState::Continue,
            })
        });
        Size {
            bytes: self.bytes.into_inner(),
            files: self.files.into_inner(),
            dirs: self.dirs.into_inner(),
            ignored_bytes: self.ignored_bytes.into_inner(),
            truncated: self.truncated.into_inner(),
        }
    }

    fn visit(&self, entry: &ignore::DirEntry) -> WalkState {
        // The root of the walk is not counted; its rules are already known.
        if entry.depth() == 0 {
            return WalkState::Continue;
        }
        if self.over_budget() {
            self.truncated.store(true, Ordering::Relaxed);
            return WalkState::Quit;
        }
        let parent = self.scope_of(entry.path().parent());
        let is_dir = entry.file_type().is_some_and(|t| t.is_dir());
        let ignored = self.is_ignored(&parent, entry, is_dir);
        if is_dir {
            self.dirs.fetch_add(1, Ordering::Relaxed);
            self.remember(
                entry.path(),
                Arc::new(self.scope_below(&parent, entry, ignored)),
            );
        } else {
            self.count_file(entry, ignored);
        }
        WalkState::Continue
    }

    /// `true` when the entry limit is reached, or the clock says that the
    /// deadline has passed (the clock is read every [`CLOCK_EVERY`] entries).
    fn over_budget(&self) -> bool {
        let seen = self.seen.fetch_add(1, Ordering::Relaxed);
        seen >= self.budget.max_entries
            || (seen.is_multiple_of(CLOCK_EVERY) && self.started.elapsed() >= self.budget.deadline)
    }

    fn scope_of(&self, dir: Option<&Path>) -> Arc<Rules> {
        let scopes = self
            .scopes
            .lock()
            .expect("no worker panics while it holds the scopes");
        dir.and_then(|d| scopes.get(d))
            .cloned()
            // Every folder is recorded before its entries are met, so a
            // parent is always here. If one were not, nothing is called ignored.
            .unwrap_or_else(|| Arc::new(Rules::Layers(Vec::new())))
    }

    fn remember(&self, dir: &Path, rules: Arc<Rules>) {
        self.scopes
            .lock()
            .expect("no worker panics while it holds the scopes")
            .insert(dir.to_path_buf(), rules);
    }

    /// An entry inside an ignored folder is ignored without a match: git never
    /// looks inside one.
    fn is_ignored(&self, parent: &Rules, entry: &ignore::DirEntry, is_dir: bool) -> bool {
        match parent {
            Rules::AllIgnored => true,
            Rules::Layers(_) => parent.ignored(&self.rel_of(entry), is_dir),
        }
    }

    /// The rules inside the folder `entry`: none to read when the folder is
    /// ignored itself.
    fn scope_below(&self, parent: &Rules, entry: &ignore::DirEntry, ignored: bool) -> Rules {
        if ignored {
            Rules::AllIgnored
        } else {
            parent.descend(&self.rel_of(entry), entry.path())
        }
    }

    /// The `/`-joined path of the entry below the checkout root, as the rules
    /// take it.
    fn rel_of(&self, entry: &ignore::DirEntry) -> String {
        ignored::components_below(self.base, entry.path()).join("/")
    }

    fn count_file(&self, entry: &ignore::DirEntry, ignored: bool) {
        self.files.fetch_add(1, Ordering::Relaxed);
        // A file that vanished while the walk ran has no length to add.
        let len = entry.metadata().map_or(0, |m| m.len());
        self.bytes.fetch_add(len, Ordering::Relaxed);
        if ignored {
            self.ignored_bytes.fetch_add(len, Ordering::Relaxed);
        }
    }
}

#[cfg(test)]
mod tests;
