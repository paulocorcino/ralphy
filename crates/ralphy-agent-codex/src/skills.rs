//! Materializing ralphy's embedded skills into Codex's discovery path
//! (`.agents/skills/`), additively alongside any skills the user already
//! maintains there.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
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

/// Link `src` into `dest` as a directory symlink, falling back to a recursive copy
/// when the symlink is rejected on Windows (no Developer Mode / not elevated).
fn link_or_copy_dir(src: &Path, dest: &Path) -> Result<()> {
    match symlink_dir(src, dest) {
        Ok(()) => Ok(()),
        Err(_) if cfg!(windows) => copy_dir_all(src, dest)
            .with_context(|| format!("copying {} -> {}", src.display(), dest.display())),
        Err(e) => {
            Err(e).with_context(|| format!("symlinking {} -> {}", src.display(), dest.display()))
        }
    }
}

#[cfg(unix)]
fn symlink_dir(src: &Path, dest: &Path) -> Result<()> {
    std::os::unix::fs::symlink(src, dest).map_err(Into::into)
}

#[cfg(windows)]
fn symlink_dir(src: &Path, dest: &Path) -> Result<()> {
    std::os::windows::fs::symlink_dir(src, dest).map_err(Into::into)
}

/// Remove a path that may be a symlink, a real directory, or a file — without
/// following a symlink into its target. On Windows a directory symlink must be
/// removed via `remove_dir`, a file symlink via `remove_file`, so both are tried.
fn remove_path(p: &Path) -> Result<()> {
    let ft = fs::symlink_metadata(p)?.file_type();
    if ft.is_symlink() {
        #[cfg(windows)]
        {
            fs::remove_file(p).or_else(|_| fs::remove_dir(p))?;
        }
        #[cfg(unix)]
        {
            fs::remove_file(p)?;
        }
    } else if ft.is_dir() {
        fs::remove_dir_all(p)?;
    } else {
        fs::remove_file(p)?;
    }
    Ok(())
}

/// Recursively copy `src` into `dest` (the Windows fallback when symlinks are
/// unavailable). Creates `dest` and every intermediate directory.
fn copy_dir_all(src: &Path, dest: &Path) -> Result<()> {
    fs::create_dir_all(dest)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let from = entry.path();
        let to = dest.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir_all(&from, &to)?;
        } else {
            fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

/// Ensure a `/<name>` ignore line exists for each ralphy skill — plus a
/// `/.gitignore` self-ignore — in `.agents/skills/.gitignore`, appending only
/// what's missing so any entries the user already keeps there survive. The
/// self-ignore keeps the file itself from being the one untracked thing that
/// surfaces `.agents/` in `git status`. Idempotent: a no-op once the lines exist.
fn ensure_gitignore_entries(path: &Path, names: &[std::ffi::OsString]) -> Result<()> {
    let existing = fs::read_to_string(path).unwrap_or_default();
    let mut lines: Vec<String> = existing.lines().map(str::to_string).collect();
    let mut changed = false;
    // Self-ignore `.gitignore` itself (`/.gitignore`) alongside each skill subdir.
    // Without the self-entry this file is the lone unignored thing left in
    // `.agents/skills/`, so `.agents/` shows as untracked and dirties the working
    // tree — aborting the next run's clean-tree check. (The OpenCode adapter avoids
    // this with a blanket `.ralphy/.gitignore = *`; Codex shares `.agents/skills`
    // with the user's own skills, so it ignores precisely its own subdirs plus
    // this file rather than the whole directory.)
    let entries = std::iter::once("/.gitignore".to_string())
        .chain(names.iter().map(|n| format!("/{}", n.to_string_lossy())));
    for entry in entries {
        if !lines.iter().any(|l| l.trim() == entry) {
            lines.push(entry);
            changed = true;
        }
    }
    if changed {
        let mut out = lines.join("\n");
        out.push('\n');
        fs::write(path, out).with_context(|| format!("writing {}", path.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
