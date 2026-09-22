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
//! block at offset 0 ([`with_color`]/[`color_of`]). It lives in the FILE and
//! not in the desk record so that closing and reopening a card keeps the
//! colour (ADR-0064 §2). One field of a closed set is not YAML, so this module
//! parses it by hand rather than growing the daemon a serialisation dependency.
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoteError {
    /// The path is not spelled `.note`, or the bytes are not a container this
    /// version can read (bad magic, unknown version, truncated stream).
    NotNote,
    /// The file — or the text it inflates to — exceeds [`MAX_READ_BYTES`].
    TooLarge,
    /// The file does not exist, or the target escapes the root on a read.
    NotFound,
    /// The write target escapes the root or names a protected directory.
    Confined,
    /// Any other filesystem failure.
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

impl std::fmt::Display for NoteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NoteError::NotNote => write!(f, "not a note"),
            NoteError::TooLarge => write!(f, "note too large"),
            NoteError::NotFound => write!(f, "note not found"),
            NoteError::Confined => write!(f, "path refused"),
            NoteError::Io => write!(f, "io error"),
        }
    }
}

impl std::error::Error for NoteError {}

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

/// The card's tone (ADR-0064 §3), a closed set so the stylesheet owns the
/// actual colours and the file carries only the name. `Sand` is the default —
/// a note whose front matter is absent or unparseable is a sand note, never a
/// refusal: the markdown is the document, the colour is decoration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Color {
    Ochre,
    Sage,
    Rose,
    Slate,
    Plum,
    #[default]
    Sand,
}

impl Color {
    /// The name as the front matter and the DOM spell it (`data-tone`).
    pub fn as_str(self) -> &'static str {
        match self {
            Color::Ochre => "ochre",
            Color::Sage => "sage",
            Color::Rose => "rose",
            Color::Slate => "slate",
            Color::Plum => "plum",
            Color::Sand => "sand",
        }
    }

    /// Every tone, for exhaustive round-trips.
    pub const ALL: &'static [Color] = &[
        Color::Ochre,
        Color::Sage,
        Color::Rose,
        Color::Slate,
        Color::Plum,
        Color::Sand,
    ];

    /// Parse a tone name. Unknown is `None`, and the caller defaults.
    pub fn parse(name: &str) -> Option<Color> {
        Color::ALL.iter().copied().find(|c| c.as_str() == name)
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
    out.extend_from_slice(&(markdown.len() as u32).to_le_bytes());
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
    let len = u32::from_le_bytes([head[5], head[6], head[7], head[8]]) as u64;
    if len > MAX_READ_BYTES {
        return Err(NoteError::TooLarge);
    }
    let mut payload = body.to_vec();
    mask(&mut payload);
    let mut text = Vec::with_capacity(len as usize);
    DeflateDecoder::new(&payload[..])
        // The declared length bounds the allocation, so a header claiming
        // little and a stream producing much is refused, not held.
        .take(len + 1)
        .read_to_end(&mut text)
        .map_err(|_| NoteError::NotNote)?;
    // Exactly, not at least: flate2 answers a stream that ends mid-block with
    // the bytes it got and no error, so the length IS the integrity check.
    if text.len() as u64 != len {
        return Err(NoteError::NotNote);
    }
    String::from_utf8(text).map_err(|_| NoteError::NotNote)
}

/// Read the confined `rel` note under `root` as markdown. The extension must
/// be `.note` ([`NoteError::NotNote`]), the file must fit [`MAX_READ_BYTES`]
/// compressed, and a missing or out-of-root target is [`NoteError::NotFound`].
pub fn read(root: &Path, rel: &str) -> Result<String, NoteError> {
    refuse_other_extension(rel)?;
    let path = confine::confine(root, rel).map_err(|_| NoteError::NotFound)?;
    let meta = std::fs::metadata(&path).map_err(|_| NoteError::NotFound)?;
    if meta.len() > MAX_READ_BYTES {
        return Err(NoteError::TooLarge);
    }
    let bytes = std::fs::read(&path).map_err(|_| NoteError::NotFound)?;
    decode(&bytes)
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
    if rel.starts_with(&format!("{DIR}/")) {
        fswrite::ensure_dir(root, ".ralphy")?;
        fswrite::ensure_dir(root, DIR)?;
    }
    if markdown.len() as u64 > MAX_READ_BYTES {
        return Err(NoteError::TooLarge);
    }
    Ok(fswrite::write_bytes(root, rel, &encode(markdown))?)
}

/// Both directions' first gate: a path this module touches is spelled `.note`.
fn refuse_other_extension(rel: &str) -> Result<(), NoteError> {
    match Path::new(rel).extension().and_then(|e| e.to_str()) {
        Some(ext) if ext == EXTENSION => Ok(()),
        _ => Err(NoteError::NotNote),
    }
}

/// The note's colour, or `None` when the document carries no front matter the
/// parser recognises. The caller defaults to [`Color::Sand`].
pub fn color_of(markdown: &str) -> Option<Color> {
    let (block, _) = split_front_matter(markdown)?;
    let value = block
        .lines()
        .find_map(|l| l.trim().strip_prefix("color:"))?
        .trim();
    Color::parse(value)
}

/// The document without its front matter — what the editor shows.
pub fn body_of(markdown: &str) -> &str {
    match split_front_matter(markdown) {
        Some((_, rest)) => rest,
        None => markdown,
    }
}

/// `markdown` with a front-matter block naming `color`, replacing one that is
/// already there. The block is written in exactly one shape (`---`, one
/// `color:` line, `---`, LF) so the JS side can produce byte-identical output
/// and an autosave never rewrites the header for no reason.
pub fn with_color(markdown: &str, color: Color) -> String {
    format!("---\ncolor: {}\n---\n{}", color.as_str(), body_of(markdown))
}

/// Split a leading front-matter block into `(inner, rest)`. The opening fence
/// must be the very first line and the closing fence a line of exactly `---`;
/// CRLF is accepted because an operator's editor may have touched the file.
fn split_front_matter(markdown: &str) -> Option<(&str, &str)> {
    let after_open = markdown
        .strip_prefix("---\n")
        .or_else(|| markdown.strip_prefix("---\r\n"))?;
    let mut offset = 0;
    for line in after_open.split_inclusive('\n') {
        if line.trim_end_matches(['\r', '\n']) == "---" {
            return Some((&after_open[..offset], &after_open[offset + line.len()..]));
        }
        offset += line.len();
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn a_container_round_trips_every_text_a_note_can_hold() {
        for text in [
            "",
            "# Title\n\n- [ ] one\n",
            "acentuação, 日本語, \u{1F5D2}\r\nCRLF kept\r\n",
            &"x".repeat(1024 * 1024),
        ] {
            assert_eq!(decode(&encode(text)).unwrap(), text);
        }
    }

    #[test]
    fn a_container_carries_no_plaintext() {
        // The whole point of the format (ADR-0064 §3, "`strings` finds
        // nothing"). Short and incompressible texts are the cases that broke
        // it before the mask: deflate emits them as a STORED block, which is
        // the markdown in the clear behind a five-byte header.
        for text in [
            "# Groceries\n\nthe secret ingredient is celery\n",
            "celery",
            "",
        ] {
            let bytes = encode(text);
            assert!(bytes.starts_with(MAGIC));
            let window = &bytes[HEADER..];
            assert!(
                !window.windows(6).any(|w| w == b"celery"),
                "{text:?} -> {:?}",
                String::from_utf8_lossy(&bytes)
            );
        }
    }

    #[test]
    fn a_tampered_header_is_not_a_note() {
        // The length is the integrity check: a stream that inflates to more or
        // to less than the header claims is refused, never half-read.
        let good = encode("# Title\n\nbody enough to compress, body enough\n");
        let mut short = good.clone();
        short[5] = 3;
        assert_eq!(decode(&short), Err(NoteError::NotNote));
        let mut long = good.clone();
        long[5] = long[5].wrapping_add(1);
        assert_eq!(decode(&long), Err(NoteError::NotNote));
        // An unmasked payload is not a stream at all.
        let mut naked = good.clone();
        super::mask(&mut naked[HEADER..]);
        assert_eq!(decode(&naked), Err(NoteError::NotNote));
    }

    #[test]
    fn decode_refuses_what_is_not_this_version_of_the_container() {
        assert_eq!(decode(b""), Err(NoteError::NotNote));
        assert_eq!(decode(b"RNOT"), Err(NoteError::NotNote));
        assert_eq!(decode(b"# plain markdown"), Err(NoteError::NotNote));
        // Right magic, wrong version.
        let mut future = encode("hi");
        future[MAGIC.len()] = VERSION + 1;
        assert_eq!(decode(&future), Err(NoteError::NotNote));
        // Right header, truncated stream.
        let good = encode(&"hello ".repeat(500));
        assert_eq!(
            decode(&good[..good.len() - 10]),
            Err(NoteError::NotNote),
            "a truncated deflate stream is not a note"
        );
    }

    #[test]
    fn decode_refuses_a_text_that_inflates_past_the_read_cap() {
        // A small file whose inflated text is over the cap: refused on OUTPUT
        // size, so the bomb is never held whole beyond one byte of slack.
        let bomb = encode(&"a".repeat(MAX_READ_BYTES as usize + 1));
        assert!((bomb.len() as u64) < MAX_READ_BYTES, "{}", bomb.len());
        assert_eq!(decode(&bomb), Err(NoteError::TooLarge));
    }

    #[test]
    fn front_matter_carries_the_colour_and_nothing_else() {
        let doc = with_color("# Title\n\nbody\n", Color::Plum);
        assert_eq!(doc, "---\ncolor: plum\n---\n# Title\n\nbody\n");
        assert_eq!(color_of(&doc), Some(Color::Plum));
        assert_eq!(body_of(&doc), "# Title\n\nbody\n");
        // Replacing, not stacking.
        let again = with_color(&doc, Color::Sage);
        assert_eq!(again, "---\ncolor: sage\n---\n# Title\n\nbody\n");
        assert_eq!(color_of(&again), Some(Color::Sage));
    }

    #[test]
    fn a_document_without_recognised_front_matter_has_no_colour() {
        assert_eq!(color_of(""), None);
        assert_eq!(color_of("# Title\n"), None);
        // An unterminated fence is not front matter.
        assert_eq!(color_of("---\ncolor: sage\n"), None);
        // A fence that is not the first line is a horizontal rule.
        assert_eq!(color_of("# Title\n---\ncolor: sage\n---\n"), None);
        // A tone outside the closed set falls back to the default.
        assert_eq!(color_of("---\ncolor: chartreuse\n---\n"), None);
        assert_eq!(body_of("# Title\n"), "# Title\n");
        assert_eq!(Color::default(), Color::Sand);
    }

    #[test]
    fn front_matter_survives_an_editor_that_writes_crlf() {
        let doc = "---\r\ncolor: rose\r\n---\r\n# Title\r\n";
        assert_eq!(color_of(doc), Some(Color::Rose));
        assert_eq!(body_of(doc), "# Title\r\n");
    }

    #[test]
    fn every_tone_round_trips_its_name() {
        for c in Color::ALL {
            assert_eq!(Color::parse(c.as_str()), Some(*c));
        }
        assert_eq!(Color::parse("SAND"), None);
        assert_eq!(Color::parse(""), None);
    }

    #[test]
    fn refusal_reasons_are_the_wire_vocabulary() {
        assert_eq!(NoteError::NotNote.reason(), "not a note");
        assert_eq!(NoteError::TooLarge.reason(), "too large");
        assert_eq!(NoteError::NotFound.reason(), "not found");
        assert_eq!(NoteError::Confined.reason(), "refused");
        assert_eq!(NoteError::Io.reason(), "io error");
    }

    #[test]
    fn write_creates_the_default_dir_on_the_first_save_and_read_returns_the_text() {
        let root = tempfile::tempdir().unwrap();
        assert!(!root.path().join(DIR).exists());
        let rel = format!("{DIR}/groceries.note");
        write(root.path(), &rel, "# Groceries\n").unwrap();
        assert_eq!(read(root.path(), &rel).unwrap(), "# Groceries\n");
        // Overwriting is the autosave, not a conflict.
        write(root.path(), &rel, "# Groceries\n\n- celery\n").unwrap();
        assert_eq!(
            read(root.path(), &rel).unwrap(),
            "# Groceries\n\n- celery\n"
        );
        assert!(fs::read(root.path().join(&rel)).unwrap().starts_with(MAGIC));
    }

    #[test]
    fn write_lands_in_a_directory_the_operator_chose_inside_the_checkout() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join("docs")).unwrap();
        write(root.path(), "docs/plan.note", "# Plan\n").unwrap();
        assert_eq!(read(root.path(), "docs/plan.note").unwrap(), "# Plan\n");
        // A directory that does not exist is a miss, not an invention: only
        // the default landing dir is created.
        assert_eq!(
            write(root.path(), "elsewhere/plan.note", "# Plan\n"),
            Err(NoteError::NotFound)
        );
        assert!(!root.path().join("elsewhere").exists());
    }

    #[test]
    fn only_a_dot_note_path_is_a_note_in_either_direction() {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("readme.md"), "# hi").unwrap();
        assert_eq!(read(root.path(), "readme.md"), Err(NoteError::NotNote));
        assert_eq!(
            write(root.path(), ".ralphy/notes/x.md", "# hi"),
            Err(NoteError::NotNote)
        );
        assert_eq!(read(root.path(), ""), Err(NoteError::NotNote));
        // A `.note` whose bytes are something else refuses on the content.
        fs::write(root.path().join("fake.note"), "# not a container").unwrap();
        assert_eq!(read(root.path(), "fake.note"), Err(NoteError::NotNote));
    }

    #[test]
    fn read_masks_a_missing_or_escaping_target_as_a_miss() {
        let root = tempfile::tempdir().unwrap();
        assert_eq!(read(root.path(), "gone.note"), Err(NoteError::NotFound));
        assert_eq!(
            read(root.path(), "../outside.note"),
            Err(NoteError::NotFound)
        );
    }

    #[test]
    fn write_refuses_a_target_the_denylist_owns() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join(".ralphy")).unwrap();
        // The carve-out is exactly `.ralphy/notes/<name>.note`; a sibling of
        // the landing dir is still `.ralphy`, and still refused.
        assert_eq!(
            write(root.path(), ".ralphy/x.note", "# hi"),
            Err(NoteError::Confined)
        );
        assert_eq!(
            write(root.path(), ".ralphy/notes/deep/x.note", "# hi"),
            Err(NoteError::Confined)
        );
        assert_eq!(
            write(root.path(), "../outside.note", "# hi"),
            Err(NoteError::Confined)
        );
    }
}
