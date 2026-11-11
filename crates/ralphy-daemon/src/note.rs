//! The `.note` container (ADR-0064 §3, amended 2026-09-22): a note is a
//! markdown document stored as
//!
//! ```text
//! "RNOT"  version u8 = 1  len u32le  mask(raw-deflate(markdown))
//! ```
//!
//! Two fields the ADR's sketch did not have, both forced by measurement while
//! implementing it:
//!
//! * `len` is the INFLATED byte length. Without it a truncated file inflates
//!   to a short string and reads as a successful, shorter note — which the
//!   next autosave would then write back, turning corruption into data loss.
//!   The decoder refuses a stream that does not produce exactly `len` bytes.
//! * `mask` is an XOR with a fixed 8-byte pattern, because deflate ALONE does
//!   not keep §3's promise that "`strings` finds nothing": a short or
//!   incompressible note is emitted as a deflate STORED block, i.e. the
//!   markdown in the clear behind a 5-byte header. The mask is not a cipher
//!   and there is no key — it is the "small blur" the operator asked for, and
//!   it is what makes the promise true for a three-line note.
//!
//! The container is **opaque, not secret**. There is no key and no promise of
//! confidentiality: anyone holding the bytes can inflate them. What it buys is
//! that a note is not accidentally read — not by an agent grepping the tree,
//! not by a diff view, not by a search hit — and that git treats it as one
//! blob instead of diffing the operator's private thinking line by line. A
//! `.md` sibling format was rejected for the same reason ADR-0064 gives: two
//! spellings of one thing is the confusion, not the choice.
//!
//! The one structured field a note carries is its `color`, in a front-matter
//! block at offset 0. It lives in the FILE and not in the desk record so that
//! closing and reopening a card keeps the colour (ADR-0064 §2) — but the
//! daemon does not PARSE it: this module stores the markdown verbatim and the
//! card (`assets/ui/wb-notes.ts`) is the one implementation of the rule. A
//! second parser here would be a copy with no caller, and the two had already
//! drifted on whitespace before this was noticed.
//!
//! Reads go through [`crate::confine`] like every Observe read; writes go
//! through [`crate::fswrite`], whose `PROTECTED_DIRS` denylist carries one
//! carve-out for exactly `.ralphy/notes/<name>.note` (ADR-0064 §5) so the
//! landing directory is reachable by the note verbs AND by `file.rename` /
//! `file.delete`, and by nothing else.

use std::io::{Read, Write as _};
use std::path::Path;

use flate2::read::DeflateDecoder;
use flate2::write::DeflateEncoder;
use flate2::Compression;

use crate::confine;
use crate::fswrite::{self, WriteError};
use crate::tree::MAX_READ_BYTES;

/// The container's first four bytes. Deliberately not a zip's `PK\x03\x04`: a
/// file that looks like an archive is an invitation to unpack it.
pub const MAGIC: &[u8; 4] = b"RNOT";

/// The container version, written by [`encode`] and required by [`decode`]. A
/// future version bumps this and teaches `decode` the older shape; an unknown
/// version is [`NoteError::NotNote`], never a guess at the bytes.
pub const VERSION: u8 = 1;

/// The extension a note is spelled with. Both directions check it: a `.md` is
/// never decoded as a note, and a note is never written to another name.
pub const EXTENSION: &str = "note";

/// The default landing directory (ADR-0064 §4), gitignored like the rest of
/// `.ralphy/` but still listed by the tree. The operator may choose any other
/// directory inside the checkout — a committable one, for backup — so this is
/// a default, not a confinement.
pub const DIR: &str = ".ralphy/notes";

/// A note read/write failure, in the wire vocabulary the console already
/// knows. `Confined` is a refused target (escape, denylist) and `NotFound`
/// masks an out-of-root read as a plain miss exactly as [`crate::tree`] does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum NoteError {
    /// The path is not spelled `.note`, or the bytes are not a container this
    /// version can read (bad magic, unknown version, truncated stream).
    #[error("not a note")]
    NotNote,
    /// The file — or the text it inflates to — exceeds [`MAX_READ_BYTES`].
    #[error("note too large")]
    TooLarge,
    /// The file does not exist, or the target escapes the root on a read.
    #[error("note not found")]
    NotFound,
    /// The write target escapes the root or names a protected directory.
    #[error("path refused")]
    Confined,
    /// Any other filesystem failure.
    #[error("io error")]
    Io,
}

impl NoteError {
    /// The refusal as the WIRE spells it, pinned by a test like
    /// [`crate::tree::ReadError::reason`]'s: the console branches on these
    /// literals (`not found` paints the missing-file card, `not a note` sends
    /// a double-clicked file to the viewer instead).
    pub fn reason(self) -> &'static str {
        match self {
            NoteError::NotNote => "not a note",
            NoteError::TooLarge => "too large",
            NoteError::NotFound => "not found",
            NoteError::Confined => "refused",
            NoteError::Io => "io error",
        }
    }
}

impl From<WriteError> for NoteError {
    fn from(e: WriteError) -> Self {
        match e {
            WriteError::Confined => NoteError::Confined,
            WriteError::NotFound => NoteError::NotFound,
            // A note write never creates-new and never names an encoding, so
            // neither of these is reachable from here; both are a filesystem
            // surprise, which is what `Io` says.
            WriteError::Conflict | WriteError::Unencodable { .. } | WriteError::Io => NoteError::Io,
        }
    }
}

/// The header's fixed size: magic, version, inflated length.
const HEADER: usize = MAGIC.len() + 1 + 4;

/// The XOR pattern laid over the compressed payload. An arbitrary constant,
/// published in this file: it blurs, it does not hide (see the module doc).
/// Eight bytes so the fold is cheap and the period is not a byte boundary an
/// eye would read through.
const MASK: [u8; 8] = [0x52, 0x4e, 0x4f, 0x54, 0x93, 0x5a, 0x51, 0x1f];

/// Apply [`MASK`] in place. Its own inverse, so one function serves both
/// directions.
fn mask(payload: &mut [u8]) {
    for (i, b) in payload.iter_mut().enumerate() {
        *b ^= MASK[i % MASK.len()];
    }
}

/// Pack `markdown` into a container. The text is stored verbatim — front
/// matter included — so what [`decode`] returns is byte-identical.
pub fn encode(markdown: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(markdown.len() / 3 + HEADER);
    out.extend_from_slice(MAGIC);
    out.push(VERSION);
    let len = u32::try_from(markdown.len()).unwrap_or(u32::MAX);
    out.extend_from_slice(&len.to_le_bytes());
    let mut enc = DeflateEncoder::new(out, Compression::best());
    // Writing to a `Vec` is infallible; the only `Err` a `DeflateEncoder` can
    // surface here would come from its sink.
    let mut bytes = enc
        .write_all(markdown.as_bytes())
        .and_then(|()| enc.finish())
        .expect("deflate into a Vec cannot fail: the sink never errors");
    mask(&mut bytes[HEADER..]);
    bytes
}

/// Unpack a container. Refuses anything that is not this version's shape with
/// [`NoteError::NotNote`], and an inflated text over [`MAX_READ_BYTES`] with
/// [`NoteError::TooLarge`] — the cap is on the OUTPUT, so a small file that
/// inflates to a gigabyte is refused rather than allocated.
pub fn decode(bytes: &[u8]) -> Result<String, NoteError> {
    let Some((head, body)) = bytes.split_at_checked(HEADER) else {
        return Err(NoteError::NotNote);
    };
    if &head[..MAGIC.len()] != MAGIC || head[MAGIC.len()] != VERSION {
        return Err(NoteError::NotNote);
    }
    let length = *head
        .get(5..)
        .and_then(<[u8]>::first_chunk::<4>)
        .expect("a note header is HEADER bytes: a 4-byte length follows the magic and version");
    let len = u64::from(u32::from_le_bytes(length));
    if len > MAX_READ_BYTES {
        return Err(NoteError::TooLarge);
    }
    let mut payload = body.to_vec();
    mask(&mut payload);
    let mut text = Vec::with_capacity(usize::try_from(len).unwrap_or(0));
    DeflateDecoder::new(&payload[..])
        // The declared length bounds the allocation, so a header claiming
        // little and a stream producing much is refused, not held.
        .take(len + 1)
        .read_to_end(&mut text)
        .map_err(|e| confine::miss(NoteError::NotNote, "inflating the note", &e))?;
    // Exactly, not at least: flate2 answers a stream that ends mid-block with
    // the bytes it got and no error, so the length IS the integrity check.
    if text.len() as u64 != len {
        return Err(NoteError::NotNote);
    }
    String::from_utf8(text)
        .map_err(|e| confine::miss(NoteError::NotNote, "reading the note text", &e))
}

/// Read the confined `rel` note under `root` as markdown. The extension must
/// be `.note` ([`NoteError::NotNote`]), the file must fit [`MAX_READ_BYTES`]
/// compressed, and a missing or out-of-root target is [`NoteError::NotFound`].
pub fn read(root: &Path, rel: &str) -> Result<String, NoteError> {
    refuse_other_extension(rel)?;
    let path = confine::confine(root, rel)
        .map_err(|e| confine::miss(NoteError::NotFound, "confining the path", &e))?;
    let meta = std::fs::metadata(&path)
        .map_err(|e| confine::miss(NoteError::NotFound, "reading the note facts", &e))?;
    if meta.len() > MAX_READ_BYTES {
        return Err(NoteError::TooLarge);
    }
    let bytes = std::fs::read(&path)
        .map_err(|e| confine::miss(NoteError::NotFound, "reading the note", &e))?;
    decode(&bytes)
}

/// When the confined `rel` note under `root` was last written, in milliseconds
/// since the epoch — what a card's footer reports as its last save (ADR-0064
/// §7 as amended).
///
/// A read of its own rather than a second value out of [`read`]: that function
/// answers one question, "what does this note say", and a clock is not part of
/// the decode contract. A note whose time cannot be told simply has none to
/// show, so every failure here is `None` and never an error a card must render.
pub fn modified(root: &Path, rel: &str) -> Option<u64> {
    refuse_other_extension(rel).ok()?;
    let path = confine::confine(root, rel).ok()?;
    let at = std::fs::metadata(&path).ok()?.modified().ok()?;
    at.duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|since| u64::try_from(since.as_millis()).unwrap_or(u64::MAX))
}

/// Write `markdown` to the confined `rel` note under `root`, creating or
/// overwriting it (autosave is last-writer-wins — ADR-0064 §7).
///
/// The landing directory is created on first save, one confined level at a
/// time like the clipboard drop's, and ONLY for the default `.ralphy/notes/`:
/// anywhere else in the checkout the operator chose an existing directory, and
/// a note write is not the act that invents a tree.
pub fn write(root: &Path, rel: &str, markdown: &str) -> Result<(), NoteError> {
    refuse_other_extension(rel)?;
    // Created only for a rel that is EXACTLY the landing shape — never for one
    // that merely starts with it. `.ralphy/notes/../../x.note` is refused
    // below, and a refused verb must not leave a `.ralphy/` behind: under a
    // checkout that would manufacture one inside a worktree, which ADR-0063 §2
    // says a worktree does not carry.
    if is_landing_path(rel) {
        fswrite::ensure_dir(root, ".ralphy")?;
        fswrite::ensure_dir(root, DIR)?;
    }
    if markdown.len() as u64 > MAX_READ_BYTES {
        return Err(NoteError::TooLarge);
    }
    Ok(fswrite::write_bytes(root, rel, &encode(markdown))?)
}

/// Exactly `.ralphy/notes/<name>.note`, the shape [`DIR`] names and the one
/// `fswrite`'s denylist carves out. Kept to forward slashes: this is the WIRE
/// spelling the client sent, not a resolved path.
fn is_landing_path(rel: &str) -> bool {
    let parts: Vec<&str> = rel.split('/').collect();
    let [dir, sub, name] = parts[..] else {
        return false;
    };
    dir == ".ralphy" && sub == "notes" && name.len() > ".note".len() && name.ends_with(".note")
}

/// Both directions' first gate: a path this module touches is spelled `.note`.
fn refuse_other_extension(rel: &str) -> Result<(), NoteError> {
    match Path::new(rel).extension().and_then(|e| e.to_str()) {
        Some(ext) if ext == EXTENSION => Ok(()),
        _ => Err(NoteError::NotNote),
    }
}

#[cfg(test)]
mod tests;
