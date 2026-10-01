//! Materializing ralphy's embedded skills into Copilot's discovery path
//! (`.agents/skills/`), additively alongside any skills the operator already
//! maintains there — plus the D9 checks that prove Copilot finds them: the skill
//! listing before the session, and the load receipt when the CLI emits one
//! (ADR-0041 D9).
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
/// [`skill_list_violation`] and [`skills_load_violation`] as the required set.
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

/// Check the output of `copilot skill list --json` (run in the repo before the
/// session): every name in `required` must be listed and `enabled`. `None` is a
/// pass; `Some(msg)` stops the run before a billed turn. Output that is not the
/// expected array is a violation: an unreadable listing proves nothing.
///
/// Live shape (`copilot` 1.0.75 and 1.0.90, 2026-10-01): a JSON array of
/// `{name, description, source, path, enabled}`. Copilot lists its own builtin
/// skills too, so this checks PRESENCE of each required name, never set equality.
pub(crate) fn skill_list_violation(listing: &str, required: &[String]) -> Option<String> {
    let Ok(serde_json::Value::Array(entries)) = serde_json::from_str(listing.trim()) else {
        return Some(
            "Copilot's skill list could not be read, so Ralphy cannot confirm that \
             its skills are there; stopping to be safe"
                .into(),
        );
    };
    let enabled: Vec<&str> = entries
        .iter()
        .filter(|e| e.get("enabled").and_then(|b| b.as_bool()) == Some(true))
        .filter_map(|e| e.get("name").and_then(|n| n.as_str()))
        .collect();
    let missing = required.iter().find(|r| !enabled.contains(&r.as_str()))?;
    Some(format!(
        "Copilot does not offer the `{missing}` skill: Ralphy put it in \
         .agents/skills, but Copilot lists only [{}] as enabled, so the steps that \
         use this skill would do nothing",
        enabled.join(", ")
    ))
}

/// Scan a Copilot JSONL stream for the `session.skills_loaded` receipt and assert
/// every name in `required` was loaded. `None` means no receipt, or a receipt
/// that lists all of ralphy's skills; `Some(msg)` is a run-failing violation.
///
/// Live shape (`copilot 1.0.71`, 2026-07-20): `data.skills[]`, each entry keyed
/// `name`. Copilot injects its OWN skills into the same array, so this checks
/// PRESENCE of each required name, never set equality.
///
/// No `ephemeral` filter, for the same reason as `guards::builtin_mcp_violation`:
/// the live receipt carries `"ephemeral":true`, so filtering would find nothing.
///
/// An ABSENT receipt is not a violation: CLI 1.0.90 no longer emits it, and the
/// skill listing before the session ([`skill_list_violation`]) is the proof that
/// the skills are there (ADR-0041 D9 amendment of 2026-10-01).
pub(crate) fn skills_load_violation(stdout: &str, required: &[String]) -> Option<String> {
    for line in stdout.lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if v.get("type").and_then(|t| t.as_str()) != Some("session.skills_loaded") {
            continue;
        }
        // A receipt with an unreadable payload is skipped, not passed: the
        // listing before the session is the proof either way.
        let Some(skills) = v
            .get("data")
            .and_then(|d| d.get("skills"))
            .and_then(|s| s.as_array())
        else {
            continue;
        };
        let loaded: Vec<&str> = skills
            .iter()
            .filter_map(|skill| skill.get("name").and_then(|n| n.as_str()))
            .collect();
        if let Some(missing) = required.iter().find(|r| !loaded.contains(&r.as_str())) {
            return Some(format!(
                "Copilot did not load the `{missing}` skill: Ralphy put it in \
                 .agents/skills, but Copilot reports only [{}], so the steps that \
                 use this skill would do nothing",
                loaded.join(", ")
            ));
        }
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
        let msg = skills_load_violation(stream, &req)
            .expect("a near-miss name must not satisfy the requirement");
        assert!(msg.contains("staged-plan"), "{msg}");
    }

    /// A receipt that IS present and is missing a required skill fails, whatever
    /// the listing said before the session.
    #[test]
    fn a_present_receipt_missing_a_skill_fails() {
        let stream = r#"{"type":"session.skills_loaded","data":{"skills":[{"name":"reviewer"}]}}"#;
        let msg = skills_load_violation(stream, &required())
            .expect("a present receipt missing a skill is always a violation");
        assert!(
            msg.contains("setup-pocock") || msg.contains("staged-plan"),
            "{msg}"
        );
    }

    #[test]
    fn skills_receipt_lists_the_ralphy_skills_passes() {
        assert_eq!(skills_load_violation(FIXTURE, &required()), None);
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

        let msg = skills_load_violation(&stream, &required())
            .expect("a missing ralphy skill must fail the run");
        assert!(msg.contains("staged-plan"), "{msg}");
    }

    /// CLI 1.0.90 emits no receipt, and a run that died early may emit none
    /// either: an absent or unreadable receipt is not a violation, because the
    /// listing before the session already proved the skills.
    #[test]
    fn an_absent_or_unreadable_receipt_is_not_a_violation() {
        for stream in [
            "error: usage limit reached\n",
            r#"{"type":"session.skills_loaded","data":{"items":[]}}"#,
        ] {
            assert_eq!(skills_load_violation(stream, &required()), None, "{stream}");
        }
    }

    // ── skill_list_violation ──────────────────────────────────────────────

    /// Real `copilot skill list --json` output (CLI 1.0.90, FinCal, 2026-10-01;
    /// descriptions shortened, paths made neutral).
    const LISTING: &str = include_str!("../fixtures/skill-list-2026-10-01.json");

    fn listing_with(edit: impl Fn(&mut serde_json::Value)) -> String {
        let mut v: serde_json::Value = serde_json::from_str(LISTING).unwrap();
        for entry in v.as_array_mut().unwrap() {
            edit(entry);
        }
        serde_json::to_string(&v).unwrap()
    }

    #[test]
    fn a_listing_with_the_ralphy_skills_passes() {
        assert_eq!(skill_list_violation(LISTING, &required()), None);
    }

    #[test]
    fn a_listing_without_a_ralphy_skill_fails() {
        let listing = listing_with(|e| {
            if e["name"] == "staged-plan" {
                e["name"] = "staged-plan-legacy".into();
            }
        });
        let msg = skill_list_violation(&listing, &required()).expect("a missing skill fails");
        assert!(msg.contains("staged-plan"), "{msg}");
    }

    /// A disabled skill is listed but never offered to the model.
    #[test]
    fn a_disabled_ralphy_skill_fails() {
        let listing = listing_with(|e| {
            if e["name"] == "reviewer" {
                e["enabled"] = false.into();
            }
        });
        let msg = skill_list_violation(&listing, &required()).expect("a disabled skill fails");
        assert!(msg.contains("reviewer"), "{msg}");
    }

    #[test]
    fn an_unreadable_listing_fails_closed() {
        for listing in ["", "not json", r#"{"skills":[]}"#] {
            assert!(
                skill_list_violation(listing, &required()).is_some(),
                "{listing:?} must fail closed"
            );
        }
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
