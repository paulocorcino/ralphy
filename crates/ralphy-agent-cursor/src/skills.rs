//! Materializing ralphy's embedded skills into Cursor's repo-local discovery
//! root (`.cursor/skills/`), additively alongside any skills the operator
//! already maintains there (ADR-0042 D12).
//!
//! Unlike Copilot (ADR-0041 D9), Cursor's stream carries no skills-loaded
//! receipt — invocation appears only as a `readToolCall` reading `SKILL.md` off
//! disk on demand (spike §8, P16) — so there is no load-receipt guard here, only
//! the materialization itself and the foreign-harvest warning D12 requires be
//! surfaced, not left to be inferred from usage reports.
//!
//! The link/copy/ignore dance itself lives in [`ralphy_adapter_support`]; only
//! the per-skill loop and the harvest notice are Cursor's own.

use std::fs;

use anyhow::{Context, Result};
use include_dir::{include_dir, Dir};

use ralphy_adapter_support::{ensure_gitignore_entries, link_or_copy_dir, remove_path};
use ralphy_core::Workspace;

/// The skills subtree, embedded at build time so the binary is self-contained.
static SKILLS: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/../../assets/plugin/skills");

/// D12: naming the foreign roots this vendor harvests with no CLI-side
/// allowlist, and the measured cost of a trivial run, so an operator meets the
/// tax in the run log rather than inferring it from a usage report.
pub(crate) const FOREIGN_HARVEST_NOTICE: &str =
    "cursor: this vendor auto-discovers skills recursively under .claude/skills, \
     .codex/skills and their ~/ equivalents with no CLI-side allowlist — a \
     trivial run measured 18 212 input tokens injecting 78 foreign skills. See \
     docs/configuration.md's Cursor section for the full cost and how it is \
     handled.";

/// Materialize the embedded skills into the canonical, ralphy-owned `.ralphy/skills`
/// store, then expose them to Cursor by linking each into `.cursor/skills/<name>`
/// — the repo-local root D12 reads by default, with no flag, env var or manifest.
///
/// `.cursor/skills` is a SHARED, operator-owned directory (rules, `mcp.json`,
/// `worktrees.json` all live under `.cursor/`), so `materialize_assets` (which
/// clears-and-replaces and writes a blanket `*` ignore) points at `.ralphy/skills`
/// only; the shared directory receives per-skill links and a MERGED
/// `.gitignore`, never a wipe.
pub(crate) fn materialize_cursor_skills(ws: &Workspace) -> Result<Vec<String>> {
    let store = ws.ralphy_dir().join("skills");
    ralphy_adapter_support::materialize_assets(&SKILLS, &store, Some(&ws.ralphy_dir()))?;

    let skills_dir = ws.repo_root().join(".cursor").join("skills");
    fs::create_dir_all(&skills_dir).context("creating .cursor/skills")?;

    let mut names: Vec<std::ffi::OsString> = Vec::new();
    for skill in SKILLS.dirs() {
        let name = skill
            .path()
            .file_name()
            .context("embedded skill directory has no name")?
            .to_owned();
        let src = store.join(&name);
        let dest = skills_dir.join(&name);

        // Replace only our own subdir; never touch sibling (operator) skills.
        if dest.symlink_metadata().is_ok() {
            remove_path(&dest).with_context(|| format!("clearing stale {}", dest.display()))?;
        }
        link_or_copy_dir(&src, &dest)
            .with_context(|| format!("exposing skill {}", name.to_string_lossy()))?;
        names.push(name);
    }

    ensure_gitignore_entries(&skills_dir.join(".gitignore"), &names)?;

    tracing::warn!("{}", FOREIGN_HARVEST_NOTICE);

    Ok(names
        .iter()
        .map(|n| n.to_string_lossy().into_owned())
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An isolated parent + repo pair: `materialize_writes_nothing_outside_the_workspace`
    /// walks the parent, and the shared OS temp root can hold thousands of
    /// unrelated entries from other processes — scoping the parent to one fresh
    /// tempdir keeps that walk to what this test itself created.
    fn workspace(tag: &str) -> (tempfile::TempDir, Workspace) {
        let parent = tempfile::Builder::new()
            .prefix(&format!("ralphy-cursor-skills-{tag}-"))
            .tempdir()
            .expect("tempdir");
        let repo = parent.path().join("repo");
        fs::create_dir_all(&repo).unwrap();
        let ws = Workspace::new(&repo);
        (parent, ws)
    }

    #[test]
    fn materialize_lands_every_embedded_skill_in_the_repo_root() {
        let (_dir, ws) = workspace("lands");

        let names = materialize_cursor_skills(&ws).expect("materialize");

        let reviewer_md = ws.repo_root().join(".cursor/skills/reviewer/SKILL.md");
        assert!(reviewer_md.is_file(), "{reviewer_md:?} must exist");
        assert!(
            !fs::read_to_string(&reviewer_md).unwrap().is_empty(),
            "reviewer/SKILL.md must be non-empty"
        );
        assert!(names.contains(&"reviewer".to_string()), "{names:?}");
        assert!(names.contains(&"staged-plan".to_string()), "{names:?}");
    }

    #[test]
    fn materialize_is_idempotent_and_keeps_an_operator_sibling() {
        let (_dir, ws) = workspace("idempotent");

        let sibling = ws.repo_root().join(".cursor/skills/operator-own/SKILL.md");
        fs::create_dir_all(sibling.parent().unwrap()).unwrap();
        fs::write(&sibling, "mine").unwrap();
        let gitignore = ws.repo_root().join(".cursor/skills/.gitignore");
        fs::create_dir_all(gitignore.parent().unwrap()).unwrap();
        fs::write(&gitignore, "my-secret\n").unwrap();

        materialize_cursor_skills(&ws).expect("first pass");
        materialize_cursor_skills(&ws).expect("second pass");

        assert_eq!(fs::read_to_string(&sibling).unwrap(), "mine");
        let gi = fs::read_to_string(&gitignore).unwrap();
        assert!(gi.lines().any(|l| l.trim() == "my-secret"), "{gi:?}");
        assert_eq!(
            gi.lines().filter(|l| l.trim() == "/reviewer").count(),
            1,
            "{gi:?}"
        );
    }

    #[test]
    fn materialize_writes_nothing_outside_the_workspace() {
        let (dir, ws) = workspace("scoped");
        let parent = dir.path().to_path_buf();

        fn listing(root: &std::path::Path) -> Vec<std::path::PathBuf> {
            let mut out = Vec::new();
            let mut stack = vec![root.to_path_buf()];
            while let Some(d) = stack.pop() {
                let Ok(entries) = fs::read_dir(&d) else {
                    continue;
                };
                for e in entries.flatten() {
                    let p = e.path();
                    if p.is_dir() {
                        stack.push(p.clone());
                    }
                    out.push(p);
                }
            }
            out.sort();
            out
        }

        let before = listing(&parent);
        materialize_cursor_skills(&ws).expect("materialize");
        let after = listing(&parent);

        for path in after {
            if before.contains(&path) {
                continue;
            }
            assert!(
                path.starts_with(ws.repo_root()),
                "materialize wrote outside the workspace: {path:?}"
            );
        }

        let needle = concat!("home", "_dir");
        assert!(
            !include_str!("skills.rs").contains(needle),
            "the user-level root must be reachable only through link_or_copy_dir"
        );
    }

    #[test]
    fn the_skills_root_needs_no_flag_env_var_or_manifest() {
        let unpinned = crate::command::build_cursor_command(
            "s",
            None,
            std::path::Path::new("/repo"),
            std::path::Path::new("/run/cfg"),
        );
        let args: Vec<String> = unpinned
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert!(
            !args.iter().any(|a| a == "--plugin-dir"),
            "argv must not depend on a plugin manifest: {args:?}"
        );

        let src = include_str!("skills.rs");
        assert!(
            !src.contains(concat!("env", "::var")),
            "materialization must not read an env var"
        );
        assert!(
            !src.contains(concat!(".cursor-", "plugin")),
            "materialization must not depend on a plugin manifest"
        );
    }

    /// Cross-path invariant (ADR-0042 D12): both `plan()` and `execute()` must
    /// materialize BEFORE spawning the child, on both the success and error
    /// paths — a refused D6 run must not have already written into the
    /// operator's repo.
    #[test]
    fn skills_are_materialized_on_both_phases() {
        let src = include_str!("lib.rs");
        let call = concat!("materialize_cursor", "_skills(ws)?");
        assert_eq!(
            src.matches(call).count(),
            2,
            "materialize_cursor_skills(ws)? must be called once in plan() and once in execute()"
        );

        let plan_offset = src.find(call).expect("first call site");
        let plan_spawn = src.find("run_plan_session(").expect("plan spawn site");
        assert!(
            plan_offset < plan_spawn,
            "materialization must precede the plan spawn"
        );

        let exec_offset = src.rfind(call).expect("second call site");
        let exec_spawn = src.rfind("run_exec_session(").expect("execute spawn site");
        assert!(
            exec_offset < exec_spawn,
            "materialization must precede the execute spawn"
        );
    }

    #[test]
    fn the_harvest_notice_names_the_foreign_roots_and_the_measured_cost() {
        assert!(FOREIGN_HARVEST_NOTICE.contains(".claude/skills"));
        assert!(FOREIGN_HARVEST_NOTICE.contains("18 212"));
        assert!(FOREIGN_HARVEST_NOTICE.contains("docs/configuration.md"));
    }
}
