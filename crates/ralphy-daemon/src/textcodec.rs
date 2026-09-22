//! Bytes ⇄ text at the file verbs (ADR-0036, amendment 2026-09-22). The Observe
//! read decodes a file's bytes into a string AND says which encoding it used;
//! the Write byte-op encodes a string back with the encoding the client names,
//! or refuses. The daemon keeps no state about it: the encoding rides the wire
//! both ways and the browser's tab is what remembers it.
//!
//! Detection is a fixed order, never a statistical guess: a BOM decides; a bare
//! UTF-16 is recognised by its NUL pattern; valid UTF-8 is UTF-8; anything else
//! without a NUL is the per-repo fallback single-byte page. Between code pages
//! nobody guesses — that is the operator's setting (`files.encoding`), and a
//! wrong setting is something they can correct where a wrong guess per file is
//! not. Names are the WHATWG labels `encoding_rs` implements — the one set a
//! browser and the daemon both already know.

use std::path::Path;

use encoding_rs::{Encoding, UTF_16BE, UTF_16LE, UTF_8};

/// What a file decodes as when it carries no BOM, is not UTF-16 by shape and
/// is not valid UTF-8: the ANSI code page of the Americas and Western Europe,
/// which is what `Set-Content` and most legacy Windows tooling write.
pub const DEFAULT_FALLBACK: &Encoding = encoding_rs::WINDOWS_1252;

/// Window scanned for the text/binary heuristic and the bare-UTF-16 shape.
pub const SNIFF_BYTES: usize = 8 * 1024;

/// Share of the sniff window's byte pairs that must have a NUL on one side for
/// a BOM-less file to be read as UTF-16. Latin text in UTF-16 has a NUL in
/// every pair; the margin tolerates a few non-Latin code units.
const UTF16_NUL_PAIR_SHARE: f32 = 0.9;

/// A decoded file: its text, the encoding the bytes were read with, and whether
/// they began with that encoding's BOM (so a save can put it back).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decoded {
    pub text: String,
    pub encoding: &'static Encoding,
    pub bom: bool,
}

/// Decode `bytes` as text, or `None` when they are binary. `hint` is a client's
/// explicit "reopen with" and skips detection (a BOM of that same encoding is
/// still stripped and reported); otherwise the order is BOM → bare UTF-16 →
/// UTF-8 → `fallback`. UTF-8 validity is decided by the WHOLE-file check, not
/// the window: a valid UTF-8 file whose window boundary splits a multibyte
/// char would false-positive otherwise. A multibyte `fallback` that fails to
/// decode is reported as binary rather than served with replacement chars.
pub fn decode(
    bytes: Vec<u8>,
    hint: Option<&'static Encoding>,
    fallback: &'static Encoding,
) -> Option<Decoded> {
    if let Some(enc) = hint {
        let (body, bom) = strip_bom_of(&bytes, enc);
        return decode_with(body, enc, bom);
    }
    if let Some((enc, len)) = Encoding::for_bom(&bytes) {
        return decode_with(&bytes[len..], enc, true);
    }
    let window = &bytes[..bytes.len().min(SNIFF_BYTES)];
    if let Some(enc) = bare_utf16_shape(window) {
        return decode_with(&bytes, enc, false);
    }
    if window.contains(&0) {
        return None;
    }
    match String::from_utf8(bytes) {
        Ok(text) => Some(Decoded {
            text,
            encoding: UTF_8,
            bom: false,
        }),
        Err(e) => decode_with(&e.into_bytes(), fallback, false),
    }
}

fn decode_with(body: &[u8], enc: &'static Encoding, bom: bool) -> Option<Decoded> {
    let (text, had_errors) = enc.decode_without_bom_handling(body);
    (!had_errors).then(|| Decoded {
        text: text.into_owned(),
        encoding: enc,
        bom,
    })
}

/// `bytes` minus the BOM of `enc` if it begins with one; whether it did.
fn strip_bom_of<'a>(bytes: &'a [u8], enc: &'static Encoding) -> (&'a [u8], bool) {
    match Encoding::for_bom(bytes) {
        Some((found, len)) if found == enc => (&bytes[len..], true),
        _ => (bytes, false),
    }
}

/// The UTF-16 byte order a BOM-less window reads as, by the shape of its NULs:
/// nearly every pair has a NUL on the same side, no pair has one on the other,
/// and the byte paired with each NUL is printable Latin (or a tab/newline) —
/// a control byte there is a binary file whose zeros happen to alternate, not
/// text. A window shorter than a pair has no shape.
fn bare_utf16_shape(window: &[u8]) -> Option<&'static Encoding> {
    let pairs = window.as_chunks::<2>().0;
    if pairs.is_empty() {
        return None;
    }
    let shaped = |nul: usize, text: usize| {
        let with_nul = pairs.iter().filter(|p| p[nul] == 0).count();
        let share = with_nul as f32 / pairs.len() as f32;
        share >= UTF16_NUL_PAIR_SHARE
            && pairs.iter().all(|p| p[text] != 0)
            && pairs
                .iter()
                .filter(|p| p[nul] == 0)
                .all(|p| p[text] >= 0x20 || matches!(p[text], b'\t' | b'\n' | b'\r' | 0x0C))
    };
    if shaped(1, 0) {
        Some(UTF_16LE)
    } else if shaped(0, 1) {
        Some(UTF_16BE)
    } else {
        None
    }
}

/// Encode `text` with `encoding`, prepending its BOM when `bom` is set (only
/// UTF-8 and UTF-16 have one; the flag is ignored for any other encoding). A
/// char the encoding cannot represent refuses the whole encode with that
/// char's zero-based index — never a `?` or a numeric reference in its place.
/// UTF-16 is hand-rolled: per WHATWG, `encoding_rs`'s UTF-16 encoders emit
/// UTF-8, and a save must give back the bytes the read took.
pub fn encode(text: &str, encoding: &'static Encoding, bom: bool) -> Result<Vec<u8>, usize> {
    let mut out = Vec::with_capacity(text.len() * 2 + 3);
    if encoding == UTF_8 {
        if bom {
            out.extend_from_slice(&[0xEF, 0xBB, 0xBF]);
        }
        out.extend_from_slice(text.as_bytes());
        return Ok(out);
    }
    if encoding == UTF_16LE || encoding == UTF_16BE {
        let le = encoding == UTF_16LE;
        if bom {
            out.extend_from_slice(if le { &[0xFF, 0xFE] } else { &[0xFE, 0xFF] });
        }
        for unit in text.encode_utf16() {
            out.extend_from_slice(&if le {
                unit.to_le_bytes()
            } else {
                unit.to_be_bytes()
            });
        }
        return Ok(out);
    }
    let mut encoder = encoding.new_encoder();
    let cap = encoder
        .max_buffer_length_from_utf8_without_replacement(text.len())
        // `None` only when the worst-case size overflows `usize`, which a string
        // that already fits in memory cannot reach.
        .expect("worst-case encoded length of an in-memory string fits usize");
    out.resize(cap, 0);
    let (result, read, written) =
        encoder.encode_from_utf8_without_replacement(text, &mut out, true);
    match result {
        encoding_rs::EncoderResult::InputEmpty => {
            out.truncate(written);
            Ok(out)
        }
        // `read` counts the unmappable char as consumed (encoding_rs docs), so
        // the offender is the last char before `read`, not the first after it.
        encoding_rs::EncoderResult::Unmappable(_) => Err(text[..read].chars().count() - 1),
        // The buffer was sized by `max_buffer_length_*` for the whole input:
        // running out is a violated invariant of encoding_rs, not a data case.
        encoding_rs::EncoderResult::OutputFull => {
            unreachable!("encoder output sized by max_buffer_length_from_utf8_without_replacement")
        }
    }
}

/// The encoding a WHATWG label names, or `None` for an unknown label and for
/// the `replacement` pseudo-encoding (which decodes everything to U+FFFD and
/// is never a way to read a file).
pub fn label(s: &str) -> Option<&'static Encoding> {
    let enc = Encoding::for_label(s.trim().as_bytes())?;
    (enc != encoding_rs::REPLACEMENT).then_some(enc)
}

/// The repo's fallback encoding: `["files"]["encoding"]` of
/// `<repo_root>/.ralphy/settings.json`, or [`DEFAULT_FALLBACK`]. The schema is
/// `ralphy-core`'s `Settings`, but the daemon may not import the core
/// (ADR-0032 §10), so it reparses the file — the precedent `session::spec`
/// sets. An unknown label is the default plus a warning: a typo must not make
/// every file in the repo binary.
pub fn fallback_for(repo_root: &Path) -> &'static Encoding {
    let path = repo_root.join(".ralphy").join("settings.json");
    let Ok(text) = std::fs::read_to_string(path) else {
        return DEFAULT_FALLBACK;
    };
    let named = serde_json::from_str::<serde_json::Value>(&text)
        .ok()
        .and_then(|v| v.get("files")?.get("encoding")?.as_str().map(str::to_owned));
    match named {
        None => DEFAULT_FALLBACK,
        Some(name) => label(&name).unwrap_or_else(|| {
            tracing::warn!(
                encoding = %name,
                default = DEFAULT_FALLBACK.name(),
                "files.encoding names no known encoding; using the default"
            );
            DEFAULT_FALLBACK
        }),
    }
}

#[cfg(test)]
mod tests;
