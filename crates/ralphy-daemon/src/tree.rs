//! The Observe read path's file/tree reader (ADR-0036 §4). Three pure functions
//! over a confined path: [`list`] returns one directory level, dropping only
//! `HARD_EXCLUDE` noise; [`read`] returns a file's text or refuses a binary /
//! oversized file — text in ANY encoding the decoder can name, reported with
//! it ([`read_with`], ADR-0036 amendment 2026-09-22); [`read_image`] returns an
//! allowlisted image's bytes (ADR-0049). Confinement ([`crate::confine`]) is
//! the security boundary.
//! `.gitignore` does not filter the listing (ADR-0036, amendment 2026-07-26):
//! the operator works in the ignored files — `.ralphy/`, run logs, build
//! output — and hiding what [`read`] would serve anyway was never protection,
//! only confusion. It only MARKS an entry ([`Entry::ignored`]).
//! The two searches ([`find`] by name, [`grep`] by content) live in
//! [`search`] and share this module's walker and text policy.

use std::path::Path;

use encoding_rs::Encoding;

use crate::confine::{self, ConfineError};
use crate::textcodec::{self, Decoded};

mod ignored;
pub mod search;
pub mod size;
pub mod stat;
pub use search::{find, grep, grep_with, FindHit, GrepHit, SearchBudget, SearchReply};
pub use size::{size, Size, SizeBudget, SizeError};
pub use stat::{stat, Stat};

/// Directory-listing hard-exclude: noise dirs never surfaced in the tree —
/// `.git`, `node_modules`, `target`. This is the ONLY listing filter left
/// (ADR-0036, amendment 2026-07-26), and it is a fixed *name* list, not a git
/// decision: a tree that opens onto 40k transitive packages or git's object
/// store is unusable. `.ralphy` is deliberately NOT here: it is surfaced so
/// `plan.md`/`runs/` are watchable and refresh live (issue #203). `pub(crate)`
/// so the watcher pump ([`crate::watch`]) drops the same noise dirs a
/// `NonRecursive` root watch still fires on.
pub(crate) const HARD_EXCLUDE: &[&str] = &["node_modules", "target", ".git"];

/// One tree entry: a child of the listed directory.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Entry {
    pub name: String,
    pub dir: bool,
    /// The repo ignores this entry (`.gitignore`, `info/exclude`, the global
    /// excludes), or it sits inside an ignored directory. A mark, never a
    /// filter: the entry is listed either way.
    pub ignored: bool,
}

/// List the one-level children of the confined `rel` directory under `root`,
/// with `HARD_EXCLUDE` noise dirs dropped and nothing else filtered — hidden
/// and gitignored entries are listed, the latter marked [`Entry::ignored`].
/// `root` is the checkout root the ignore rules are anchored to. Entries are
/// sorted dirs-first, then by name. A confinement failure (escape/missing)
/// propagates as [`ConfineError`].
pub fn list(root: &Path, rel: &str) -> Result<Vec<Entry>, ConfineError> {
    let base = confine::confine(root, "")?;
    let dir = confine::confine(root, rel)?;
    let parts = ignored::components_below(&base, &dir);
    let rules = ignored::Rules::for_dir(&base, &parts);
    let dir_rel = parts.join("/");

    let mut entries: Vec<Entry> = walker(&dir)
        .max_depth(Some(1))
        .build()
        .filter_map(Result::ok)
        // `max_depth(Some(1))` still yields the root dir itself at depth 0; drop it.
        .filter(|e| e.depth() > 0)
        .map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let dir = e.file_type().is_some_and(|t| t.is_dir());
            let ignored = rules.ignored(&ignored::join(&dir_rel, &name), dir);
            Entry { name, dir, ignored }
        })
        .collect();

    entries.sort_by(|a, b| b.dir.cmp(&a.dir).then_with(|| a.name.cmp(&b.name)));
    Ok(entries)
}

/// The tree's walker over `dir`: every standard filter off (gitignore /
/// exclude / global and the hidden-file rule) and [`HARD_EXCLUDE`] dropped at
/// the entry, so a noise dir is never descended. [`list`] caps it at one level;
/// [`search::find`] lets it recurse. The policy is the same in both because a
/// name search must find exactly what the tree can show.
pub(crate) fn walker(dir: &Path) -> ignore::WalkBuilder {
    let mut b = ignore::WalkBuilder::new(dir);
    b.standard_filters(false).filter_entry(is_not_noise);
    b
}

/// The [`HARD_EXCLUDE`] test as a `filter_entry` predicate.
pub(crate) fn is_not_noise(e: &ignore::DirEntry) -> bool {
    e.file_name()
        .to_str()
        .is_none_or(|n| !HARD_EXCLUDE.contains(&n))
}

/// A [`read`] failure that is not a plain confinement escape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ReadError {
    /// The file's bytes are text in no encoding the decoder can name: a NUL in
    /// the first window that is not the shape of UTF-16, or a multibyte
    /// fallback that fails to decode ([`textcodec::decode`]). The daemon serves
    /// text, so a binary file is refused.
    #[error("binary file")]
    Binary,
    /// The file exceeds [`MAX_READ_BYTES`].
    #[error("file too large")]
    TooLarge,
    /// The file does not exist or escapes the root (an out-of-root read is
    /// reported as a plain miss, never leaking whether the target exists).
    #[error("file not found")]
    NotFound,
    /// [`read_image`] only: the path's extension names no allowlisted image
    /// type, or the bytes do not agree with the type the extension claimed
    /// (ADR-0049 §3).
    #[error("not an image")]
    NotImage,
}

impl ReadError {
    /// The refusal as the WIRE spells it. `ralphy blob read`'s JSON serves the
    /// same three literals for the HEAD side of a diff; the daemon deliberately
    /// does not depend on `ralphy-core`, so this is the daemon-side pin and
    /// `tests::refusal_reasons_are_the_wire_vocabulary` reds if it drifts.
    pub fn reason(self) -> &'static str {
        match self {
            ReadError::Binary => "binary",
            ReadError::TooLarge => "too large",
            ReadError::NotFound => "not found",
            ReadError::NotImage => "not an image",
        }
    }
}

/// Hard cap on a single [`read`]; the daemon serves bytes, so one read is bounded.
pub const MAX_READ_BYTES: u64 = 2 * 1024 * 1024;

/// Read the confined `rel` file under `root` as text, detecting its encoding
/// with the default fallback and answering the text alone. The verb uses
/// [`read_with`]; this is the shape every caller that wants only the string
/// keeps. Refuses a binary file with [`ReadError::Binary`], a file over
/// [`MAX_READ_BYTES`] with [`ReadError::TooLarge`], and a missing/out-of-root
/// target with [`ReadError::NotFound`] (an escape is masked as a miss, never
/// leaking existence).
pub fn read(root: &Path, rel: &str) -> Result<String, ReadError> {
    read_with(root, rel, None, textcodec::DEFAULT_FALLBACK).map(|d| d.text)
}

/// [`read`] that answers the encoding too. `hint` is the client's explicit
/// "reopen with" (detection skipped); `fallback` is the repo's single-byte
/// page for bytes that are neither BOM-marked, UTF-16-shaped nor valid UTF-8
/// ([`textcodec::fallback_for`]).
pub fn read_with(
    root: &Path,
    rel: &str,
    hint: Option<&'static Encoding>,
    fallback: &'static Encoding,
) -> Result<Decoded, ReadError> {
    let path = confine::confine(root, rel)
        .map_err(|e| confine::miss(ReadError::NotFound, "confining the path", &e))?;
    let meta = std::fs::metadata(&path)
        .map_err(|e| confine::miss(ReadError::NotFound, "reading the file facts", &e))?;
    if meta.len() > MAX_READ_BYTES {
        return Err(ReadError::TooLarge);
    }
    let bytes = std::fs::read(&path)
        .map_err(|e| confine::miss(ReadError::NotFound, "reading the file", &e))?;
    textcodec::decode(bytes, hint, fallback).ok_or(ReadError::Binary)
}

/// The text/binary decision, shared by [`read_with`] and [`search::grep`] so a
/// file the daemon would refuse to serve is never a search hit either.
pub(crate) fn text_of_with(bytes: Vec<u8>, fallback: &'static Encoding) -> Option<String> {
    textcodec::decode(bytes, None, fallback).map(|d| d.text)
}

/// An image media type the workbench serves (ADR-0049 §3). A CLOSED allowlist:
/// the daemon names only the types it can also verify, so a format absent here
/// is refused rather than guessed at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageType {
    Png,
    Jpeg,
    Gif,
    Webp,
    Bmp,
    Icon,
    Svg,
}

impl ImageType {
    /// The media type as the wire (and the `data:` URL) spells it.
    pub fn media_type(self) -> &'static str {
        match self {
            ImageType::Png => "image/png",
            ImageType::Jpeg => "image/jpeg",
            ImageType::Gif => "image/gif",
            ImageType::Webp => "image/webp",
            ImageType::Bmp => "image/bmp",
            ImageType::Icon => "image/x-icon",
            ImageType::Svg => "image/svg+xml",
        }
    }

    /// The extension a file of this type is written with. The inverse of
    /// [`ImageType::from_extension`] for the write direction (ADR-0055 §2): the
    /// name on disk is derived from the VERIFIED type, never from a claim.
    pub(crate) fn extension(self) -> &'static str {
        match self {
            ImageType::Png => "png",
            ImageType::Jpeg => "jpg",
            ImageType::Gif => "gif",
            ImageType::Webp => "webp",
            ImageType::Bmp => "bmp",
            ImageType::Icon => "ico",
            ImageType::Svg => "svg",
        }
    }

    /// Which RASTER type do `bytes` carry, with no extension to propose one?
    /// The write direction's sniff (ADR-0055 §2): a paste has no name, so the
    /// bytes alone dispose. Deliberately narrower than the read allowlist —
    /// SVG (script-bearing text), BMP and ICO (no browser paste produces them)
    /// are never tried, so they are refused as `not an image` on this path.
    pub(crate) fn detect_raster(bytes: &[u8]) -> Option<ImageType> {
        [
            ImageType::Png,
            ImageType::Jpeg,
            ImageType::Gif,
            ImageType::Webp,
        ]
        .into_iter()
        .find(|kind| kind.matches(bytes))
    }

    /// The type an extension CLAIMS, case-insensitively. This only proposes;
    /// [`ImageType::matches`] disposes.
    fn from_extension(ext: &str) -> Option<ImageType> {
        match ext.to_ascii_lowercase().as_str() {
            "png" => Some(ImageType::Png),
            "jpg" | "jpeg" => Some(ImageType::Jpeg),
            "gif" => Some(ImageType::Gif),
            "webp" => Some(ImageType::Webp),
            "bmp" => Some(ImageType::Bmp),
            "ico" => Some(ImageType::Icon),
            "svg" => Some(ImageType::Svg),
            _ => None,
        }
    }

    /// Do `bytes` actually carry this type? The daemon must never label bytes
    /// with a media type it did not verify (ADR-0049 §3) — `notes.png` full of
    /// HTML is refused, not served as `image/png` on the extension's say-so.
    fn matches(self, bytes: &[u8]) -> bool {
        match self {
            ImageType::Png => bytes.starts_with(b"\x89PNG\r\n\x1a\n"),
            ImageType::Jpeg => bytes.starts_with(b"\xff\xd8\xff"),
            ImageType::Gif => bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a"),
            // RIFF container: the form type at byte 8 is what makes it a WebP.
            ImageType::Webp => {
                bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP"
            }
            ImageType::Bmp => bytes.starts_with(b"BM"),
            // ICO's reserved-zero + type-1 header; type 2 is a cursor, not an icon.
            ImageType::Icon => bytes.starts_with(b"\x00\x00\x01\x00"),
            // SVG is text, so it has no magic number — it is checked structurally.
            ImageType::Svg => std::str::from_utf8(bytes).is_ok_and(svg_root_element),
        }
    }
}

/// Is `src` an XML document whose root element is `<svg`? Skips what may
/// legally precede the root — whitespace, an XML declaration, comments, a
/// doctype — then requires the root element itself. Every arm either advances
/// past a FOUND terminator or returns, so this cannot loop; an unterminated
/// prologue construct is simply not an SVG.
///
/// Known conservative case: a doctype carrying an internal subset (`<!DOCTYPE
/// svg [ … ]>`) can hold a `>` inside the brackets, which ends the skip early
/// and refuses the file. That direction is the safe one — a refusal the
/// operator sees, not bytes served under an unverified type — and such a
/// prologue is vanishingly rare in a repo asset.
fn svg_root_element(src: &str) -> bool {
    // `<!--` must be tried BEFORE `<!`, which is its prefix.
    const PROLOGUE: [(&str, &str); 3] = [("<?", "?>"), ("<!--", "-->"), ("<!", ">")];
    let mut rest = src.trim_start();
    loop {
        let Some((open, close)) = PROLOGUE.iter().find(|(open, _)| rest.starts_with(open)) else {
            // Not a prologue construct: this is the root element (or not an SVG).
            return rest.starts_with("<svg");
        };
        let after = &rest[open.len()..];
        let Some(i) = after.find(close) else {
            return false;
        };
        rest = after[i + close.len()..].trim_start();
    }
}

/// An image the Observe read path serves: the VERIFIED media type plus the raw
/// bytes. The caller base64s the bytes into the reply (ADR-0049 §2); this
/// module deliberately does no encoding, so the reader stays pure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    pub media_type: &'static str,
    pub bytes: Vec<u8>,
}

/// Hard cap on a single [`read_image`], larger than [`MAX_READ_BYTES`] because
/// an image's cost is decode and paint, not lines in an editor (ADR-0049 §4).
pub const MAX_IMAGE_BYTES: u64 = 4 * 1024 * 1024;

/// The one transport cap on a command a client sends, on the `spawn` channel,
/// `POST /api/command`, the console channel and `/api/peer/command`: the base64 size of a
/// [`MAX_IMAGE_BYTES`] image (the largest payload a verb takes, `image.write`)
/// plus 64 KiB for the JSON envelope. A larger message is refused before it is
/// parsed (ADR-0072 D11).
#[expect(
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    reason = "MAX_IMAGE_BYTES is 4 MiB, which fits usize on every target"
)]
pub const MAX_COMMAND_BYTES: usize = (MAX_IMAGE_BYTES as usize).div_ceil(3) * 4 + 64 * 1024;

/// Read the confined `rel` file under `root` as an allowlisted image. The
/// extension picks the candidate [`ImageType`] and the bytes must AGREE with it
/// (`ImageType::matches`); a disagreement — or an extension naming no image
/// type at all — is [`ReadError::NotImage`]. Oversize is [`ReadError::TooLarge`]
/// and a missing/out-of-root target is [`ReadError::NotFound`], masking an
/// escape as a plain miss exactly like [`read`].
pub fn read_image(root: &Path, rel: &str) -> Result<Image, ReadError> {
    let ext = Path::new(rel)
        .extension()
        .and_then(|e| e.to_str())
        .ok_or(ReadError::NotImage)?;
    let kind = ImageType::from_extension(ext).ok_or(ReadError::NotImage)?;

    let path = confine::confine(root, rel)
        .map_err(|e| confine::miss(ReadError::NotFound, "confining the path", &e))?;
    let meta = std::fs::metadata(&path)
        .map_err(|e| confine::miss(ReadError::NotFound, "reading the file facts", &e))?;
    if meta.len() > MAX_IMAGE_BYTES {
        return Err(ReadError::TooLarge);
    }
    let bytes = std::fs::read(&path)
        .map_err(|e| confine::miss(ReadError::NotFound, "reading the file", &e))?;
    if !kind.matches(&bytes) {
        return Err(ReadError::NotImage);
    }
    Ok(Image {
        media_type: kind.media_type(),
        bytes,
    })
}

#[cfg(test)]
mod tests;
