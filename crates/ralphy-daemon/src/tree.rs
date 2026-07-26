//! The Observe read path's file/tree reader (ADR-0036 §4). Three pure functions
//! over a confined path: [`list`] returns one directory level, dropping only
//! [`HARD_EXCLUDE`] noise; [`read`] returns a file's text or refuses a binary /
//! oversized file; [`read_image`] returns an allowlisted image's bytes
//! (ADR-0049). Confinement ([`crate::confine`]) is the security boundary.
//! `.gitignore` is NOT consulted (ADR-0036, amendment 2026-07-26): the operator
//! works in the ignored files — `.ralphy/`, run logs, build output — and hiding
//! what [`read`] would serve anyway was never protection, only confusion.

use std::path::Path;

use crate::confine::{self, ConfineError};

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
}

/// List the one-level children of the confined `rel` directory under `root`,
/// with [`HARD_EXCLUDE`] noise dirs dropped and nothing else filtered — hidden
/// and gitignored entries are listed. Entries are sorted dirs-first, then by
/// name. A confinement failure (escape/missing) propagates as [`ConfineError`].
pub fn list(root: &Path, rel: &str) -> Result<Vec<Entry>, ConfineError> {
    let dir = confine::confine(root, rel)?;

    let mut entries: Vec<Entry> = ignore::WalkBuilder::new(&dir)
        .max_depth(Some(1))
        // Every standard filter off: gitignore/exclude/global and the hidden-file
        // rule. `HARD_EXCLUDE` below is the whole policy.
        .standard_filters(false)
        .filter_entry(|e| {
            e.file_name()
                .to_str()
                .map(|n| !HARD_EXCLUDE.contains(&n))
                .unwrap_or(true)
        })
        .build()
        .filter_map(Result::ok)
        // `max_depth(Some(1))` still yields the root dir itself at depth 0; drop it.
        .filter(|e| e.depth() > 0)
        .map(|e| Entry {
            name: e.file_name().to_string_lossy().into_owned(),
            dir: e.file_type().map(|t| t.is_dir()).unwrap_or(false),
        })
        .collect();

    entries.sort_by(|a, b| b.dir.cmp(&a.dir).then_with(|| a.name.cmp(&b.name)));
    Ok(entries)
}

/// A [`read`] failure that is not a plain confinement escape.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadError {
    /// The file contains a NUL byte or invalid UTF-8 in its first window — the
    /// daemon serves text, so a binary file is refused.
    Binary,
    /// The file exceeds [`MAX_READ_BYTES`].
    TooLarge,
    /// The file does not exist or escapes the root (an out-of-root read is
    /// reported as a plain miss, never leaking whether the target exists).
    NotFound,
    /// [`read_image`] only: the path's extension names no allowlisted image
    /// type, or the bytes do not agree with the type the extension claimed
    /// (ADR-0049 §3).
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

impl std::fmt::Display for ReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReadError::Binary => write!(f, "binary file"),
            ReadError::TooLarge => write!(f, "file too large"),
            ReadError::NotFound => write!(f, "file not found"),
            ReadError::NotImage => write!(f, "not an image"),
        }
    }
}

impl std::error::Error for ReadError {}

/// Hard cap on a single [`read`]; the daemon serves bytes, so one read is bounded.
pub const MAX_READ_BYTES: u64 = 2 * 1024 * 1024;

/// Window scanned for the text/binary heuristic.
const SNIFF_BYTES: usize = 8 * 1024;

/// Read the confined `rel` file under `root` as text. Refuses a binary file
/// (NUL byte or invalid UTF-8 in the first [`SNIFF_BYTES`]) with
/// [`ReadError::Binary`], a file over [`MAX_READ_BYTES`] with
/// [`ReadError::TooLarge`], and a missing/out-of-root target with
/// [`ReadError::NotFound`] (an escape is masked as a miss, never leaking existence).
pub fn read(root: &Path, rel: &str) -> Result<String, ReadError> {
    let path = confine::confine(root, rel).map_err(|_| ReadError::NotFound)?;
    let meta = std::fs::metadata(&path).map_err(|_| ReadError::NotFound)?;
    if meta.len() > MAX_READ_BYTES {
        return Err(ReadError::TooLarge);
    }
    let bytes = std::fs::read(&path).map_err(|_| ReadError::NotFound)?;
    // NUL in the first window is the cheap binary tell. UTF-8 validity is decided
    // by the WHOLE-file check below, NOT the window: a valid UTF-8 file whose
    // 8 KiB boundary splits a multibyte char would false-positive as binary if
    // the window were UTF-8-checked on its own.
    if bytes[..bytes.len().min(SNIFF_BYTES)].contains(&0) {
        return Err(ReadError::Binary);
    }
    String::from_utf8(bytes).map_err(|_| ReadError::Binary)
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
    let mut rest = src.trim_start();
    loop {
        // `<!--` must be tried BEFORE `<!`, which is its prefix.
        let after = if let Some(a) = rest.strip_prefix("<?") {
            match a.find("?>") {
                Some(i) => &a[i + 2..],
                None => return false,
            }
        } else if let Some(a) = rest.strip_prefix("<!--") {
            match a.find("-->") {
                Some(i) => &a[i + 3..],
                None => return false,
            }
        } else if let Some(a) = rest.strip_prefix("<!") {
            match a.find('>') {
                Some(i) => &a[i + 1..],
                None => return false,
            }
        } else {
            // Not a prologue construct: this is the root element (or not an SVG).
            return rest.starts_with("<svg");
        };
        rest = after.trim_start();
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

/// Read the confined `rel` file under `root` as an allowlisted image. The
/// extension picks the candidate [`ImageType`] and the bytes must AGREE with it
/// ([`ImageType::matches`]); a disagreement — or an extension naming no image
/// type at all — is [`ReadError::NotImage`]. Oversize is [`ReadError::TooLarge`]
/// and a missing/out-of-root target is [`ReadError::NotFound`], masking an
/// escape as a plain miss exactly like [`read`].
pub fn read_image(root: &Path, rel: &str) -> Result<Image, ReadError> {
    let ext = Path::new(rel)
        .extension()
        .and_then(|e| e.to_str())
        .ok_or(ReadError::NotImage)?;
    let kind = ImageType::from_extension(ext).ok_or(ReadError::NotImage)?;

    let path = confine::confine(root, rel).map_err(|_| ReadError::NotFound)?;
    let meta = std::fs::metadata(&path).map_err(|_| ReadError::NotFound)?;
    if meta.len() > MAX_IMAGE_BYTES {
        return Err(ReadError::TooLarge);
    }
    let bytes = std::fs::read(&path).map_err(|_| ReadError::NotFound)?;
    if !kind.matches(&bytes) {
        return Err(ReadError::NotImage);
    }
    Ok(Image {
        media_type: kind.media_type(),
        bytes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// The smallest byte string that passes each type's magic check.
    fn magic(kind: ImageType) -> Vec<u8> {
        match kind {
            ImageType::Png => b"\x89PNG\r\n\x1a\n".to_vec(),
            ImageType::Jpeg => b"\xff\xd8\xff\xe0".to_vec(),
            ImageType::Gif => b"GIF89a".to_vec(),
            ImageType::Webp => b"RIFF\x00\x00\x00\x00WEBPVP8 ".to_vec(),
            ImageType::Bmp => b"BM\x00\x00".to_vec(),
            ImageType::Icon => b"\x00\x00\x01\x00".to_vec(),
            ImageType::Svg => b"<svg xmlns=\"http://www.w3.org/2000/svg\"/>".to_vec(),
        }
    }

    #[test]
    fn list_filters_noise() {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("visible.txt"), b"x").unwrap();
        fs::create_dir(root.path().join("node_modules")).unwrap();
        fs::write(root.path().join("node_modules/x"), b"x").unwrap();
        let names: Vec<String> = list(root.path(), "")
            .unwrap()
            .into_iter()
            .map(|e| e.name)
            .collect();
        assert!(names.contains(&"visible.txt".to_string()));
        assert!(!names.contains(&"node_modules".to_string()));
    }

    #[test]
    fn read_refuses_binary() {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("bin.dat"), [0x00, 0x01]).unwrap();
        assert_eq!(read(root.path(), "bin.dat"), Err(ReadError::Binary));
    }

    #[test]
    fn refusal_reasons_are_the_wire_vocabulary() {
        // These four literals are the wire contract the workbench matches on;
        // the first three are shared with `ralphy blob read --format json`'s
        // HEAD side, and `not an image` is `read_image`'s alone (ADR-0049 §4).
        assert_eq!(ReadError::Binary.reason(), "binary");
        assert_eq!(ReadError::TooLarge.reason(), "too large");
        assert_eq!(ReadError::NotFound.reason(), "not found");
        assert_eq!(ReadError::NotImage.reason(), "not an image");
    }

    #[test]
    fn read_refuses_oversized() {
        let root = tempfile::tempdir().unwrap();
        let big = vec![b'a'; (MAX_READ_BYTES + 1) as usize];
        fs::write(root.path().join("big.txt"), &big).unwrap();
        assert_eq!(read(root.path(), "big.txt"), Err(ReadError::TooLarge));
    }

    #[test]
    fn read_returns_text() {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("f.txt"), b"hello world").unwrap();
        assert_eq!(read(root.path(), "f.txt"), Ok("hello world".to_string()));
    }

    #[test]
    fn read_accepts_large_utf8_split_at_window_boundary() {
        // A valid UTF-8 file whose byte at SNIFF_BYTES lands mid-multibyte-char
        // must NOT be misread as binary (the window UTF-8 check regression).
        let root = tempfile::tempdir().unwrap();
        // 'é' is 2 bytes; pad so a char straddles the 8 KiB boundary.
        let mut s = "a".repeat(SNIFF_BYTES - 1);
        s.push('é');
        s.push_str(&"b".repeat(100));
        fs::write(root.path().join("big.txt"), s.as_bytes()).unwrap();
        assert_eq!(read(root.path(), "big.txt"), Ok(s));
    }

    #[test]
    fn read_image_serves_every_allowlisted_type() {
        // The whole allowlist round-trips: extension → verified type → bytes.
        let root = tempfile::tempdir().unwrap();
        for (name, kind) in [
            ("a.png", ImageType::Png),
            ("a.jpg", ImageType::Jpeg),
            ("a.jpeg", ImageType::Jpeg),
            ("a.gif", ImageType::Gif),
            ("a.webp", ImageType::Webp),
            ("a.bmp", ImageType::Bmp),
            ("a.ico", ImageType::Icon),
            ("a.svg", ImageType::Svg),
        ] {
            let bytes = magic(kind);
            fs::write(root.path().join(name), &bytes).unwrap();
            assert_eq!(
                read_image(root.path(), name),
                Ok(Image {
                    media_type: kind.media_type(),
                    bytes
                }),
                "{name}"
            );
        }
    }

    #[test]
    fn read_image_is_case_insensitive_on_the_extension() {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("A.PNG"), magic(ImageType::Png)).unwrap();
        assert_eq!(
            read_image(root.path(), "A.PNG").map(|i| i.media_type),
            Ok("image/png")
        );
    }

    #[test]
    fn read_image_refuses_bytes_that_belie_the_extension() {
        // The core of ADR-0049 §3: the extension proposes, the magic disposes.
        // HTML named `.png` must never be served AS `image/png`.
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("evil.png"), b"<html><script>x</script>").unwrap();
        assert_eq!(
            read_image(root.path(), "evil.png"),
            Err(ReadError::NotImage)
        );
        // ...and the mismatch is symmetric: a real PNG under a `.svg` name.
        fs::write(root.path().join("mislabelled.svg"), magic(ImageType::Png)).unwrap();
        assert_eq!(
            read_image(root.path(), "mislabelled.svg"),
            Err(ReadError::NotImage)
        );
    }

    #[test]
    fn read_image_refuses_a_non_image_extension() {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("f.txt"), b"hello").unwrap();
        assert_eq!(read_image(root.path(), "f.txt"), Err(ReadError::NotImage));
        // A PDF is binary and NOT an image: it stays refused (ADR-0049 §1).
        fs::write(root.path().join("d.pdf"), b"%PDF-1.7").unwrap();
        assert_eq!(read_image(root.path(), "d.pdf"), Err(ReadError::NotImage));
        // No extension at all.
        fs::write(root.path().join("LICENSE"), b"x").unwrap();
        assert_eq!(read_image(root.path(), "LICENSE"), Err(ReadError::NotImage));
    }

    #[test]
    fn read_image_accepts_an_svg_behind_a_prologue() {
        // Declaration, comment and doctype may all precede the root element.
        let root = tempfile::tempdir().unwrap();
        let svg = "<?xml version=\"1.0\"?>\n<!-- drawn by hand -->\n\
                   <!DOCTYPE svg PUBLIC \"-//W3C//DTD SVG 1.1//EN\" \"svg11.dtd\">\n\
                   <svg xmlns=\"http://www.w3.org/2000/svg\"><rect/></svg>";
        fs::write(root.path().join("logo.svg"), svg).unwrap();
        assert_eq!(
            read_image(root.path(), "logo.svg").map(|i| i.media_type),
            Ok("image/svg+xml")
        );
    }

    #[test]
    fn read_image_refuses_html_and_an_unterminated_prologue_as_svg() {
        let root = tempfile::tempdir().unwrap();
        // A document whose root is NOT <svg> — the XSS-shaped case.
        fs::write(root.path().join("page.svg"), b"<html><svg/></html>").unwrap();
        assert_eq!(
            read_image(root.path(), "page.svg"),
            Err(ReadError::NotImage)
        );
        // An unterminated comment must refuse, not scan forever.
        fs::write(root.path().join("open.svg"), b"<!-- <svg/>").unwrap();
        assert_eq!(
            read_image(root.path(), "open.svg"),
            Err(ReadError::NotImage)
        );
        // Invalid UTF-8 can never be an SVG.
        fs::write(root.path().join("bin.svg"), [0xff, 0xfe, 0x00]).unwrap();
        assert_eq!(read_image(root.path(), "bin.svg"), Err(ReadError::NotImage));
    }

    #[test]
    fn read_image_refuses_oversized() {
        let root = tempfile::tempdir().unwrap();
        let mut big = magic(ImageType::Png);
        big.resize((MAX_IMAGE_BYTES + 1) as usize, 0);
        fs::write(root.path().join("big.png"), &big).unwrap();
        assert_eq!(read_image(root.path(), "big.png"), Err(ReadError::TooLarge));
    }

    #[test]
    fn read_image_masks_escape_as_not_found() {
        // Same ADR-0036 §5 masking as `read`: an out-of-root image read is
        // indistinguishable from a plain miss.
        let root = tempfile::tempdir().unwrap();
        assert_eq!(
            read_image(root.path(), "../secret.png"),
            Err(ReadError::NotFound)
        );
        assert_eq!(
            read_image(root.path(), "absent.png"),
            Err(ReadError::NotFound)
        );
    }

    #[test]
    fn read_and_read_image_refuse_each_others_inputs() {
        // The two readers do not overlap (ADR-0049 §1): an image is never a text
        // read, and a text file is never an image read.
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("a.png"), magic(ImageType::Png)).unwrap();
        fs::write(root.path().join("a.txt"), b"hello").unwrap();
        assert_eq!(read(root.path(), "a.png"), Err(ReadError::Binary));
        assert_eq!(read_image(root.path(), "a.txt"), Err(ReadError::NotImage));
    }

    #[test]
    fn read_masks_escape_as_not_found() {
        // Security: an out-of-root read must be indistinguishable from a plain
        // miss — `Escape` collapses to `NotFound`, never leaking existence.
        let root = tempfile::tempdir().unwrap();
        assert_eq!(read(root.path(), "../secret"), Err(ReadError::NotFound));
    }
}
