//! The desk history (ADR-0050 amendment 2026-10-04, desk history): the last
//! [`HISTORY_MAX`] versions of the desk layout, one JSON file each in a
//! `desk-history/` directory beside `desk.toml`.
//!
//! The rules that decide what is written ([`capture_step`], [`same_layout`])
//! and how a restore builds the new desk ([`restore`]) are pure and take the
//! clock as an argument. The file functions are path-explicit, like
//! [`super::load_from`].

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use super::{DeskRecord, DeskStore};

/// How many versions are kept.
pub const HISTORY_MAX: usize = 50;

/// A change less than this many ms after the FIRST change of the newest
/// version goes into that version, so a version holds at most one minute of
/// changes: one drag is one version. A restore's version takes them too: a
/// page that reloads after a restore names its consoles and writes at once.
/// A `before-restore` version never does: it is the way back.
pub const COALESCE_MS: i64 = 60_000;

/// The `kind` of a version file. An uploaded file without it is not a
/// version.
pub const VERSION_KIND: &str = "ralphy-desk-version";

/// Why a version was written.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Reason {
    /// The operator changed the layout.
    Change,
    /// The desk as it was just before a restore, so the restore can be undone.
    BeforeRestore,
    /// The desk a restore of a saved version produced.
    Restore,
    /// The desk a restore of an uploaded file produced.
    Upload,
}

/// One version: the file on disk, and the file a download hands out.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Version {
    pub kind: String,
    /// The epoch ms of the version's first change, and its file name.
    pub id: i64,
    /// When the version's first change was written.
    pub started_at: i64,
    /// When the version was last written.
    pub saved_at: i64,
    pub reason: Reason,
    pub desk: DeskStore,
}

/// One row of the history list: a version without its desk.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionInfo {
    pub id: i64,
    pub started_at: i64,
    pub saved_at: i64,
    pub reason: Reason,
    pub windows: usize,
    pub fences: usize,
    pub notes: usize,
}

impl From<&Version> for VersionInfo {
    fn from(v: &Version) -> Self {
        VersionInfo {
            id: v.id,
            started_at: v.started_at,
            saved_at: v.saved_at,
            reason: v.reason,
            windows: v.desk.windows.len(),
            fences: v.desk.fences.len(),
            notes: v.desk.notes.len(),
        }
    }
}

/// The desk with every field that changes without a layout change cleared:
/// `ts` and `sessionId` (each reconnect rewrites both), the generation and
/// the `rev`, and the sub-pixel part of each rect.
fn layout_of(desk: &DeskStore) -> DeskStore {
    let mut d = desk.clone();
    d.generation = 0;
    d.rev = 0;
    let round = |r: &mut super::DeskRect| {
        r.left = r.left.round();
        r.top = r.top.round();
        r.width = r.width.round();
        r.height = r.height.round();
    };
    for w in &mut d.windows {
        w.ts = 0;
        w.session_id = None;
        round(&mut w.rect);
    }
    for f in &mut d.fences {
        f.ts = 0;
        round(&mut f.rect);
    }
    for n in &mut d.notes {
        n.ts = 0;
        round(&mut n.rect);
    }
    d
}

/// Whether two desks show the same layout.
pub fn same_layout(a: &DeskStore, b: &DeskStore) -> bool {
    layout_of(a) == layout_of(b)
}

fn is_empty(desk: &DeskStore) -> bool {
    desk.windows.is_empty() && desk.fences.is_empty() && desk.notes.is_empty()
}

/// What a desk write does to the history.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// The newest version already shows this layout.
    Nothing,
    /// The change goes into the newest version.
    Overwrite,
    /// A new version starts. With `keep_before`, the desk as it was before
    /// the change is written first: no version holds it yet.
    Append { keep_before: bool },
}

/// Decide what a desk write from `before` to `after` at `now` does, given the
/// newest version.
pub fn capture_step(
    newest: Option<&Version>,
    before: &DeskStore,
    after: &DeskStore,
    now: i64,
) -> Step {
    if newest.is_some_and(|v| same_layout(&v.desk, after)) {
        return Step::Nothing;
    }
    if newest.is_some_and(|v| v.reason != Reason::BeforeRestore && now - v.started_at < COALESCE_MS)
    {
        return Step::Overwrite;
    }
    let held = newest.is_some_and(|v| same_layout(&v.desk, before));
    Step::Append {
        keep_before: !held && !is_empty(before) && !same_layout(before, after),
    }
}

fn same_session(a: &DeskRecord, b: &DeskRecord) -> bool {
    a.session_id.is_some()
        && a.session_id == b.session_id
        && a.daemon_id == b.daemon_id
        && a.repo == b.repo
        && a.agent == b.agent
        && a.kind == b.kind
}

/// Build the desk a restore of `version` produces over `current`, at `now`.
/// Returns the desk and the ids of the current windows kept because they run,
/// which the caller pins against the cap.
///
/// - A window of the version takes its saved place. When a current record is
///   the same console (same id, or the same live session), the version's
///   layout goes onto that record and the record keeps its id and session, so
///   a running console is never shown twice.
/// - A current window that is not in the version stays when it has a
///   session, and is dropped when it is a placeholder.
/// - Fences and note cards are the version's. The checkouts are the current
///   ones: a selection, not layout.
///
/// Every restored record gets `ts = now`.
pub fn restore(current: DeskStore, version: DeskStore, now: i64) -> (DeskStore, HashSet<String>) {
    let mut left: Vec<Option<DeskRecord>> = current.windows.into_iter().map(Some).collect();
    let mut windows = Vec::with_capacity(version.windows.len() + left.len());
    for mut v in version.windows {
        // An uploaded file is held to the same name cap as a PUT.
        super::cap_console_name(&mut v);
        let found = left
            .iter()
            .position(|c| c.as_ref().is_some_and(|c| c.id == v.id))
            .or_else(|| {
                left.iter()
                    .position(|c| c.as_ref().is_some_and(|c| same_session(c, &v)))
            });
        let record = match found.and_then(|i| left[i].take()) {
            Some(c) => DeskRecord {
                id: c.id,
                session_id: c.session_id,
                daemon_id: c.daemon_id,
                environment: c.environment,
                ts: now,
                ..v
            },
            None => DeskRecord { ts: now, ..v },
        };
        windows.push(record);
    }
    let mut running = HashSet::new();
    for c in left.into_iter().flatten() {
        if c.session_id.is_some() {
            running.insert(c.id.clone());
            windows.push(c);
        }
    }
    let mut fences = version.fences;
    fences.iter_mut().for_each(|f| f.ts = now);
    let mut notes = version.notes;
    notes.iter_mut().for_each(|n| n.ts = now);
    let desk = DeskStore {
        rev: current.rev,
        generation: current.generation,
        windows,
        fences,
        notes,
        checkouts: current.checkouts,
    };
    (desk, running)
}

fn file_of(dir: &Path, id: i64) -> PathBuf {
    dir.join(format!("{id}.json"))
}

/// The version ids in `dir`, oldest first. A missing directory has none.
fn ids(dir: &Path) -> Result<Vec<i64>> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e).with_context(|| format!("listing {}", dir.display())),
    };
    let mut ids = Vec::new();
    for entry in entries {
        let entry = entry.with_context(|| format!("listing {}", dir.display()))?;
        let name = entry.file_name();
        let id = name
            .to_str()
            .and_then(|n| n.strip_suffix(".json"))
            .and_then(|n| n.parse::<i64>().ok());
        if let Some(id) = id {
            ids.push(id);
        }
    }
    ids.sort_unstable();
    Ok(ids)
}

/// Read one version. A missing file is an error whose root is
/// `std::io::ErrorKind::NotFound`, which [`is_not_found`] tells apart.
pub fn load(dir: &Path, id: i64) -> Result<Version> {
    let path = file_of(dir, id);
    let text = std::fs::read_to_string(&path)
        .with_context(|| format!("reading desk version {}", path.display()))?;
    serde_json::from_str(&text).with_context(|| format!("parsing desk version {}", path.display()))
}

/// Whether a [`load`] error is a version that does not exist.
pub fn is_not_found(e: &anyhow::Error) -> bool {
    e.chain().any(|c| {
        c.downcast_ref::<std::io::Error>()
            .is_some_and(|io| io.kind() == std::io::ErrorKind::NotFound)
    })
}

/// The newest version that can be read. One that cannot is skipped with a
/// warning: a damaged file must not stop the history.
fn newest(dir: &Path) -> Result<Option<Version>> {
    for id in ids(dir)?.into_iter().rev() {
        match load(dir, id) {
            Ok(v) => return Ok(Some(v)),
            Err(e) => tracing::warn!(error = %format!("{e:#}"), "desk version skipped"),
        }
    }
    Ok(None)
}

fn write(dir: &Path, version: &Version) -> Result<()> {
    crate::owner_only::create_owner_only_dir(dir)?;
    let text = serde_json::to_vec_pretty(version).context("serializing desk version")?;
    crate::owner_only::write_owner_only(&file_of(dir, version.id), &text)
}

/// Delete every version but the [`HISTORY_MAX`] newest.
fn prune(dir: &Path) -> Result<()> {
    let ids = ids(dir)?;
    let extra = ids.len().saturating_sub(HISTORY_MAX);
    for id in &ids[..extra] {
        let path = file_of(dir, *id);
        std::fs::remove_file(&path).with_context(|| format!("deleting {}", path.display()))?;
    }
    Ok(())
}

/// Write `desk` as a new version, then prune. Its id is `now`, or one more
/// than the newest id when that is not older, so two versions written in the
/// same ms keep their order. Returns the id.
pub fn append(dir: &Path, desk: &DeskStore, reason: Reason, now: i64) -> Result<i64> {
    let id = ids(dir)?.last().map_or(now, |&last| now.max(last + 1));
    let mut desk = desk.clone();
    desk.generation = 0;
    desk.rev = 0;
    write(
        dir,
        &Version {
            kind: VERSION_KIND.to_string(),
            id,
            started_at: now,
            saved_at: now,
            reason,
            desk,
        },
    )?;
    prune(dir)?;
    Ok(id)
}

/// Record a desk write from `before` to `after` at `now`, by [`capture_step`].
pub fn capture(dir: &Path, before: &DeskStore, after: &DeskStore, now: i64) -> Result<()> {
    let newest = newest(dir)?;
    match capture_step(newest.as_ref(), before, after, now) {
        Step::Nothing => Ok(()),
        Step::Overwrite => {
            let mut v = newest.context("an overwrite needs a newest version")?;
            v.desk = after.clone();
            v.desk.generation = 0;
            v.desk.rev = 0;
            v.saved_at = now;
            write(dir, &v)
        }
        Step::Append { keep_before } => {
            if keep_before {
                append(dir, before, Reason::Change, now)?;
            }
            append(dir, after, Reason::Change, now).map(|_| ())
        }
    }
}

/// Every version that can be read, newest first. One that cannot is skipped
/// with a warning.
pub fn list(dir: &Path) -> Result<Vec<VersionInfo>> {
    let mut out = Vec::new();
    for id in ids(dir)?.into_iter().rev() {
        match load(dir, id) {
            Ok(v) => out.push(VersionInfo::from(&v)),
            Err(e) => tracing::warn!(error = %format!("{e:#}"), "desk version skipped"),
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests;
