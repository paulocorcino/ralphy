use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

const SKILLS_REPO_URL: &str = "https://github.com/paulocorcino/skills.git";

pub(crate) const SKILLS_SUBTREE: &str = "agents_template/skills";

/// The engineering skills offered by `init`, sourced from `SKILLS_REPO_URL`. This
/// list is informational — shown to the dev before consent so no network hit
/// precedes their `y`. The set actually installed is whatever `main` of the skills
/// repo holds at fetch time (see `sparse_fetch_commands`), so the two MAY drift;
/// keep this in sync with the repo's top-level dirs on each release.
const INSTALLABLE_SKILLS: &[&str] = &[
    "domain-modeling",
    "grill-me",
    "grill-with-docs",
    "handoff",
    "improve-codebase-architecture",
    "prototype",
    "to-issues",
    "to-prd",
];

/// The names shown to the dev before the skills download prompt. Static (not
/// derived from a local tree) because the skills now live in a separate repo.
pub fn skill_names() -> Vec<&'static str> {
    INSTALLABLE_SKILLS.to_vec()
}

/// Resolve the skills installation target from the configured agent skills dir.
/// - `None` → `.agents/skills`
/// - A value already ending in `skills` → used as-is (idempotent)
/// - `.codex` → `.agents/skills` (codex discovers there per ADR-0004)
/// - Anything else → `<dir>/skills`
pub fn skills_target(skills_dir: Option<&str>) -> PathBuf {
    match skills_dir {
        None => PathBuf::from(".agents/skills"),
        Some(d) if d == ".codex" || d.ends_with("/.codex") => PathBuf::from(".agents/skills"),
        Some(d) if d.ends_with("/skills") || d == "skills" => PathBuf::from(d),
        Some(d) if d.ends_with("skills") => PathBuf::from(d),
        Some(d) => PathBuf::from(d).join("skills"),
    }
}

/// Return `true` only when the dev explicitly authorizes the download with
/// `y` or `yes` (case-insensitive, trimmed). Any other answer — including silence
/// — declines. The prompt defaults to `[y/N]`, so silence is a network-safe no.
pub fn download_decision(answer: &str) -> bool {
    matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes")
}

/// The git ref the skills sparse-fetch follows. The skills live in their own repo
/// (`SKILLS_REPO_URL`), which has no Ralphy tags/SHAs to pin to — so init tracks the
/// tip of `main`. This trades reproducibility for "anyone can install the latest".
pub(crate) const SKILLS_REF: &str = "main";

/// Build the exact git argv sequence for a sparse fetch of `subtree` from the skills
/// repo at `git_ref`. Pure: the impure shell feeds these to `git::git`.
/// Order: init → remote add → sparse-checkout init --cone →
///        sparse-checkout set <subtree> → fetch --depth 1 origin <git_ref> →
///        checkout FETCH_HEAD.
pub fn sparse_fetch_commands(git_ref: &str, subtree: &str) -> Vec<Vec<String>> {
    vec![
        vec!["init".into()],
        vec![
            "remote".into(),
            "add".into(),
            "origin".into(),
            SKILLS_REPO_URL.into(),
        ],
        vec!["sparse-checkout".into(), "init".into(), "--cone".into()],
        vec!["sparse-checkout".into(), "set".into(), subtree.into()],
        vec![
            "fetch".into(),
            "--depth".into(),
            "1".into(),
            "origin".into(),
            git_ref.into(),
        ],
        vec!["checkout".into(), "FETCH_HEAD".into()],
    ]
}

/// Recursively copy `src` into `dst`, mirroring the directory structure.
fn copy_dir_all(src: &Path, dst: &Path) -> Result<()> {
    std::fs::create_dir_all(dst).with_context(|| format!("creating {}", dst.display()))?;
    for entry in std::fs::read_dir(src).with_context(|| format!("reading dir {}", src.display()))? {
        let entry = entry.with_context(|| format!("reading entry in {}", src.display()))?;
        let ty = entry
            .file_type()
            .with_context(|| format!("file type for {}", entry.path().display()))?;
        let dst_path = dst.join(entry.file_name());
        if ty.is_dir() {
            copy_dir_all(&entry.path(), &dst_path)?;
        } else {
            std::fs::copy(entry.path(), &dst_path).with_context(|| {
                format!(
                    "copying {} → {}",
                    entry.path().display(),
                    dst_path.display()
                )
            })?;
        }
    }

    Ok(())
}

/// Install skills from `src` into `dst` by replacing each managed skill subdir.
/// INVARIANT: only immediate subdirs of `src` are removed and replaced — unrelated
/// sibling dirs already in `dst` (the user's own skills) are never touched
/// (ADR-0004). Returns the count of installed skills.
pub fn install_skills_from(src: &Path, dst: &Path) -> Result<usize> {
    std::fs::create_dir_all(dst)
        .with_context(|| format!("creating skills dir {}", dst.display()))?;
    let mut count = 0usize;
    for entry in std::fs::read_dir(src)
        .with_context(|| format!("reading source skills {}", src.display()))?
    {
        let entry = entry.with_context(|| format!("reading entry in {}", src.display()))?;
        if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }

        let name = entry.file_name();
        let dst_skill = dst.join(&name);
        // Remove only this managed skill; sibling user dirs are untouched.
        let _ = std::fs::remove_dir_all(&dst_skill);
        copy_dir_all(&entry.path(), &dst_skill)?;
        count += 1;
    }

    Ok(count)
}

/// The result of the skills download step.
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    Installed(usize),
    #[allow(dead_code)]
    Skipped,
    Failed(String),
}

/// Run the skills install step.  `fetch` materialises a pinned subtree into a
/// scratch dir and returns the path to it.  EVERY failure — creating the scratch
/// dir, the fetch closure, or the subsequent copy — is absorbed and returned as
/// `Ok(Outcome::Failed(_))`.  This function NEVER propagates an error; the caller
/// (init's `run`) logs a warning and continues (warn-and-continue boundary).
pub fn install_skills_step(
    dst: &Path,
    fetch: impl FnOnce(&Path) -> Result<PathBuf>,
) -> Result<Outcome> {
    let scratch = std::env::temp_dir().join(format!("ralphy-skills-fetch-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&scratch);
    if let Err(e) = std::fs::create_dir_all(&scratch) {
        return Ok(Outcome::Failed(format!("creating scratch dir: {e}")));
    }

    let src = match fetch(&scratch) {
        Ok(p) => p,
        Err(e) => {
            let _ = std::fs::remove_dir_all(&scratch);
            return Ok(Outcome::Failed(e.to_string()));
        }
    };
    let outcome = match install_skills_from(&src, dst) {
        Ok(n) => Outcome::Installed(n),
        Err(e) => Outcome::Failed(e.to_string()),
    };
    let _ = std::fs::remove_dir_all(&scratch);
    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skill_names_is_non_empty_and_contains_to_issues() {
        let names = skill_names();
        assert!(!names.is_empty(), "INSTALLABLE_SKILLS must be non-empty");
        assert!(
            names.contains(&"to-issues"),
            "expected 'to-issues' in {:?}",
            names
        );
    }

    #[test]
    fn skills_target_maps_correctly() {
        assert_eq!(skills_target(None), PathBuf::from(".agents/skills"));
        assert_eq!(
            skills_target(Some(".codex")),
            PathBuf::from(".agents/skills")
        );
        assert_eq!(
            skills_target(Some(".claude")),
            PathBuf::from(".claude/skills")
        );
        assert_eq!(
            skills_target(Some(".cursor")),
            PathBuf::from(".cursor/skills")
        );
        // Already ends in "skills" → used as-is.
        assert_eq!(
            skills_target(Some(".agents/skills")),
            PathBuf::from(".agents/skills")
        );
    }

    #[test]
    fn download_decision_yes_true_others_false() {
        assert!(download_decision("yes"));
        assert!(download_decision("y"));
        assert!(download_decision("  Y  "));
        assert!(download_decision("YES"));
        assert!(!download_decision(""));
        assert!(!download_decision("n"));
        assert!(!download_decision("no"));
        assert!(!download_decision("maybe"));
    }

    #[test]
    fn sparse_fetch_commands_contains_expected_argv() {
        let git_ref = SKILLS_REF;
        let subtree = "agents_template/skills";
        let cmds = sparse_fetch_commands(git_ref, subtree);
        let fetch_argv: Vec<String> = vec!["fetch", "--depth", "1", "origin", git_ref]
            .into_iter()
            .map(str::to_string)
            .collect();
        let sc_set_argv: Vec<String> = vec!["sparse-checkout", "set", subtree]
            .into_iter()
            .map(str::to_string)
            .collect();
        assert!(cmds.contains(&fetch_argv), "missing fetch argv in {cmds:?}");
        assert!(
            cmds.contains(&sc_set_argv),
            "missing sparse-checkout set argv in {cmds:?}"
        );
        // No argv should reference a local path (token starts with `.` or is a
        // plain `name/name` without `://`) other than `subtree`.
        for argv in &cmds {
            for token in argv {
                let is_url = token.contains("://");
                if !is_url && token.contains('/') && token.as_str() != subtree {
                    panic!("unexpected path token {token:?} in {argv:?}");
                }
            }
        }
    }

    #[test]
    fn install_skills_from_idempotent_and_preserves_sibling() {
        let base = std::env::temp_dir().join(format!("ralphy-skills-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);

        // Build source fixture with two skill dirs.
        let src = base.join("src");
        std::fs::create_dir_all(src.join("skill-a")).unwrap();
        std::fs::write(src.join("skill-a").join("skill.md"), "skill-a content").unwrap();
        std::fs::create_dir_all(src.join("skill-b")).unwrap();
        std::fs::write(src.join("skill-b").join("skill.md"), "skill-b content").unwrap();

        let dst = base.join("dst");
        // Pre-create a stale file in managed skill and an unrelated sibling.
        std::fs::create_dir_all(dst.join("skill-a")).unwrap();
        std::fs::write(dst.join("skill-a").join("STALE.md"), "stale").unwrap();
        std::fs::create_dir_all(dst.join("user-skill")).unwrap();
        std::fs::write(dst.join("user-skill").join("keep.md"), "keep me").unwrap();

        // First install.
        let n = install_skills_from(&src, &dst).unwrap();
        assert_eq!(n, 2);
        // Stale file gone.
        assert!(
            !dst.join("skill-a").join("STALE.md").exists(),
            "STALE.md must be gone after install"
        );
        // Real skill file present.
        assert!(dst.join("skill-a").join("skill.md").exists());
        // Sibling preserved.
        assert!(
            dst.join("user-skill").join("keep.md").exists(),
            "user-skill sibling must survive"
        );

        // Second install (idempotency).
        let n2 = install_skills_from(&src, &dst).unwrap();
        assert_eq!(n2, 2);
        assert!(dst.join("skill-a").join("skill.md").exists());
        assert!(dst.join("user-skill").join("keep.md").exists());

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn install_skills_step_returns_failed_on_fetch_error() {
        let dst =
            std::env::temp_dir().join(format!("ralphy-skills-step-err-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dst);
        let result = install_skills_step(&dst, |_scratch| Err(anyhow::anyhow!("boom")));
        match result.unwrap() {
            Outcome::Failed(msg) => assert!(msg.contains("boom"), "expected 'boom' in {msg}"),
            other => panic!("expected Failed, got {other:?}"),
        }

        let _ = std::fs::remove_dir_all(&dst);
    }

    #[test]
    fn install_skills_step_returns_installed_on_success() {
        let base =
            std::env::temp_dir().join(format!("ralphy-skills-step-ok-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);

        // Build a fixture source with one skill.
        let src = base.join("src");
        std::fs::create_dir_all(src.join("my-skill")).unwrap();
        std::fs::write(src.join("my-skill").join("skill.md"), "content").unwrap();

        let dst = base.join("dst");
        let src_clone = src.clone();
        let result = install_skills_step(&dst, move |_scratch| Ok(src_clone));
        match result.unwrap() {
            Outcome::Installed(n) => assert_eq!(n, 1, "expected 1 skill installed"),
            other => panic!("expected Installed, got {other:?}"),
        }

        assert!(dst.join("my-skill").join("skill.md").exists());

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn install_skills_step_returns_failed_when_install_fails() {
        // Prove that a post-fetch copy error is also absorbed (not propagated).
        let base = std::env::temp_dir().join(format!(
            "ralphy-skills-step-installfail-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&base);
        // Build a fixture source — but point the fetch closure at a *file*, not a dir,
        // so install_skills_from's read_dir fails.
        let bad_src = base.join("not-a-dir.txt");
        std::fs::create_dir_all(&base).unwrap();
        std::fs::write(&bad_src, "oops").unwrap();
        let dst = base.join("dst");
        let bad_src_clone = bad_src.clone();
        let result = install_skills_step(&dst, move |_scratch| Ok(bad_src_clone));
        match result.unwrap() {
            Outcome::Failed(_) => {} // expected
            other => panic!("expected Failed, got {other:?}"),
        }

        let _ = std::fs::remove_dir_all(&base);
    }
}
