//! Ralphy's skills in the owned root (ADR-0086 D8). With `--gemini_dir`, `agy`
//! reads its global skills from `<root>/skills` and no longer from the
//! operator's `~/.gemini` (spike §8). Ralphy owns the whole root, so the folder
//! is replaced as a whole on each run.

use std::path::Path;

use anyhow::Result;
use include_dir::{include_dir, Dir};

/// The skills, embedded at build time so the binary is self-contained.
static SKILLS: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/../../assets/plugin/skills");

/// Write the embedded skills into `<root>/skills`.
pub(crate) fn materialize(root: &Path) -> Result<()> {
    ralphy_adapter_support::materialize_assets(&SKILLS, &root.join("skills"), None)
}

#[cfg(test)]
mod tests;
