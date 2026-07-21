//! Materializing ralphy's embedded skills into Gemini's owned configuration
//! root (ADR-0043 D13), plus a model-free receipt that confirms discovery
//! without paying for a turn.
//!
//! Unlike Codex/Copilot/Cursor, this root is 100% Ralphy-owned (D4): there is
//! no operator-shared directory to link into, no foreign-skill harvest to warn
//! about, and no `.gitignore` merge dance — `materialize_assets`'s
//! clear-and-replace is safe here because nothing but Ralphy ever writes under
//! `<GEMINI_CLI_HOME>/.gemini/skills`.

use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};
use include_dir::{include_dir, Dir};

/// The skills subtree, embedded at build time so the binary is self-contained.
static SKILLS: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/../../assets/plugin/skills");

/// Materialize the embedded skills into `<root>/.gemini/skills`, the vendor's
/// USER tier (D13). Returns the sorted skill names for the caller's discovery
/// receipt.
pub(crate) fn materialize_gemini_skills(root: &crate::root::GeminiRoot) -> Result<Vec<String>> {
    let dest = root.cli_dir().join("skills");
    ralphy_adapter_support::materialize_assets(&SKILLS, &dest, None)?;

    let mut names: Vec<String> = SKILLS
        .dirs()
        .map(|d| {
            d.path()
                .file_name()
                .context("embedded skill directory has no name")
                .map(|n| n.to_string_lossy().into_owned())
        })
        .collect::<Result<Vec<String>>>()?;
    names.sort();
    Ok(names)
}

/// The `required` names appearing as substrings of `output` (stdout+stderr
/// combined by the caller), in `required`'s own order. Pure, so the shape of
/// `gemini skills list` can be pinned in a test without spawning anything.
pub(crate) fn present_skills(output: &str, required: &[String]) -> Vec<String> {
    required
        .iter()
        .filter(|name| output.contains(name.as_str()))
        .cloned()
        .collect()
}

/// Confirm discovery with `gemini skills list` — no model call, no request
/// (D13's cheap-diagnosis criterion). `None` on a spawn error or timeout;
/// never fails the run, and never `?`s in the caller.
pub(crate) fn probe_skill_discovery(
    home: &Path,
    auth_type: Option<&str>,
    required: &[String],
) -> Option<Vec<String>> {
    let mut cmd = std::process::Command::new(crate::command::resolve_gemini_program());
    cmd.args(["skills", "list"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    crate::command::apply_auth_env(&mut cmd, std::env::vars().map(|(k, _)| k), auth_type, home);

    let out = ralphy_adapter_support::run_headless(cmd, "", Duration::from_secs(30)).ok()?;
    if out.timed_out {
        return None;
    }
    let combined = format!("{}{}", out.stdout, out.stderr);
    Some(present_skills(&combined, required))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// Captured 2026-07-21 via `gemini skills list` in a fresh, untrusted
    /// scratch cwd, no `--skip-trust`, all three embedded skills materialized
    /// (`.ralphy/gemprobe/skills-list.txt`, scratch and not committed — pasted
    /// here verbatim since the source file itself is not part of the tree).
    const LIVE_LISTING: &str = r#"Skipping project agents due to untrusted folder. To enable, ensure that the project root is trusted.
Ripgrep is not available. Falling back to GrepTool.
Project hooks disabled because the folder is not trusted.
Discovered Agent Skills:

reviewer [Enabled]
  Description: Use ONLY when the user explicitly invokes /reviewer (literal slash command). Performs a native, findings-first review with a deterministic coverage audit run by the reviewer before emission (`scripts/fact_pack.py` + `scripts/audit.py`). Four subagent capabilities (defect-hunter, test-auditor, verifier, scout) are spawnable on judgment, not always-on. During validation this skill must NOT match generic "code review" requests.
  Location:    C:\Dev\ralphy\.ralphy\gemprobe\gem-probe\gemini-home\.gemini\skills\reviewer\SKILL.md

setup-pocock [Enabled]
  Description: Sets up an `## Agent skills` block in AGENTS.md/CLAUDE.md and `docs/agents/` so the engineering skills know this repo's issue tracker (GitHub or local markdown), triage label vocabulary, domain doc layout, and — optionally — a PRD/roadmap track model. Run before first use of `to-issues`, `to-prd`, `triage`, `diagnose`, `tdd`, `improve-codebase-architecture`, or `zoom-out` — or if those skills appear to be missing context about the issue tracker, triage labels, or domain docs.
  Location:    C:\Dev\ralphy\.ralphy\gemprobe\gem-probe\gemini-home\.gemini\skills\setup-pocock\SKILL.md

staged-plan [Enabled]
  Description: Design a self-contained multi-stage plan whose markdown is the operational contract — every execution detail (Execution model, Hand-off conventions, retry rule, working-tree policy, reviewer gate, pre-execution placeholder gate) is encoded in the plan file itself. This is a PLANNING skill — it produces a plan and stops. Use when the user wants to design, scaffold, or decompose work into a staged subagent track. Typical invocations - "design a staged plan", "decompose this into stages", "scaffold a multi-stage plan", "plan in stages", "create a staged execution plan". Do NOT invoke during Phase 2 execution — the plan markdown is self-sufficient and re-invoking the skill is redundant.
  Location:    C:\Dev\ralphy\.ralphy\gemprobe\gem-probe\gemini-home\.gemini\skills\staged-plan\SKILL.md

EXIT:0
"#;

    fn base(tag: &str) -> tempfile::TempDir {
        tempfile::Builder::new()
            .prefix(&format!("ralphy-gemini-skills-{tag}-"))
            .tempdir()
            .expect("tempdir")
    }

    #[test]
    fn materialize_lands_every_skill_in_the_owned_root() {
        let dir = base("lands");
        let root = crate::root::ensure(dir.path()).expect("ensure");

        let names = materialize_gemini_skills(&root).expect("materialize");

        let reviewer_md = root.cli_dir().join("skills/reviewer/SKILL.md");
        assert!(reviewer_md.is_file(), "{reviewer_md:?} must exist");
        assert!(
            !fs::read_to_string(&reviewer_md).unwrap().is_empty(),
            "reviewer/SKILL.md must be non-empty"
        );
        assert_eq!(
            names,
            vec![
                "reviewer".to_string(),
                "setup-pocock".to_string(),
                "staged-plan".to_string()
            ]
        );
    }

    #[test]
    fn materialize_is_idempotent_and_leaves_the_root_alone() {
        let dir = base("idempotent");
        let root = crate::root::ensure(dir.path()).expect("ensure");
        let id_file = root.cli_dir().join("installation_id");
        fs::write(&id_file, b"b54f6a30-stable").unwrap();
        let settings_bytes = fs::read(&root.settings).unwrap();

        materialize_gemini_skills(&root).expect("first pass");

        fn snapshot(dir: &Path) -> Vec<(std::path::PathBuf, Vec<u8>)> {
            let mut out = Vec::new();
            let mut stack = vec![dir.to_path_buf()];
            while let Some(d) = stack.pop() {
                let Ok(entries) = fs::read_dir(&d) else {
                    continue;
                };
                for e in entries.flatten() {
                    let p = e.path();
                    if p.is_dir() {
                        stack.push(p);
                    } else {
                        out.push((p.clone(), fs::read(&p).unwrap()));
                    }
                }
            }
            out.sort_by(|a, b| a.0.cmp(&b.0));
            out
        }

        let skills_dir = root.cli_dir().join("skills");
        let before = snapshot(&skills_dir);
        materialize_gemini_skills(&root).expect("second pass");
        let after = snapshot(&skills_dir);

        assert_eq!(
            before, after,
            "the skills tree must be byte-identical across passes"
        );
        assert_eq!(fs::read(&root.settings).unwrap(), settings_bytes);
        assert_eq!(fs::read_to_string(&id_file).unwrap(), "b54f6a30-stable");
    }

    #[test]
    fn materialize_never_writes_under_the_operators_gemini_root() {
        let Some(home) = ralphy_proc_util::home_dir() else {
            return; // no HOME/USERPROFILE resolvable in this environment
        };
        let operator_skills = home.join(".gemini").join("skills");

        fn snapshot(dir: &Path) -> Option<Vec<std::ffi::OsString>> {
            let mut names: Vec<_> = fs::read_dir(dir)
                .ok()?
                .flatten()
                .map(|e| e.file_name())
                .collect();
            names.sort();
            Some(names)
        }

        let before = snapshot(&operator_skills);
        let dir = base("home-safety");
        let root = crate::root::ensure(dir.path()).expect("ensure");
        materialize_gemini_skills(&root).expect("materialize");
        let after = snapshot(&operator_skills);

        assert_eq!(
            before, after,
            "materialize_gemini_skills must never write under the operator's real \
             ~/.gemini/skills: {operator_skills:?}"
        );
    }

    #[test]
    fn embedded_skill_frontmatter_carries_name_and_description() {
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
            let mut lines = md.lines();
            assert_eq!(
                lines.next(),
                Some("---"),
                "{dir_name}/SKILL.md must open with ---"
            );
            let front: Vec<&str> = lines.by_ref().take_while(|l| *l != "---").collect();
            assert!(
                front.iter().any(|l| l.starts_with("name:")),
                "{dir_name}/SKILL.md frontmatter has no name:"
            );
            assert!(
                front.iter().any(|l| l.starts_with("description:")),
                "{dir_name}/SKILL.md frontmatter has no description:"
            );
            checked += 1;
        }
        assert!(checked >= 3, "expected >= 3 skills, checked {checked}");
    }

    #[test]
    fn present_skills_reads_the_live_listing() {
        let required = vec![
            "reviewer".to_string(),
            "setup-pocock".to_string(),
            "staged-plan".to_string(),
        ];
        assert_eq!(
            present_skills(LIVE_LISTING, &required),
            vec![
                "reviewer".to_string(),
                "setup-pocock".to_string(),
                "staged-plan".to_string()
            ]
        );
        assert_eq!(
            present_skills("No skills discovered.\n", &required),
            Vec::<String>::new()
        );
    }
}
