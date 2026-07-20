//! Materializing ralphy's embedded skills into Copilot's discovery path
//! (`.agents/skills/`), additively alongside any skills the operator already
//! maintains there — plus the D9 load receipt that proves Copilot actually read
//! them (ADR-0041 D9).
//!
//! The link/copy/ignore dance itself lives in [`ralphy_adapter_support`]; only
//! the per-skill loop and the receipt guard are Copilot's own.

use std::fs;

use anyhow::{Context, Result};
use include_dir::{include_dir, Dir};

use ralphy_adapter_support::{ensure_gitignore_entries, link_or_copy_dir, remove_path};
use ralphy_core::Workspace;

/// The skills subtree, embedded at build time so the binary is self-contained.
static SKILLS: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/../../assets/plugin/skills");

/// Materialize the embedded skills into the canonical, ralphy-owned `.ralphy/skills`
/// store, then expose them to Copilot by linking each into `.agents/skills/<name>`.
///
/// `.agents/skills` is a SHARED, operator-owned directory, so `materialize_assets`
/// (which clears-and-replaces and writes a blanket `*` ignore) points at
/// `.ralphy/skills` only; the shared directory receives per-skill links and a
/// MERGED `.gitignore`, never a wipe.
///
/// Returns the exposed skill names, which the caller feeds to
/// [`skills_load_violation`] as the required set for the D9 receipt.
pub(crate) fn materialize_copilot_skills(ws: &Workspace) -> Result<Vec<String>> {
    let store = ws.ralphy_dir().join("skills");
    ralphy_adapter_support::materialize_assets(&SKILLS, &store, Some(&ws.ralphy_dir()))?;

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

        // Replace only our own subdir; never touch sibling (operator) skills.
        if dest.symlink_metadata().is_ok() {
            remove_path(&dest).with_context(|| format!("clearing stale {}", dest.display()))?;
        }
        link_or_copy_dir(&src, &dest)
            .with_context(|| format!("exposing skill {}", name.to_string_lossy()))?;
        names.push(name);
    }

    ensure_gitignore_entries(&skills_dir.join(".gitignore"), &names)?;

    Ok(names
        .iter()
        .map(|n| n.to_string_lossy().into_owned())
        .collect())
}

/// Scan a Copilot JSONL stream for the `session.skills_loaded` receipt and assert
/// every name in `required` was loaded. `None` means the receipt was seen and all
/// of ralphy's skills are there; `Some(msg)` is a run-failing violation.
///
/// Live shape (`copilot 1.0.71`, 2026-07-20): `data.skills[]`, each entry keyed
/// `name`. Copilot injects its OWN skills into the same array, so this checks
/// PRESENCE of each required name, never set equality.
///
/// No `ephemeral` filter, for the same reason as `guards::builtin_mcp_violation`:
/// the live receipt carries `"ephemeral":true`, so filtering would find nothing.
///
/// `require_receipt` mirrors D7's split exactly. A MISSING required skill is
/// always a violation. An ABSENT receipt is only one for a run that reached normal
/// completion: a run killed by a usage limit, a crash or the wall clock can die
/// before the receipt is emitted, and fail-closing there would overwrite the typed
/// `Limit`/`Timeout` outcome with "skills receipt missing".
pub(crate) fn skills_load_violation(
    stdout: &str,
    required: &[String],
    require_receipt: bool,
) -> Option<String> {
    let mut saw_receipt = false;
    let mut loaded: Vec<String> = Vec::new();
    for line in stdout.lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if v.get("type").and_then(|t| t.as_str()) != Some("session.skills_loaded") {
            continue;
        }
        // A receipt counts as SEEN only once its payload is readable: a renamed or
        // missing `data.skills` is vendor drift, and treating it as a pass would
        // report green on a run that silently lost every skill.
        let Some(skills) = v
            .get("data")
            .and_then(|d| d.get("skills"))
            .and_then(|s| s.as_array())
        else {
            continue;
        };
        saw_receipt = true;
        for skill in skills {
            if let Some(name) = skill.get("name").and_then(|n| n.as_str()) {
                loaded.push(name.to_string());
            }
        }
    }

    if saw_receipt {
        if let Some(missing) = required.iter().find(|r| !loaded.contains(r)) {
            return Some(format!(
                "Copilot loaded no `{missing}` skill: ralphy materialized it into \
                 .agents/skills but the session.skills_loaded receipt lists only \
                 [{}] — the charter's skill invocations will silently do nothing \
                 (ADR-0041 D9)",
                loaded.join(", ")
            ));
        }
        return None;
    }

    if require_receipt {
        return Some(
            "no session.skills_loaded receipt in the Copilot stream — ralphy's \
             skills are unverifiable, failing closed (ADR-0041 D9)"
                .into(),
        );
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("../fixtures/skills-loaded-2026-07-20.jsonl");

    fn required() -> Vec<String> {
        ["reviewer", "setup-pocock", "staged-plan"]
            .iter()
            .map(|s| s.to_string())
            .collect()
    }

    #[test]
    fn materialize_copilot_skills_extracts_required_skills() {
        let base =
            std::env::temp_dir().join(format!("ralphy-copilot-skills-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        let ws = Workspace::new(&base);

        let names = materialize_copilot_skills(&ws).expect("materialize");

        // Real content in the canonical, ralphy-owned store...
        assert!(
            ws.ralphy_dir().join("skills/reviewer/SKILL.md").is_file(),
            "reviewer/SKILL.md must land in the .ralphy/skills store"
        );
        // ...and resolving through Copilot's discovery path.
        assert!(
            ws.repo_root()
                .join(".agents/skills/reviewer/SKILL.md")
                .is_file(),
            "reviewer/SKILL.md must resolve under .agents/skills"
        );
        assert!(
            names.contains(&"staged-plan".to_string()),
            "the returned required set must name staged-plan: {names:?}"
        );

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn materialize_copilot_skills_preserves_user_skills() {
        // The defect this guards: `.agents/skills` is shared with the operator, so
        // pointing `materialize_assets`'s clear-and-replace at it would wipe their
        // skills and clobber their ignore. Reds if that ever changes.
        let base =
            std::env::temp_dir().join(format!("ralphy-copilot-userskill-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        let ws = Workspace::new(&base);

        let user_skill = ws.repo_root().join(".agents/skills/my-skill");
        fs::create_dir_all(&user_skill).unwrap();
        fs::write(user_skill.join("SKILL.md"), b"user skill").unwrap();
        let user_gitignore = ws.repo_root().join(".agents/skills/.gitignore");
        fs::write(&user_gitignore, b"my-secret\n").unwrap();

        materialize_copilot_skills(&ws).expect("materialize");

        assert!(ws
            .repo_root()
            .join(".agents/skills/reviewer/SKILL.md")
            .is_file());
        assert!(
            user_skill.join("SKILL.md").is_file(),
            "the operator's skill must be preserved"
        );
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

    /// The machine oracle for "the tree is clean afterwards": materializing must
    /// leave `git status --porcelain` empty, or the next run's clean-tree check
    /// aborts.
    #[test]
    fn materialize_copilot_skills_leaves_a_clean_git_tree() {
        let git = |args: &[&str], cwd: &std::path::Path| {
            std::process::Command::new("git")
                .args(args)
                .current_dir(cwd)
                .output()
        };
        if std::process::Command::new("git")
            .arg("--version")
            .output()
            .is_err()
        {
            tracing::warn!("git not available; skipping the clean-tree oracle");
            return;
        }

        let base =
            std::env::temp_dir().join(format!("ralphy-copilot-clean-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();

        git(&["init"], &base).expect("git init");
        git(
            &[
                "-c",
                "user.email=t@t",
                "-c",
                "user.name=t",
                "commit",
                "--allow-empty",
                "-m",
                "base",
            ],
            &base,
        )
        .expect("git commit");

        let ws = Workspace::new(&base);
        materialize_copilot_skills(&ws).expect("materialize");

        let out = git(&["status", "--porcelain"], &base).expect("git status");
        // Without this the oracle passes VACUOUSLY: a failed `git status` also
        // yields empty stdout, which would satisfy the assertion below.
        assert!(
            out.status.success(),
            "git status failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let porcelain = String::from_utf8(out.stdout).unwrap();
        assert_eq!(
            porcelain, "",
            "materializing must leave a clean tree, got: {porcelain:?}"
        );

        let _ = fs::remove_dir_all(&base);
    }

    /// The load-bearing invariant behind the whole D9 guard: `required` is built
    /// from embedded DIRECTORY names, but Copilot reports each skill by its
    /// SKILL.md frontmatter `name`. They agree today; nothing in the type system
    /// binds them, so a fourth skill whose frontmatter name differs from its
    /// directory would fail EVERY real run while the suite stayed green. This is
    /// that check, in the gate, where it reds when reality diverges.
    #[test]
    fn every_embedded_skill_directory_matches_its_frontmatter_name() {
        let mut checked = 0usize;
        for skill in SKILLS.dirs() {
            let dir_name = skill
                .path()
                .file_name()
                .expect("embedded skill directory has no name")
                .to_string_lossy()
                .into_owned();
            let md = SKILLS
                .get_file(format!("{dir_name}/SKILL.md"))
                .unwrap_or_else(|| panic!("{dir_name} has no SKILL.md"))
                .contents_utf8()
                .unwrap_or_else(|| panic!("{dir_name}/SKILL.md is not valid UTF-8"));
            // Frontmatter only: stop at the closing delimiter so a `name:` in the
            // body cannot satisfy this.
            let front = md
                .lines()
                .skip(1)
                .take_while(|l| *l != "---")
                .find_map(|l| l.strip_prefix("name:"))
                .unwrap_or_else(|| panic!("{dir_name}/SKILL.md frontmatter has no `name:`"))
                .trim()
                .to_string();
            assert_eq!(
                front, dir_name,
                "skill directory `{dir_name}` declares frontmatter name `{front}`; the D9 \
                 required set uses directory names, so this would fail every real run"
            );
            checked += 1;
        }
        assert!(checked >= 3, "expected >= 3 skills, checked {checked}");
    }

    /// A required name must match a loaded name EXACTLY. Without this, rewriting
    /// the check as a substring scan passes every other test while
    /// `staged-plan-legacy` silently satisfies the `staged-plan` requirement.
    #[test]
    fn a_similarly_named_skill_does_not_satisfy_the_requirement() {
        let stream =
            r#"{"type":"session.skills_loaded","data":{"skills":[{"name":"staged-plan-legacy"}]}}"#;
        let req = vec!["staged-plan".to_string()];
        let msg = skills_load_violation(stream, &req, true)
            .expect("a near-miss name must not satisfy the requirement");
        assert!(msg.contains("staged-plan"), "{msg}");
    }

    /// `require_receipt` gates ONLY the absent-receipt case. A receipt that IS
    /// present and is missing a required skill fails even for a run that died
    /// early — otherwise an implementation that early-returns `None` whenever
    /// `require_receipt` is false would pass the whole suite.
    #[test]
    fn a_present_receipt_missing_a_skill_fails_even_on_a_run_that_died_early() {
        let stream = r#"{"type":"session.skills_loaded","data":{"skills":[{"name":"reviewer"}]}}"#;
        let msg = skills_load_violation(stream, &required(), false)
            .expect("a present receipt missing a skill is always a violation");
        assert!(
            msg.contains("setup-pocock") || msg.contains("staged-plan"),
            "{msg}"
        );
    }

    #[test]
    fn skills_receipt_lists_the_ralphy_skills_passes() {
        assert_eq!(skills_load_violation(FIXTURE, &required(), true), None);
    }

    /// The FAILS-before / PASSES-after oracle for the whole slice: drop
    /// `staged-plan` from the live receipt and the guard must name it.
    #[test]
    fn skills_receipt_missing_ralphy_skill_fails() {
        let mut v: serde_json::Value = serde_json::from_str(FIXTURE.trim()).unwrap();
        let skills = v["data"]["skills"].as_array().unwrap().clone();
        v["data"]["skills"] = serde_json::Value::Array(
            skills
                .into_iter()
                .filter(|s| s["name"] != "staged-plan")
                .collect(),
        );
        let stream = serde_json::to_string(&v).unwrap();

        let msg = skills_load_violation(&stream, &required(), true)
            .expect("a missing ralphy skill must fail the run");
        assert!(msg.contains("staged-plan"), "{msg}");
    }

    /// A receipt whose payload ralphy cannot read is not a receipt: vendor drift in
    /// `data.skills` must not silently count as "all skills loaded".
    #[test]
    fn skills_receipt_with_unreadable_payload_fails_closed() {
        let drifted = r#"{"type":"session.skills_loaded","data":{"items":[]}}"#;
        assert!(
            skills_load_violation(drifted, &required(), true).is_some(),
            "an unreadable receipt payload must fail closed"
        );
    }

    /// D7's MEDIUM-1 fix, applied to D9: a run that died before emitting the
    /// receipt must not be turned into "skills receipt missing" — that would
    /// overwrite the typed Limit/Timeout outcome with a wrong error.
    #[test]
    fn absent_skills_receipt_is_not_a_violation_for_a_run_that_died_early() {
        assert_eq!(
            skills_load_violation("error: usage limit reached\n", &required(), false),
            None
        );
    }

    /// The live receipt is ephemeral; an ephemeral filter would fail closed on
    /// every real run. Keeps that trap pinned if the fixture is regenerated.
    #[test]
    fn skills_receipt_is_read_from_ephemeral_records() {
        assert!(
            FIXTURE.contains(r#""ephemeral":true"#),
            "the live receipt is ephemeral; the guard must not filter on it"
        );
    }
}
