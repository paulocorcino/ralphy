//! Skills materialization (ADR-0005 D7): extracting the embedded skills tree to
//! `<repo>/.ralphy/skills` and building the `OPENCODE_CONFIG_CONTENT` JSON that
//! points OpenCode's `skills.paths` at it.

use std::path::PathBuf;

use anyhow::Result;
use include_dir::{include_dir, Dir};

use ralphy_core::Workspace;

/// The skills subtree, embedded at build time so the binary is self-contained.
/// OpenCode discovers skills via `skills.paths` in its config; we extract this
/// tree to `.ralphy/skills` and inject the path via `OPENCODE_CONFIG_CONTENT`
/// before every plan and execute call (ADR-0005 D7, mirrors Codex adapter).
static SKILLS: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/../../assets/plugin/skills");

/// Materialize the embedded skills into `<repo>/.ralphy/skills/` so OpenCode can
/// discover them via the injected `skills.paths` config. Clears any prior copy,
/// re-extracts fresh, and writes `<repo>/.ralphy/.gitignore` (`*`) to keep the
/// materialized tree out of executor commits. Returns the `.ralphy/skills` path.
pub(crate) fn materialize_opencode_skills(ws: &Workspace) -> Result<PathBuf> {
    let ralphy_dir = ws.ralphy_dir();
    let skills_dir = ralphy_dir.join("skills");
    ralphy_adapter_support::materialize_assets(&SKILLS, &skills_dir, Some(&ralphy_dir))?;
    Ok(skills_dir)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    // ── materialize_opencode_skills ────────────────────────────────────────

    #[test]
    fn materialize_opencode_skills_extracts_required_skills() {
        let base =
            std::env::temp_dir().join(format!("ralphy-opencode-skills-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        let ws = Workspace::new(&base);

        let skills_dir = materialize_opencode_skills(&ws).expect("materialize");
        assert_eq!(skills_dir, ws.ralphy_dir().join("skills"));
        assert!(
            skills_dir.join("reviewer/SKILL.md").is_file(),
            "reviewer/SKILL.md must be materialized"
        );
        assert!(
            skills_dir.join("staged-plan/SKILL.md").is_file(),
            "staged-plan/SKILL.md must be materialized"
        );
        assert!(
            skills_dir.join("reviewer/scripts/audit.py").is_file(),
            "reviewer/scripts/audit.py must be materialized"
        );
        assert!(
            ws.ralphy_dir().join(".gitignore").is_file(),
            ".ralphy/.gitignore must be written"
        );

        // Idempotent: a second call clears and re-extracts cleanly.
        materialize_opencode_skills(&ws).expect("re-materialize");
        assert!(skills_dir.join("reviewer/SKILL.md").is_file());

        let _ = fs::remove_dir_all(&base);
    }
}
