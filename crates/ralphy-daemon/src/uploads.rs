//! The file upload writer (ADR-0036 amendment 2026-10-19): one file from the
//! operator's computer arrives in parts of at most [`PART_MAX`] bytes. Each part
//! is appended to `.ralphy/uploads/<id>.part`; the part that ends the file moves
//! it to its target with one rename.
//!
//! The `.part` file lives under `.ralphy/`, which the tree, the watcher and git
//! never show, so a half-written file is never seen. Its path is fixed by the
//! verb from an `id` of 32 hex digits, so the client names no path inside
//! `.ralphy/`; the TARGET is checked by [`fswrite::confine_outside_protected`]
//! like every other Write.

use std::fs::OpenOptions;
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use crate::confine;
use crate::fswrite::{self, ensure_dir, map_confine, WriteError};

/// The landing directory of the parts.
pub const DIR: &str = ".ralphy/uploads";

/// The largest part, in bytes. Its base64 fits under `MAX_COMMAND_BYTES`.
pub const PART_MAX: usize = 1 << 20;

/// `files.upload_max_mb` when the settings name none.
const DEFAULT_MAX_MB: u64 = 25;

/// A larger `files.upload_max_mb` is read as this, with a warning.
const CEILING_MB: u64 = 1024;

/// A `.part` file older than this is deleted when an upload starts.
const STALE_AFTER: Duration = Duration::from_secs(3600);

/// How many `name (n).ext` names `keep_both` tries before it gives up.
const MAX_COPY: u32 = 1000;

/// What the last part does when the target already exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnConflict {
    Fail,
    Replace,
    KeepBoth,
}

impl OnConflict {
    /// The wire value; absent is `fail`, an unknown value is `None`.
    pub fn from_wire(value: Option<&str>) -> Option<OnConflict> {
        match value {
            None | Some("fail") => Some(OnConflict::Fail),
            Some("replace") => Some(OnConflict::Replace),
            Some("keep_both") => Some(OnConflict::KeepBoth),
            Some(_) => None,
        }
    }
}

/// One part as the page sent it, already decoded.
#[derive(Debug, Clone)]
pub struct Part {
    pub id: String,
    pub path: String,
    pub offset: u64,
    pub total: u64,
    pub data: Vec<u8>,
    pub on_conflict: OnConflict,
}

/// What the last part did to the target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Placed {
    Created,
    Replaced,
}

impl Placed {
    /// The word the reply and the audit record carry.
    pub fn as_str(self) -> &'static str {
        match self {
            Placed::Created => "created",
            Placed::Replaced => "replaced",
        }
    }
}

/// The result of one accepted part.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// The file is not complete; this many bytes are confirmed.
    Received(u64),
    /// The file is complete and now lives at `path`.
    Done { path: String, placed: Placed },
}

/// A refused part. The page resends from `expected` after `Offset`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UploadError {
    Offset { expected: u64 },
    TooLarge,
    Conflict,
    Invalid,
    Write(WriteError),
}

impl UploadError {
    /// The error reply on the wire.
    pub fn reply(self) -> serde_json::Value {
        let reason = match self {
            UploadError::Offset { expected } => {
                return serde_json::json!({
                    "status": "error",
                    "reason": "offset",
                    "expected": expected,
                })
            }
            UploadError::TooLarge => "too large",
            UploadError::Conflict => "conflict",
            UploadError::Invalid => "invalid",
            UploadError::Write(e) => e.reason(),
        };
        serde_json::json!({ "status": "error", "reason": reason })
    }
}

/// Read one part out of a command payload. The encoded length is bounded
/// before the decode, so an oversized part never allocates its bytes.
pub fn parse(payload: &serde_json::Value) -> Result<Part, UploadError> {
    let text = |key: &str| payload.get(key).and_then(|v| v.as_str());
    let number = |key: &str| payload.get(key).and_then(|v| v.as_u64());
    let id = text("id")
        .filter(|id| is_id(id))
        .ok_or(UploadError::Invalid)?;
    let path = text("path")
        .filter(|p| !p.is_empty())
        .ok_or(UploadError::Invalid)?;
    let offset = number("offset").ok_or(UploadError::Invalid)?;
    let total = number("total").ok_or(UploadError::Invalid)?;
    let on_conflict = OnConflict::from_wire(text("on_conflict")).ok_or(UploadError::Invalid)?;
    let encoded = text("data").unwrap_or("");
    if encoded.len() > PART_MAX.div_ceil(3) * 4 {
        return Err(UploadError::TooLarge);
    }
    let data = data_encoding::BASE64
        .decode(encoded.as_bytes())
        .map_err(|_| UploadError::Invalid)?;
    if data.len() > PART_MAX {
        return Err(UploadError::TooLarge);
    }
    Ok(Part {
        id: id.to_owned(),
        path: path.to_owned(),
        offset,
        total,
        data,
        on_conflict,
    })
}

/// An upload id: exactly 32 lowercase hex digits, so it is a safe file name.
fn is_id(id: &str) -> bool {
    id.len() == 32 && id.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// The largest file `repo_root` accepts, in bytes, from `files.upload_max_mb`.
pub fn limit_bytes(repo_root: &Path) -> u64 {
    let path = repo_root.join(".ralphy").join("settings.json");
    let named = std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .and_then(|v| v.get("files")?.get("upload_max_mb")?.as_u64());
    limit_from(named) * 1024 * 1024
}

/// The limit in MiB for a setting value: the default when absent, the ceiling
/// when above it.
fn limit_from(named: Option<u64>) -> u64 {
    match named {
        None => DEFAULT_MAX_MB,
        Some(mb) if mb > CEILING_MB => {
            tracing::warn!(
                upload_max_mb = mb,
                ceiling = CEILING_MB,
                "files.upload_max_mb is above the ceiling; using the ceiling"
            );
            CEILING_MB
        }
        Some(mb) => mb,
    }
}

/// Accept one part under `root`. `now` decides which old `.part` files the
/// first part deletes; `limit` is the largest `total` in bytes.
///
/// The page sends the parts of one id one after another, so two requests never
/// append to one `.part` file at the same time.
pub fn accept(root: &Path, part: &Part, now: SystemTime, limit: u64) -> Result<Step, UploadError> {
    if part.total > limit {
        return Err(UploadError::TooLarge);
    }
    ensure_dir(root, crate::clipboard::PARENT).map_err(UploadError::Write)?;
    ensure_dir(root, DIR).map_err(UploadError::Write)?;
    if part.offset == 0 {
        sweep(root, now);
    }
    let rel = format!("{DIR}/{}.part", part.id);
    let file =
        confine::confine_write(root, &rel).map_err(|e| UploadError::Write(map_confine(e)))?;
    let held = held_length(&file)?;
    if held != part.offset {
        return Err(UploadError::Offset { expected: held });
    }
    let end = part.offset + part.data.len() as u64;
    if end > part.total {
        return Err(UploadError::TooLarge);
    }
    append(&file, &part.data)?;
    if end < part.total {
        return Ok(Step::Received(end));
    }
    let (path, placed) = place(root, &file, &part.path, part.on_conflict)?;
    Ok(Step::Done { path, placed })
}

/// The length of the `.part` file, 0 when there is none yet. Anything other
/// than a plain file squatting on the name is refused.
fn held_length(file: &Path) -> Result<u64, UploadError> {
    match std::fs::symlink_metadata(file) {
        Ok(meta) if meta.is_file() => Ok(meta.len()),
        Ok(_) => Err(UploadError::Write(WriteError::Confined)),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(0),
        Err(_) => Err(UploadError::Write(WriteError::Io)),
    }
}

fn append(file: &Path, data: &[u8]) -> Result<(), UploadError> {
    let mut out = OpenOptions::new()
        .create(true)
        .append(true)
        .open(file)
        .map_err(|_| UploadError::Write(WriteError::Io))?;
    out.write_all(data)
        .map_err(|_| UploadError::Write(WriteError::Io))
}

/// Delete each `.part` file of [`DIR`] that was last written before
/// `now - STALE_AFTER`. A failure is logged: the upload that asked goes on.
fn sweep(root: &Path, now: SystemTime) {
    let Some(cutoff) = now.checked_sub(STALE_AFTER) else {
        return;
    };
    let entries = match std::fs::read_dir(root.join(DIR)) {
        Ok(entries) => entries,
        Err(e) => {
            tracing::warn!(error = %e, "could not list the upload parts");
            return;
        }
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|ext| ext == "part") && is_older(&path, cutoff) {
            if let Err(e) = std::fs::remove_file(&path) {
                tracing::warn!(error = %e, path = %path.display(), "could not delete an old upload part");
            }
        }
    }
}

/// Whether `path` is a plain file last written before `cutoff`.
fn is_older(path: &Path, cutoff: SystemTime) -> bool {
    std::fs::symlink_metadata(path)
        .ok()
        .filter(|meta| meta.is_file())
        .and_then(|meta| meta.modified().ok())
        .is_some_and(|modified| modified < cutoff)
}

/// Move the complete `.part` file to the confined `to_rel`, by `mode`. Returns
/// the repo-relative path used, with forward slashes, and whether a file was
/// replaced.
///
/// `fail` and `keep_both` check that the name is free, then rename: a file the
/// operator creates at that name in between is replaced. The window is one
/// system call, and the operator is the one writer of the tree.
fn place(
    root: &Path,
    part: &Path,
    to_rel: &str,
    mode: OnConflict,
) -> Result<(String, Placed), UploadError> {
    let to_rel = to_rel.replace('\\', "/");
    let target = fswrite::confine_outside_protected(root, &to_rel).map_err(UploadError::Write)?;
    let (rel, target, placed) = match (exists(&target)?, mode) {
        (None, _) => (to_rel, target, Placed::Created),
        (Some(_), OnConflict::Fail) => return Err(UploadError::Conflict),
        (Some(true), OnConflict::Replace) => (to_rel, target, Placed::Replaced),
        (Some(false), OnConflict::Replace) => return Err(UploadError::Conflict),
        (Some(_), OnConflict::KeepBoth) => {
            let (rel, target) = free_copy_name(root, &to_rel)?;
            (rel, target, Placed::Created)
        }
    };
    std::fs::rename(part, &target).map_err(|_| UploadError::Write(WriteError::Io))?;
    Ok((rel, placed))
}

/// `None` when nothing is at `path`; `Some(true)` for a plain file, and
/// `Some(false)` for a directory or a link, which an upload never replaces.
fn exists(path: &Path) -> Result<Option<bool>, UploadError> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) => Ok(Some(meta.is_file())),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
        Err(_) => Err(UploadError::Write(WriteError::Io)),
    }
}

/// The first free `name (n).ext` beside `rel`, confined like the target.
fn free_copy_name(root: &Path, rel: &str) -> Result<(String, PathBuf), UploadError> {
    let (dir, name) = rel.rsplit_once('/').map_or(("", rel), |(d, n)| (d, n));
    for n in 1..=MAX_COPY {
        let candidate = copy_name(name, n);
        let rel = if dir.is_empty() {
            candidate
        } else {
            format!("{dir}/{candidate}")
        };
        let target = fswrite::confine_outside_protected(root, &rel).map_err(UploadError::Write)?;
        if exists(&target)?.is_none() {
            return Ok((rel, target));
        }
    }
    Err(UploadError::Conflict)
}

/// `report.pdf` → `report (n).pdf`; a name with no extension, or a dot file
/// such as `.env`, takes the number at its end.
fn copy_name(name: &str, n: u32) -> String {
    match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => format!("{stem} ({n}).{ext}"),
        _ => format!("{name} ({n})"),
    }
}

#[cfg(test)]
mod tests;
