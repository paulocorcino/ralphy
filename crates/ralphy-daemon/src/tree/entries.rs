//! The listing of an archive behind `file.entries` (ADR-0091 D7): the entries
//! of a zip from its central directory, and of a tar or a tar.gz from its
//! stream. Nothing is written to disk, and an archive inside is not opened.

use std::borrow::Cow;
use std::cell::Cell;
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;

use crate::confine;

/// The most entries one listing holds.
pub(crate) const MAX_ENTRIES: usize = 10_000;
/// The most bytes a tar.gz listing inflates.
pub(crate) const MAX_INFLATED_BYTES: u64 = 1024 * 1024 * 1024;

/// The limits of one listing.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Caps {
    pub(crate) entries: usize,
    pub(crate) inflated: u64,
}

impl Default for Caps {
    fn default() -> Self {
        Caps {
            entries: MAX_ENTRIES,
            inflated: MAX_INFLATED_BYTES,
        }
    }
}

/// One entry of an archive. `modified` is in Unix milliseconds.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArchiveEntry {
    pub path: String,
    pub size: u64,
    /// A zip only: the bytes the entry takes in the archive.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub compressed_size: Option<u64>,
    pub folder: bool,
    pub modified: Option<i64>,
}

/// The entries of an archive; with `truncated`, a cap stopped the listing or
/// the archive ended early.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Listing {
    pub entries: Vec<ArchiveEntry>,
    pub truncated: bool,
}

/// An [`entries`] refusal. A path outside the root is `NotFound`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum EntriesError {
    #[error("file not found")]
    NotFound,
    #[error("not an archive")]
    NotArchive,
}

impl EntriesError {
    /// The refusal as the wire spells it.
    pub fn reason(self) -> &'static str {
        match self {
            EntriesError::NotFound => "not found",
            EntriesError::NotArchive => "not an archive",
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum Format {
    Zip,
    Tar,
    TarGz,
}

fn format_of(rel: &str) -> Option<Format> {
    let name = Path::new(rel);
    let ext = name.extension()?.to_str()?.to_ascii_lowercase();
    let inner = name.file_stem().map(Path::new).and_then(Path::extension);
    let inner_is_tar = inner.is_some_and(|e| e.eq_ignore_ascii_case("tar"));
    match ext.as_str() {
        "zip" => Some(Format::Zip),
        "tar" => Some(Format::Tar),
        "tgz" => Some(Format::TarGz),
        "gz" if inner_is_tar => Some(Format::TarGz),
        _ => None,
    }
}

/// List the confined `rel` archive under `root`.
pub fn entries(root: &Path, rel: &str) -> Result<Listing, EntriesError> {
    entries_with(root, rel, Caps::default())
}

pub(crate) fn entries_with(root: &Path, rel: &str, caps: Caps) -> Result<Listing, EntriesError> {
    let format = format_of(rel).ok_or(EntriesError::NotArchive)?;
    let path = confine::confine(root, rel)
        .map_err(|e| confine::miss(EntriesError::NotFound, "confining the path", &e))?;
    let file = File::open(&path)
        .map_err(|e| confine::miss(EntriesError::NotFound, "opening the archive", &e))?;
    let reader = BufReader::new(file);
    match format {
        Format::Zip => list_zip(reader, caps),
        Format::Tar => {
            let mut archive = tar::Archive::new(reader);
            let all = archive
                .entries_with_seek()
                .map_err(|e| confine::miss(EntriesError::NotArchive, "reading the tar", &e))?;
            list_tar(all, caps.entries)
        }
        Format::TarGz => {
            let hit = Cell::new(false);
            let inflated = Capped {
                inner: flate2::read::MultiGzDecoder::new(reader),
                left: caps.inflated,
                hit: &hit,
            };
            let mut archive = tar::Archive::new(inflated);
            let all = archive
                .entries()
                .map_err(|e| confine::miss(EntriesError::NotArchive, "reading the tar.gz", &e))?;
            let mut listing = list_tar(all, caps.entries)?;
            listing.truncated |= hit.get();
            Ok(listing)
        }
    }
}

fn list_zip(reader: BufReader<File>, caps: Caps) -> Result<Listing, EntriesError> {
    let archive = zip::ZipArchive::new(reader)
        .map_err(|e| confine::miss(EntriesError::NotArchive, "reading the zip", &e))?;
    let shown = archive.len().min(caps.entries);
    let mut entries = Vec::with_capacity(shown);
    for index in 0..shown {
        let entry = archive
            .by_index_data(index)
            .map_err(|e| confine::miss(EntriesError::NotArchive, "reading a zip entry", &e))?;
        entries.push(ArchiveEntry {
            path: entry.name().map_or_else(|_| String::new(), Cow::into_owned),
            size: entry.size(),
            compressed_size: Some(entry.compressed_size()),
            folder: entry.is_dir(),
            modified: entry.last_modified().map(dos_time_ms),
        });
    }
    Ok(Listing {
        entries,
        truncated: archive.len() > shown,
    })
}

/// The tar entries, up to `cap`. An error before the first entry means the
/// file is no tar; a later one ends the listing as partial.
fn list_tar<R: Read>(all: tar::Entries<'_, R>, cap: usize) -> Result<Listing, EntriesError> {
    let mut entries = Vec::new();
    let mut all = all;
    loop {
        if entries.len() == cap {
            let more = all.next().is_some();
            return Ok(Listing {
                entries,
                truncated: more,
            });
        }
        match all.next() {
            None => {
                return Ok(Listing {
                    entries,
                    truncated: false,
                })
            }
            Some(Ok(entry)) => entries.push(tar_entry(&entry)),
            Some(Err(e)) if entries.is_empty() => {
                return Err(confine::miss(
                    EntriesError::NotArchive,
                    "reading a tar entry",
                    &e,
                ))
            }
            Some(Err(e)) => {
                tracing::debug!(error = ?e, "a tar listing ended early");
                return Ok(Listing {
                    entries,
                    truncated: true,
                });
            }
        }
    }
}

fn tar_entry<R: Read>(entry: &tar::Entry<'_, R>) -> ArchiveEntry {
    let header = entry.header();
    ArchiveEntry {
        path: String::from_utf8_lossy(&entry.path_bytes()).into_owned(),
        size: entry.size(),
        compressed_size: None,
        folder: header.entry_type().is_dir(),
        modified: header
            .mtime()
            .ok()
            .and_then(|s| i64::try_from(s).ok())
            .map(|s| s.saturating_mul(1000)),
    }
}

/// A zip time, read as UTC, in Unix milliseconds.
fn dos_time_ms(t: zip::DateTime) -> i64 {
    let days = days_from_civil(
        i64::from(t.year()),
        i64::from(t.month()),
        i64::from(t.day()),
    );
    let seconds = i64::from(t.hour()) * 3600 + i64::from(t.minute()) * 60 + i64::from(t.second());
    (days * 86_400 + seconds) * 1000
}

/// Days since 1970-01-01 of a date of the proleptic Gregorian calendar.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// A reader that ends after `left` bytes and marks `hit` when it does.
struct Capped<'a, R> {
    inner: R,
    left: u64,
    hit: &'a Cell<bool>,
}

impl<R: Read> Read for Capped<'_, R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.left == 0 {
            self.hit.set(true);
            return Ok(0);
        }
        let room = usize::try_from(self.left)
            .unwrap_or(usize::MAX)
            .min(buf.len());
        let read = self.inner.read(buf.get_mut(..room).unwrap_or_default())?;
        self.left = self
            .left
            .saturating_sub(u64::try_from(read).unwrap_or(u64::MAX));
        Ok(read)
    }
}

#[cfg(test)]
mod tests;
