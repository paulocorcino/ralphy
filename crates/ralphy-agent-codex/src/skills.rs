//! Materializing ralphy's embedded skills into Codex's discovery path
//! (`.agents/skills/`), additively alongside any skills the user already
//! maintains there.

use anyhow::Result;
use include_dir::{include_dir, Dir};

use ralphy_core::Workspace;

/// The skills subtree, embedded at build time so the binary is self-contained.
/// Codex auto-discovers skills in `.agents/skills/`; we extract this tree there
/// before every plan and execute call so a run never depends on globally-installed
/// skills (mirrors `materialize_plugin` in the Claude adapter).
static SKILLS: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/../../assets/plugin/skills");

/// Materialize the embedded skills into the canonical, ralphy-owned `.ralphy/skills`
/// store, then expose them to Codex by linking each one into `.agents/skills/<name>`.
///
/// Codex offers no way to point at a private skills directory: it only ever scans
/// the conventional `.agents/skills` hierarchy (CWD up to repo root, plus
/// `$HOME/.agents/skills` and `/etc/codex/skills`), and its sole skills config key,
/// `[[skills.config]]`, just toggles a skill on/off — there is no additional-path
/// setting (unlike `OpenCode`'s `skills.paths`). `.agents/skills` is therefore a
/// user-owned, shared location we must NOT wipe.
///
/// So the real skill content lives in `.ralphy/skills` (cleared-and-replaced
/// wholesale, like the `OpenCode` adapter, and kept out of git by `.ralphy/.gitignore`),
/// and only per-skill symlinks are placed into `.agents/skills/<name>` —
/// **additively**, replacing just the subdirectories ralphy owns and leaving sibling
/// (user) skills intact. On Windows, where a symlink needs Developer Mode/admin, the
/// link falls back to copying the skill tree. A merged `.agents/skills/.gitignore`
/// keeps our entries out of the executor's commits without clobbering the user's own.
///
/// Returns the `.agents/skills` path Codex discovers.
pub(crate) fn materialize_codex_skills(ws: &Workspace) -> Result<std::path::PathBuf> {
    let skills_dir = ws.repo_root().join(".agents").join("skills");
    ralphy_adapter_support::expose_skills(&SKILLS, &ws.ralphy_dir(), &skills_dir)?;
    Ok(skills_dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ADR-0041 D9: the dance lives in `ralphy-adapter-support`, and this adapter
    /// must keep CALLING it rather than growing a second copy. Fragments are
    /// spliced with `concat!` so the assertion cannot match itself.
    // pin: the skill link and gitignore code comes from the shared adapter crate —
    // a second copy behaves the same, so no behavior test can tell them apart.
    #[test]
    fn the_dance_is_not_reimplemented_locally() {
        let src = include_str!("skills.rs");
        assert!(
            !src.contains(concat!("fn link_or_copy", "_dir")),
            "link_or_copy_dir must come from ralphy-adapter-support"
        );
        assert!(
            !src.contains(concat!("fn ensure_gitignore", "_entries")),
            "ensure_gitignore_entries must come from ralphy-adapter-support"
        );
    }

    /// The codex part of skill exposure: the discovery folder is `.agents/skills`
    /// and the embedded skills resolve there. The link, store and ignore rules
    /// belong to `expose_skills` in the shared crate.
    #[test]
    fn codex_finds_the_embedded_skills_in_agents_skills() {
        let base = tempfile::tempdir().expect("a temp dir");
        let ws = Workspace::new(base.path());

        let skills_dir = materialize_codex_skills(&ws).expect("materialize");

        assert_eq!(skills_dir, ws.repo_root().join(".agents").join("skills"));
        for file in [
            "reviewer/SKILL.md",
            "staged-plan/SKILL.md",
            "reviewer/scripts/audit.py",
        ] {
            assert!(skills_dir.join(file).is_file(), "{file} must resolve");
        }
    }

    #[test]
    fn embedded_skill_frontmatter_is_valid_yaml() {
        let mut checked = 0usize;
        for skill in SKILLS.dirs() {
            let name = skill
                .path()
                .file_name()
                .expect("embedded skill directory has no name")
                .to_string_lossy()
                .into_owned();
            let skill_md = SKILLS
                .get_file(format!("{name}/SKILL.md"))
                .unwrap_or_else(|| panic!("{name} has no SKILL.md"));
            let contents = skill_md
                .contents_utf8()
                .unwrap_or_else(|| panic!("{name}/SKILL.md is not valid UTF-8"));

            let mut all_lines = contents.lines();
            let first = all_lines
                .next()
                .unwrap_or_else(|| panic!("{name}/SKILL.md is empty"));
            assert_eq!(
                first, "---",
                "{name}/SKILL.md does not start with a '---' frontmatter delimiter"
            );
            let rest: Vec<&str> = all_lines.collect();
            let end = rest
                .iter()
                .position(|l| *l == "---")
                .unwrap_or_else(|| panic!("{name}/SKILL.md has no closing '---' delimiter"));
            let frontmatter = rest[..end].join("\n");

            let res =
                saphyr_parser::Parser::new_from_str(&frontmatter).collect::<Result<Vec<_>, _>>();
            assert!(
                res.is_ok(),
                "{name}/SKILL.md frontmatter is not valid YAML: {:?}",
                res.err()
            );
            checked += 1;
        }
        assert!(
            checked >= 3,
            "expected to check at least 3 skills, checked {checked}"
        );
    }
}
