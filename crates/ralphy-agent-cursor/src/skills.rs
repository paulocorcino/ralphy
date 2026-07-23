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

/// The capstone-measured harvest floor (ralphy#251, ADR-0042 validation Phase 4):
/// the input tokens the Cursor CLI injects on EACH invocation by auto-discovering
/// 78 foreign skills. Single source of truth for both the operator notice below
/// and the read-time harvest-tax estimate (issue #270) — so the two cannot drift.
/// This is the per-invocation harvest floor, NOT the `18 212` trivial-run *total*
/// (which folds in the run's own tiny input); the estimate multiplies this by the
/// invocation count, so it must exclude non-harvest input.
pub const CURSOR_HARVEST_FLOOR_TOKENS: u64 = 15_679;

/// D12: naming the foreign roots this vendor harvests with no CLI-side allowlist,
/// and the measured per-invocation cost, so an operator meets the tax in the run
/// log rather than inferring it from a usage report. Built from
/// [`CURSOR_HARVEST_FLOOR_TOKENS`] so the notice and the #270 estimate share one
/// number.
pub(crate) fn foreign_harvest_notice() -> String {
    format!(
        "cursor: this vendor auto-discovers skills recursively under .claude/skills, \
         .codex/skills and their ~/ equivalents with no CLI-side allowlist — a measured \
         ~{} input tokens per invocation injecting 78 foreign skills. See \
         docs/configuration.md's Cursor section for the full cost and how it is handled.",
        fmt_thousands(CURSOR_HARVEST_FLOOR_TOKENS)
    )
}

/// Group digits with an ASCII space (`15679` → `15 679`), matching the separator
/// the D12 notice has always used. ASCII space only, to keep the string
/// byte-stable across platforms (the drift test asserts on this form).
fn fmt_thousands(n: u64) -> String {
    let digits = n.to_string();
    let bytes = digits.as_bytes();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, b) in bytes.iter().enumerate() {
        if i > 0 && (bytes.len() - i).is_multiple_of(3) {
            out.push(' ');
        }
        out.push(*b as char);
    }
    out
}

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

    tracing::warn!("{}", foreign_harvest_notice());

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
    }

    /// A source-text grep for the literal "home_dir" cannot catch a regression
    /// that reaches the user-level root through a different call shape, and it
    /// only ever checked `skills.rs`'s own text — not the actual location a
    /// leak would land. This observes the REAL location instead: snapshot
    /// `<home>/.cursor/skills` before and after, on this machine, where a
    /// regression calling `ralphy_adapter_support::home_dir()` (already used
    /// elsewhere in this crate, `command.rs`'s config-dir seeding) would
    /// actually write.
    #[test]
    fn materialize_never_writes_under_the_real_home_directory() {
        let (_dir, ws) = workspace("home-safety");
        let Some(home) = ralphy_adapter_support::home_dir() else {
            return; // no HOME/USERPROFILE resolvable in this environment
        };
        let home_skills = home.join(".cursor").join("skills");

        fn snapshot(dir: &std::path::Path) -> Option<Vec<std::ffi::OsString>> {
            let mut names: Vec<_> = fs::read_dir(dir)
                .ok()?
                .flatten()
                .map(|e| e.file_name())
                .collect();
            names.sort();
            Some(names)
        }

        let before = snapshot(&home_skills);
        materialize_cursor_skills(&ws).expect("materialize");
        let after = snapshot(&home_skills);

        assert_eq!(
            before, after,
            "materialize_cursor_skills must never write under the operator's real \
             home-level skills root: {home_skills:?}"
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
    /// paths — the D6 gate runs first, so a run it stops must not have already
    /// materialized skills into the operator's repo.
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
        let notice = foreign_harvest_notice();
        assert!(notice.contains(".claude/skills"));
        // The notice cites the same measured floor the #270 estimate multiplies,
        // so the two surfaces can never drift.
        assert!(notice.contains(&fmt_thousands(CURSOR_HARVEST_FLOOR_TOKENS)));
        assert!(notice.contains("15 679"));
        assert!(notice.contains("docs/configuration.md"));
    }

    #[test]
    fn fmt_thousands_groups_with_ascii_space() {
        assert_eq!(fmt_thousands(15_679), "15 679");
        assert_eq!(fmt_thousands(235_185), "235 185");
        assert_eq!(fmt_thousands(999), "999");
        assert_eq!(fmt_thousands(1_000), "1 000");
    }
}
