//! The file bytes reader (ADR-0091 D3, D4): one confined range of a file of a
//! closed list of kinds. The extension proposes the kind and the first bytes
//! decide; a file that disagrees, or has no listed kind, is not found.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use super::ImageType;
use crate::confine;

const MIB: u64 = 1024 * 1024;
/// Cap on the whole file of an image, a document, a csv or a tsv file.
pub(super) const MAX_WHOLE_FILE_BYTES: u64 = 32 * MIB;
/// Cap on the whole file of an xlsx or an ods file (ADR-0091 D3, amended 2026-11-23).
pub(super) const MAX_SHEET_BYTES: u64 = 16 * MIB;
/// Cap on the whole file of a font.
pub(super) const MAX_FONT_BYTES: u64 = 16 * MIB;
/// Cap on the bytes of one answer.
pub(super) const MAX_ANSWER_BYTES: u64 = 8 * MIB;
/// The first bytes the kind checks read.
const HEAD_BYTES: u64 = 8 * 1024;

/// The first-bytes check of a kind.
#[derive(Clone, Copy)]
enum Check {
    Image(ImageType),
    Pdf,
    Zip,
    OpenDoc(&'static [u8]),
    Text,
    Mp4,
    Webm,
    Mp3,
    Wav,
    Ogg,
    Font(&'static [&'static [u8]]),
}

/// One row of the kind table: media type, check, and cap on the file.
struct Kind {
    media_type: &'static str,
    check: Check,
    cap: Option<u64>,
}

const ODT_MIME: &[u8] = b"application/vnd.oasis.opendocument.text";
const ODS_MIME: &[u8] = b"application/vnd.oasis.opendocument.spreadsheet";
const SFNT: &[u8] = b"\x00\x01\x00\x00";

const fn row(media_type: &'static str, check: Check, cap: Option<u64>) -> Kind {
    Kind {
        media_type,
        check,
        cap,
    }
}

fn kind_of(ext: &str) -> Option<Kind> {
    let whole = Some(MAX_WHOLE_FILE_BYTES);
    let font = Some(MAX_FONT_BYTES);
    match ext.to_ascii_lowercase().as_str() {
        "pdf" => Some(row("application/pdf", Check::Pdf, None)),
        "docx" => Some(row(DOCX, Check::Zip, whole)),
        "odt" => Some(row(ODT, Check::OpenDoc(ODT_MIME), whole)),
        "xlsx" => Some(row(XLSX, Check::Zip, Some(MAX_SHEET_BYTES))),
        "ods" => Some(row(ODS, Check::OpenDoc(ODS_MIME), Some(MAX_SHEET_BYTES))),
        "csv" => Some(row("text/csv", Check::Text, whole)),
        "tsv" => Some(row("text/tab-separated-values", Check::Text, whole)),
        "mp4" => Some(row("video/mp4", Check::Mp4, None)),
        "m4a" => Some(row("audio/mp4", Check::Mp4, None)),
        "webm" => Some(row("video/webm", Check::Webm, None)),
        "mp3" => Some(row("audio/mpeg", Check::Mp3, None)),
        "wav" => Some(row("audio/wav", Check::Wav, None)),
        "ogg" | "oga" => Some(row("audio/ogg", Check::Ogg, None)),
        "ogv" => Some(row("video/ogg", Check::Ogg, None)),
        "ttf" => Some(row("font/ttf", Check::Font(&[SFNT, b"true"]), font)),
        "otf" => Some(row("font/otf", Check::Font(&[b"OTTO", SFNT]), font)),
        "woff" => Some(row("font/woff", Check::Font(&[b"wOFF"]), font)),
        "woff2" => Some(row("font/woff2", Check::Font(&[b"wOF2"]), font)),
        other => {
            let image = ImageType::from_extension(other)?;
            Some(row(image.media_type(), Check::Image(image), whole))
        }
    }
}

const DOCX: &str = "application/vnd.openxmlformats-officedocument.wordprocessingml.document";
const XLSX: &str = "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet";
const ODT: &str = "application/vnd.oasis.opendocument.text";
const ODS: &str = "application/vnd.oasis.opendocument.spreadsheet";

impl Check {
    fn passes(self, head: &[u8]) -> bool {
        match self {
            Check::Image(image) => image.matches(head),
            Check::Pdf => head.starts_with(b"%PDF-"),
            Check::Zip => head.starts_with(b"PK\x03\x04"),
            Check::OpenDoc(mime) => open_doc_mimetype_is(head, mime),
            Check::Text => !head.contains(&0),
            Check::Mp4 => head.get(4..8) == Some(b"ftyp"),
            Check::Webm => head.starts_with(b"\x1a\x45\xdf\xa3"),
            Check::Mp3 => head.starts_with(b"ID3") || is_mpeg_frame(head),
            Check::Wav => head.starts_with(b"RIFF") && head.get(8..12) == Some(b"WAVE"),
            Check::Ogg => head.starts_with(b"OggS"),
            Check::Font(magics) => magics.iter().any(|magic| head.starts_with(magic)),
        }
    }
}

fn is_mpeg_frame(head: &[u8]) -> bool {
    matches!(head, [0xff, second, ..] if second & 0xe0 == 0xe0)
}

/// An `OpenDocument` package stores `mimetype` first and uncompressed: its name starts at
/// byte 30 of the first local header and its text follows the name.
fn open_doc_mimetype_is(head: &[u8], mime: &[u8]) -> bool {
    const NAME: &[u8] = b"mimetype";
    let name_end = 30 + NAME.len();
    head.starts_with(b"PK\x03\x04")
        && head.get(30..name_end) == Some(NAME)
        && head
            .get(name_end..)
            .is_some_and(|rest| rest.starts_with(mime))
}

/// A byte range as `Range: bytes=start-end` names it; `end` is inclusive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ByteRange {
    pub start: u64,
    pub end: Option<u64>,
}

/// The bytes of one answer, with the facts a `206` reply needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bytes {
    pub media_type: &'static str,
    pub bytes: Vec<u8>,
    pub start: u64,
    /// The length of the whole file.
    pub total: u64,
}

/// A [`read_range`] refusal. A file outside the list, outside the root or
/// whose bytes disagree with its extension is `NotFound`, so an answer leaks
/// nothing about the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum BytesError {
    #[error("file not found")]
    NotFound,
    #[error("file too large")]
    TooLarge,
    #[error("range not satisfiable")]
    Unsatisfiable,
}

/// Read the asked range of the confined `rel` file under `root`.
pub fn read_range(root: &Path, rel: &str, range: ByteRange) -> Result<Bytes, BytesError> {
    let ext = Path::new(rel)
        .extension()
        .and_then(|e| e.to_str())
        .ok_or(BytesError::NotFound)?;
    let kind = kind_of(ext).ok_or(BytesError::NotFound)?;
    let path = confine::confine(root, rel)
        .map_err(|e| confine::miss(BytesError::NotFound, "confining the path", &e))?;
    let mut file = File::open(&path)
        .map_err(|e| confine::miss(BytesError::NotFound, "opening the file", &e))?;
    let total = file
        .metadata()
        .map_err(|e| confine::miss(BytesError::NotFound, "reading the file facts", &e))?
        .len();
    if kind.cap.is_some_and(|cap| total > cap) {
        return Err(BytesError::TooLarge);
    }
    let head = read_up_to(&mut file, HEAD_BYTES)?;
    if !kind.check.passes(&head) {
        return Err(BytesError::NotFound);
    }
    let (start, last) = resolve(range, total)?;
    file.seek(SeekFrom::Start(start))
        .map_err(|e| confine::miss(BytesError::NotFound, "seeking in the file", &e))?;
    let bytes = read_up_to(&mut file, last - start + 1)?;
    Ok(Bytes {
        media_type: kind.media_type,
        bytes,
        start,
        total,
    })
}

/// The first and last byte of the answer: inside the file, at most
/// [`MAX_ANSWER_BYTES`] long.
fn resolve(range: ByteRange, total: u64) -> Result<(u64, u64), BytesError> {
    let start = range.start;
    if start >= total {
        return Err(BytesError::Unsatisfiable);
    }
    let asked = range.end.unwrap_or(u64::MAX);
    let last = asked
        .min(total - 1)
        .min(start.saturating_add(MAX_ANSWER_BYTES - 1));
    if last < start {
        return Err(BytesError::Unsatisfiable);
    }
    Ok((start, last))
}

fn read_up_to(file: &mut File, len: u64) -> Result<Vec<u8>, BytesError> {
    let mut bytes = Vec::new();
    file.take(len)
        .read_to_end(&mut bytes)
        .map_err(|e| confine::miss(BytesError::NotFound, "reading the file", &e))?;
    Ok(bytes)
}

#[cfg(test)]
mod tests;
