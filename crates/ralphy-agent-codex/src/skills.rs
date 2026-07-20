//! Materializing ralphy's embedded skills into Codex's discovery path
//! (`.agents/skills/`), additively alongside any skills the user already
//! maintains there.

use std::fs;

use anyhow::{Context, Result};
use include_dir::{include_dir, Dir};

use ralphy_adapter_support::{ensure_gitignore_entries, link_or_copy_dir, remove_path};
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
/// setting (unlike OpenCode's `skills.paths`). `.agents/skills` is therefore a
/// user-owned, shared location we must NOT wipe.
///
/// So the real skill content lives in `.ralphy/skills` (cleared-and-replaced
/// wholesale, like the OpenCode adapter, and kept out of git by `.ralphy/.gitignore`),
/// and only per-skill symlinks are placed into `.agents/skills/<name>` —
/// **additively**, replacing just the subdirectories ralphy owns and leaving sibling
/// (user) skills intact. On Windows, where a symlink needs Developer Mode/admin, the
/// link falls back to copying the skill tree. A merged `.agents/skills/.gitignore`
/// keeps our entries out of the executor's commits without clobbering the user's own.
///
/// Returns the `.agents/skills` path Codex discovers.
pub(crate) fn materialize_codex_skills(ws: &Workspace) -> Result<std::path::PathBuf> {
    // 1. Canonical store: real files under `.ralphy/skills`, fully ralphy-owned, so
    //    clearing and re-extracting wholesale (and `.ralphy/.gitignore = *`) is safe.
    let store = ws.ralphy_dir().join("skills");
    ralphy_adapter_support::materialize_assets(&SKILLS, &store, Some(&ws.ralphy_dir()))?;

    // 2. Expose to Codex's discovery path additively: reuse `.agents/skills` if it
    //    already exists, else create it, and (re)link each ralphy skill into it.
    let skills_dir = ws.repo_root().join(".agents").join("skills");
    fs::create_dir_all(&skills_dir).context("creating .agents/skills")?;

    let mut names: Vec<std::ffi::OsString> = Vec::new();
    for skill in SKILLS.dirs() {
        let name = skill
            .path()
            .file_name()
            .context("embedded skill directory has no name")?
            .to_owned();
        let src = store.join(&name);
        let dest = skills_dir.join(&name);

        // Replace only our own subdir; never touch sibling (user) skills.
        if dest.symlink_metadata().is_ok() {
            remove_path(&dest).with_context(|| format!("clearing stale {}", dest.display()))?;
        }
        link_or_copy_dir(&src, &dest)
            .with_context(|| format!("exposing skill {}", name.to_string_lossy()))?;
        names.push(name);
    }

    // 3. Keep our linked skills out of the executor's commits, preserving any
    //    `.gitignore` the user already maintains in `.agents/skills`.
    ensure_gitignore_entries(&skills_dir.join(".gitignore"), &names)?;

    Ok(skills_dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ADR-0041 D9: the dance lives in `ralphy-adapter-support`, and this adapter
    /// must keep CALLING it rather than growing a second copy. Fragments are
    /// spliced with `concat!` so the assertion cannot match itself.
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

    #[test]
    fn materialize_codex_skills_extracts_required_skills() {
        let base = std::env::temp_dir().join(format!("ralphy-codex-skills-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        let ws = Workspace::new(&base);

        let skills_dir = materialize_codex_skills(&ws).expect("materialize");
        assert_eq!(skills_dir, ws.repo_root().join(".agents").join("skills"));

        // The real skill content lives in the canonical `.ralphy/skills` store.
        let store = ws.ralphy_dir().join("skills");
        assert!(
            store.join("reviewer/SKILL.md").is_file(),
            "reviewer/SKILL.md must land in the .ralphy/skills store"
        );
        assert!(
            ws.ralphy_dir().join(".gitignore").is_file(),
            ".ralphy/.gitignore must be written"
        );

        // Codex's discovery path resolves each skill (through the symlink, or the
        // Windows copy fallback) to the same content.
        assert!(
            skills_dir.join("reviewer/SKILL.md").is_file(),
            "reviewer/SKILL.md must resolve under .agents/skills"
        );
        assert!(
            skills_dir.join("staged-plan/SKILL.md").is_file(),
            "staged-plan/SKILL.md must resolve under .agents/skills"
        );
        assert!(
            skills_dir.join("reviewer/scripts/audit.py").is_file(),
            "reviewer/scripts/audit.py must resolve under .agents/skills"
        );

        // The merged ignore lists our skills without a `*` that would swallow the
        // user's sibling skills in `.agents/skills`.
        let gi = fs::read_to_string(skills_dir.join(".gitignore")).expect("read .gitignore");
        assert!(
            gi.lines().any(|l| l.trim() == "/reviewer"),
            "gitignore: {gi:?}"
        );
        assert!(
            gi.lines().any(|l| l.trim() == "/staged-plan"),
            "gitignore: {gi:?}"
        );
        // The ignore self-ignores, so `.gitignore` is not the lone untracked file
        // that would surface `.agents/` in `git status` and dirty the tree.
        assert!(
            gi.lines().any(|l| l.trim() == "/.gitignore"),
            "gitignore must self-ignore: {gi:?}"
        );

        // Idempotent: a second call re-links cleanly and adds no duplicate entries.
        materialize_codex_skills(&ws).expect("re-materialize");
        assert!(skills_dir.join("reviewer/SKILL.md").is_file());
        let gi2 = fs::read_to_string(skills_dir.join(".gitignore")).expect("read .gitignore");
        assert_eq!(
            gi2.lines().filter(|l| l.trim() == "/reviewer").count(),
            1,
            "re-materialize must not duplicate ignore entries: {gi2:?}"
        );

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn materialize_codex_skills_preserves_user_skills() {
        // The defect this guards: materializing ralphy's skills must NOT wipe a
        // skill the user already keeps in the shared `.agents/skills` location, nor
        // overwrite their `.agents/.gitignore`. Only ralphy's own subdirs are touched.
        let base =
            std::env::temp_dir().join(format!("ralphy-codex-userskill-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        let ws = Workspace::new(&base);

        // A pre-existing user skill and a user-maintained .agents/skills/.gitignore.
        let user_skill = ws.repo_root().join(".agents/skills/my-skill");
        fs::create_dir_all(&user_skill).unwrap();
        fs::write(user_skill.join("SKILL.md"), b"user skill").unwrap();
        let user_gitignore = ws.repo_root().join(".agents/skills/.gitignore");
        fs::write(&user_gitignore, b"my-secret\n").unwrap();

        materialize_codex_skills(&ws).expect("materialize");

        // ralphy's skills landed...
        assert!(ws
            .repo_root()
            .join(".agents/skills/reviewer/SKILL.md")
            .is_file());
        // ...and the user's skill survived untouched.
        assert!(
            user_skill.join("SKILL.md").is_file(),
            "user skill must be preserved"
        );
        // The user's gitignore line is preserved and ours are merged in, not
        // overwritten.
        let gi = fs::read_to_string(&user_gitignore).unwrap();
        assert!(
            gi.lines().any(|l| l.trim() == "my-secret"),
            "gitignore: {gi:?}"
        );
        assert!(
            gi.lines().any(|l| l.trim() == "/reviewer"),
            "gitignore: {gi:?}"
        );

        let _ = fs::remove_dir_all(&base);
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

            let res = serde_yaml_ng::from_str::<serde_yaml_ng::Value>(&frontmatter);
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
