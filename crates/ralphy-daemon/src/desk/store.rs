//! Reading and writing `desk.toml`.

use std::path::Path;

use anyhow::{Context, Result};

use super::DeskStore;

/// Load the desk from `path`. A missing file reads as an empty desk. A file
/// that exists but cannot be read or parsed is an error, never an empty desk:
/// a write over it would destroy the layout it could not read (ADR-0070 D4).
/// The daemon does not read the desk at startup, so this error never stops it.
pub fn load_from(path: &Path) -> Result<DeskStore> {
    match std::fs::read_to_string(path) {
        Ok(text) => {
            toml::from_str(&text).with_context(|| format!("parsing desk layout {}", path.display()))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(DeskStore::default()),
        Err(e) => Err(e).with_context(|| format!("reading desk layout {}", path.display())),
    }
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
