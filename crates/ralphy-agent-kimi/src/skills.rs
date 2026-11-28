//! Materializing ralphy's embedded skills into a ralphy-owned store that Kimi is
//! pointed at via `--skills-dir`.
//!
//! Unlike Codex (which only scans the conventional `.agents/skills` hierarchy and
//! needs a symlink/`.gitignore` dance), Kimi accepts a `--skills-dir <path>` flag
//! (spike §2), so the store lives entirely under `.ralphy/skills` — kept out of
//! git by the `.ralphy/.gitignore = *` `materialize_assets` writes.

use std::path::PathBuf;

use anyhow::Result;
use ralphy_adapter_support::SKILLS;

use ralphy_core::Workspace;

/// Materialize the embedded skills into the ralphy-owned `.ralphy/skills` store
/// (cleared-and-replaced wholesale) and return its path for `--skills-dir`.
pub(crate) fn materialize_kimi_skills(ws: &Workspace) -> Result<PathBuf> {
    let store = ws.ralphy_dir().join("skills");
    ralphy_adapter_support::materialize_assets(&SKILLS, &store, Some(&ws.ralphy_dir()))?;
    Ok(store)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cleanup::remove_scratch;
    use std::fs;

    #[test]
    fn materialize_kimi_skills_extracts_reviewer() {
        let base = std::env::temp_dir().join(format!("ralphy-kimi-skills-{}", std::process::id()));
        remove_scratch(&base);
        fs::create_dir_all(&base).unwrap();
        let ws = Workspace::new(&base);

        let store = materialize_kimi_skills(&ws).expect("materialize");
        assert_eq!(store, ws.ralphy_dir().join("skills"));
        assert!(
            store.join("reviewer/SKILL.md").is_file(),
            "reviewer/SKILL.md must land in the .ralphy/skills store"
        );
        assert!(
            store.join("staged-plan/SKILL.md").is_file(),
            "staged-plan/SKILL.md must land in the .ralphy/skills store"
        );
        assert!(
            ws.ralphy_dir().join(".gitignore").is_file(),
            ".ralphy/.gitignore must be written"
        );

        remove_scratch(&base);
    }
}
