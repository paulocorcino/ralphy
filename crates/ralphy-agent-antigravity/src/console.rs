//! The owned root for a workbench console. The daemon may not import this crate
//! (ADR-0032 §10), so it runs `ralphy antigravity prepare-root`, which lands here
//! and writes the same root a run writes.

use std::path::PathBuf;

use anyhow::Result;

/// Prepare the owned root of this OS user and return its absolute path, the
/// value a console gives `agy` as `--gemini_dir`.
pub fn prepare_console_root() -> Result<PathBuf> {
    let root = ralphy_proc_util::antigravity::owned_root()?;
    crate::prepare_root(&root)?;
    Ok(root)
}
