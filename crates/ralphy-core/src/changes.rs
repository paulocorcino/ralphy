//! The working-tree change set: one reader over `git status --porcelain=v2 -z`
//! that answers "what differs from HEAD" for a repo. This is the single
//! definition of a dirty tree — `git::is_clean_ignoring_ralphy` is expressed on
//! top of it.

use std::path::Path;

use anyhow::{Context, Result};
use serde::Serialize;

/// What happened to a path, as the change set reports it. Used three ways on one
/// entry: the derived `status`, which answers "what differs from HEAD" (the first
/// non-`.` side, so a path staged as added and then deleted from disk still
/// reports `Added` — the index side wins), plus the index and worktree sides
/// separately.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeStatus {
    Modified,
    Added,
    Deleted,
    Renamed,
    Untracked,
    Conflicted,
}

/// One changed path. `original_path` is set only for a rename/copy.
///
/// `index_status` and `worktree_status` are the two sides of git's `XY` field,
/// `None` meaning unmodified on that side (git's `.`). `status` is the derived
/// projection over both — see [`ChangeStatus`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Change {
    pub path: String,
    pub original_path: Option<String>,
    pub status: ChangeStatus,
    pub index_status: Option<ChangeStatus>,
    pub worktree_status: Option<ChangeStatus>,
}

/// The repo's working-tree change set, in git's own emission order. Entries
/// under the gitignored run directory (`.ralphy/`) are never reported.
///
/// `repo` must be the git TOPLEVEL: git reports the whole repo's change set from
/// any directory inside it, while the run-artifact filter anchors at the root, so
/// a subdirectory path would yield root-relative entries under a mismatched
/// filter. Callers resolve with [`crate::git::resolve_toplevel`].
pub fn changes(repo: &Path) -> Result<Vec<Change>> {
    let out = crate::git::git(repo, &["status", "--porcelain=v2", "-z"])
        .with_context(|| format!("reading the change set of {}", repo.display()))?;
    let records: Vec<&str> = out.split('\0').filter(|t| !t.is_empty()).collect();

    let mut list = Vec::new();
    // Explicit index: a `2 ` (rename/copy) record consumes the NEXT NUL-framed
    // token as its original path.
    let mut i = 0;
    while i < records.len() {
        let record = records[i];
        i += 1;
        let entry = match record.as_bytes().first() {
            Some(b'?') => field(record, 2).map(|path| Change {
                path,
                original_path: None,
                status: ChangeStatus::Untracked,
                index_status: None,
                worktree_status: Some(ChangeStatus::Untracked),
            }),
            Some(b'1') => field(record, 9).map(|path| {
                let (index_status, worktree_status) = sides(record);
                Change {
                    path,
                    original_path: None,
                    status: ordinary_status(record),
                    index_status,
                    worktree_status,
                }
            }),
            Some(b'2') => {
                let original = records.get(i).map(ToString::to_string);
                i += 1;
                field(record, 10).map(|path| {
                    let (index_status, worktree_status) = sides(record);
                    Change {
                        path,
                        original_path: original,
                        status: ChangeStatus::Renamed,
                        index_status,
                        worktree_status,
                    }
                })
            }
            // An unresolved conflict is worktree work whatever its XY reads: a
            // per-char split would file `AA` under the index side and claim a
            // commit would contain it.
            Some(b'u') => field(record, 11).map(|path| Change {
                path,
                original_path: None,
                status: ChangeStatus::Conflicted,
                index_status: None,
                worktree_status: Some(ChangeStatus::Conflicted),
            }),
            // `!` (ignored) and `#` (headers) carry no change.
            _ => None,
        };
        if let Some(entry) = entry {
            // The `.ralphy/` rule is shared with the daemon's dirty bit.
            if ralphy_git_read::is_run_artifact(&entry.path)
                || entry
                    .original_path
                    .as_deref()
                    .is_some_and(ralphy_git_read::is_run_artifact)
            {
                continue;
            }
            list.push(entry);
        }
    }
    Ok(list)
}

/// The path field of a porcelain-v2 record: everything after the first
/// `count - 1` space-separated fields. Only the path itself may contain a
/// space, so a bounded split is exact.
fn field(record: &str, count: usize) -> Option<String> {
    record
        .splitn(count, ' ')
        .nth(count - 1)
        .filter(|p| !p.is_empty())
        .map(str::to_owned)
}

/// The two-char `XY` status field of a `1`/`2` record.
fn xy(record: &str) -> &str {
    record.split(' ').nth(1).unwrap_or("")
}

/// One side of `XY`: `.` is unmodified on that side, and every other char falls
/// back to `Modified` (`M`, `T`, and anything a future git adds).
fn side_status(c: char) -> Option<ChangeStatus> {
    Some(match c {
        '.' => return None,
        'A' => ChangeStatus::Added,
        'D' => ChangeStatus::Deleted,
        'R' | 'C' => ChangeStatus::Renamed,
        'U' => ChangeStatus::Conflicted,
        _ => ChangeStatus::Modified,
    })
}

/// Both sides of a `1`/`2` record, index first. A short or absent `XY` reads as
/// no change on the missing side rather than a guess.
fn sides(record: &str) -> (Option<ChangeStatus>, Option<ChangeStatus>) {
    let mut chars = xy(record).chars();
    let index = chars.next().and_then(side_status);
    let worktree = chars.next().and_then(side_status);
    (index, worktree)
}

/// Status of an ordinary (`1 `) record: the first non-`.` char of its two-char
/// `XY` field. The surface answers "what differs from HEAD", so a staged add
/// that was then edited is one `Added`, not two states. Expressed over
/// [`side_status`] so the projection and the split can never drift apart.
fn ordinary_status(record: &str) -> ChangeStatus {
    xy(record)
        .chars()
        .find(|c| *c != '.')
        .and_then(side_status)
        .unwrap_or(ChangeStatus::Modified)
}

#[cfg(test)]
mod tests;
