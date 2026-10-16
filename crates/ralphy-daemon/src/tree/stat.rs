//! The properties of one path: the reader behind the `file.stat` verb
//! (ADR-0036, amendment 2026-10-15). The facts come from the file system and
//! from the rules the tree already uses. What a file IS (text, image, binary,
//! too large) is not decided here: it is the answer of [`super::read_with`] and
//! [`super::read_image`], so the dialog never calls a file text that
//! `file.read` then refuses.

use std::path::Path;
use std::time::UNIX_EPOCH;

use encoding_rs::Encoding;
use serde::Serialize;

use super::{confine, ignored, read_image, read_with, ImageType, ReadError};

/// What the path is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    File,
    Folder,
}

/// What a file is, as the workbench would open it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Class {
    Text,
    Image,
    Binary,
    TooLarge,
}

/// The type of a file: its [`Class`] and what the class adds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FileType {
    pub class: Class,
    /// Text only: the encoding the file was decoded with.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub encoding: Option<&'static str>,
    /// Text only: the file starts with a byte order mark.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bom: Option<bool>,
    /// Image only: the extension the type is written with (`png`, `jpg`…).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image: Option<&'static str>,
}

impl FileType {
    fn of(class: Class) -> FileType {
        FileType {
            class,
            encoding: None,
            bom: None,
            image: None,
        }
    }
}

/// The facts of one path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Stat {
    pub name: String,
    pub kind: Kind,
    /// The logical length in bytes. A file only: a folder is the business of
    /// `tree.size`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    /// Milliseconds since the Unix epoch, when the system keeps a time.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modified: Option<u64>,
    pub readonly: bool,
    /// The repo ignores the path, by the same rules as `tree.list`.
    pub ignored: bool,
    /// A file only.
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub file_type: Option<FileType>,
}

/// The facts of the confined `rel` path under `root`, the checkout root the
/// ignore rules are anchored to. `rel` empty is the root itself. A missing or
/// out-of-root path is [`ReadError::NotFound`], as for every Observe read.
/// `fallback` is the repo's single-byte page ([`crate::textcodec::fallback_for`]).
pub fn stat(root: &Path, rel: &str, fallback: &'static Encoding) -> Result<Stat, ReadError> {
    let base = confine::confine(root, "").map_err(|_| ReadError::NotFound)?;
    let path = confine::confine(root, rel).map_err(|_| ReadError::NotFound)?;
    let meta = std::fs::metadata(&path).map_err(|_| ReadError::NotFound)?;
    let is_dir = meta.is_dir();
    let parts = ignored::components_below(&base, &path);
    let file_type = if is_dir {
        None
    } else {
        Some(classify(root, rel, fallback)?)
    };
    Ok(Stat {
        name: name_of(&base, rel),
        kind: if is_dir { Kind::Folder } else { Kind::File },
        size: (!is_dir).then_some(meta.len()),
        modified: meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .and_then(|d| u64::try_from(d.as_millis()).ok()),
        readonly: meta.permissions().readonly(),
        ignored: ignored_mark(&base, &parts, is_dir),
        file_type,
    })
}

/// The last component of what the operator asked for, not of the canonical
/// path: a link shows its own name. The root has the name of its folder.
fn name_of(base: &Path, rel: &str) -> String {
    let asked = Path::new(rel).file_name().or_else(|| base.file_name());
    asked.map_or_else(String::new, |n| n.to_string_lossy().into_owned())
}

/// Is the entry at `parts` ignored? The root is the checkout and is never
/// ignored. Any other entry is judged by the rules of its parent folder.
fn ignored_mark(base: &Path, parts: &[String], is_dir: bool) -> bool {
    match parts.split_last() {
        None => false,
        Some((_, parent)) => {
            ignored::Rules::for_dir(base, parent).ignored(&parts.join("/"), is_dir)
        }
    }
}

/// The type of the file at `rel`. A file with an image extension is an image
/// only when its bytes agree with the extension: `notes.png` full of HTML is
/// judged as the text it is.
fn classify(root: &Path, rel: &str, fallback: &'static Encoding) -> Result<FileType, ReadError> {
    let claimed = Path::new(rel)
        .extension()
        .and_then(|e| e.to_str())
        .and_then(ImageType::from_extension);
    if let Some(kind) = claimed {
        match read_image(root, rel) {
            Ok(_) => {
                return Ok(FileType {
                    image: Some(kind.extension()),
                    ..FileType::of(Class::Image)
                })
            }
            Err(ReadError::NotImage) => {}
            Err(refusal) => return refusal_type(refusal),
        }
    }
    match read_with(root, rel, None, fallback) {
        Ok(decoded) => Ok(FileType {
            encoding: Some(decoded.encoding.name()),
            bom: Some(decoded.bom),
            ..FileType::of(Class::Text)
        }),
        Err(refusal) => refusal_type(refusal),
    }
}

/// A refused read as the type of the file. A miss stays a miss.
fn refusal_type(refusal: ReadError) -> Result<FileType, ReadError> {
    match refusal {
        ReadError::TooLarge => Ok(FileType::of(Class::TooLarge)),
        ReadError::NotFound => Err(ReadError::NotFound),
        // `read_with` never answers `NotImage`; a file that no reader takes is
        // binary.
        ReadError::Binary | ReadError::NotImage => Ok(FileType::of(Class::Binary)),
    }
}

#[cfg(test)]
mod tests;
