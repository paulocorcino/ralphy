//! Reading and writing `desk.toml`.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use super::DeskStore;

/// Load the desk from `path`. A missing file reads as an empty desk. A file
/// that exists but cannot be read or parsed is an error, never an empty desk:
/// a write over it would destroy the layout it could not read (ADR-0070 D4).
/// The daemon does not read the desk at startup, so this error never stops it.
///
/// A read that fails is tried once more: on Windows an antivirus or an indexer
/// can hold a file that is fine for a moment. [`is_parse_error`] tells a
/// layout that cannot be parsed from a file that cannot be read.
pub fn load_from(path: &Path) -> Result<DeskStore> {
    let read = std::fs::read_to_string(path).or_else(|e| match e.kind() {
        std::io::ErrorKind::NotFound => Err(e),
        _ => std::fs::read_to_string(path),
    });
    match read {
        Ok(text) => {
            toml::from_str(&text).with_context(|| format!("parsing desk layout {}", path.display()))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(DeskStore::default()),
        Err(e) => Err(e).with_context(|| format!("reading desk layout {}", path.display())),
    }
}

/// Whether a [`load_from`] error is a layout that cannot be parsed, as opposed
/// to a file that cannot be read. Only the first is a desk to replace: a read
/// error may pass, and the file behind it may be fine.
pub fn is_parse_error(e: &anyhow::Error) -> bool {
    e.chain()
        .any(|c| c.downcast_ref::<toml::de::Error>().is_some())
}

/// Write the desk to `path` owner-only, creating the parent directory.
///
/// ATOMIC: written to a sibling temp file and renamed over the target, because
/// this is written on every drag, resize and close. A truncated in-place write
/// would read back as an unreadable desk, which refuses every later write until
/// the operator starts a new one.
pub fn save_to(store: &DeskStore, path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    let text = toml::to_string_pretty(store).context("serializing desk layout")?;
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, text).with_context(|| format!("writing {}", tmp.display()))?;
    crate::registry::set_owner_only(&tmp)?;
    std::fs::rename(&tmp, path)
        .with_context(|| format!("replacing {} with {}", path.display(), tmp.display()))?;
    Ok(())
}

/// Rename an unreadable desk aside to `<file name>.unreadable-<today>`, adding
/// `-2`, `-3`… when that name is taken, so no earlier copy is overwritten.
/// Returns the new path. Nothing is deleted (ADR-0070 D4).
pub fn move_aside(path: &Path, today: &str) -> Result<PathBuf> {
    let name = path
        .file_name()
        .with_context(|| format!("{} has no file name", path.display()))?
        .to_string_lossy();
    let base = format!("{name}.unreadable-{today}");
    let mut target = path.with_file_name(&base);
    let mut n = 2;
    // `exists` is false on a metadata error too, and `rename` replaces an
    // existing target on every platform: an error must stop the move.
    while target
        .try_exists()
        .with_context(|| format!("checking {}", target.display()))?
    {
        target = path.with_file_name(format!("{base}-{n}"));
        n += 1;
    }
    std::fs::rename(path, &target)
        .with_context(|| format!("renaming {} to {}", path.display(), target.display()))?;
    Ok(target)
}
