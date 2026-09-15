//! Shared cwd-to-repo attribution matcher (ADR-0033 §6, ADR-0063 §5). A vendor
//! session's cwd attributes to a registered repo either directly, or — when the
//! cwd is a linked worktree of that repo — through the worktree's `.git`
//! POINTER FILE (`gitdir: <primary>/.git/worktrees/<name>`). Everything here is
//! a file read, never a `git` spawn.

use std::fs;
use std::path::{Path, PathBuf};

use crate::RegisteredRepo;

/// Normalize a filesystem path for a case-insensitive compare: `\` → `/`,
/// trailing `/` trimmed.
pub(crate) fn normalize_path(p: &str) -> String {
    p.replace('\\', "/").trim_end_matches('/').to_string()
}

/// True when two paths name the same directory: normalized and compared with
/// `eq_ignore_ascii_case` (correct on Windows' case-insensitive FS).
pub(crate) fn paths_eq(a: &str, b: &str) -> bool {
    normalize_path(a).eq_ignore_ascii_case(&normalize_path(b))
}

/// `true` for a normalized (forward-slash) path that is absolute: POSIX-rooted
/// (`/…`) or a Windows drive (`C:/…`). Hand-rolled rather than
/// `Path::is_relative()` because that call's answer for `C:/x` depends on the
/// host OS, which would make the lexical-join fixtures diverge per CI leg.
fn is_absolute(p: &str) -> bool {
    p.starts_with('/')
        || (p.len() >= 3 && p.as_bytes()[0].is_ascii_alphabetic() && &p[1..3] == ":/")
}

/// Resolve a relative `gitdir:` target against a normalized cwd, lexically
/// (`.`/`..` collapsed, no filesystem IO).
fn join_lexically(cwd_norm: &str, target: &str) -> String {
    let mut components: Vec<&str> = cwd_norm.split('/').collect();
    for part in target.split('/') {
        match part {
            ".." => {
                components.pop();
            }
            "." | "" => {}
            _ => components.push(part),
        }
    }
    components.join("/")
}

/// When `cwd` is a linked worktree, the primary repo's path it points at —
/// read from `<cwd>/.git`, which git writes as a POINTER FILE (not a
/// directory) for a linked worktree: `gitdir: <primary>/.git/worktrees/<name>`.
/// `None` when `<cwd>/.git` is missing, is a directory (ordinary repo), or does
/// not carry the `…/.git/worktrees/<name>` shape. Does NOT require
/// `<primary>/.git/worktrees/<name>` to still exist on disk — a stale
/// (prunable) pointer still names the repo whose spend this is.
pub(crate) fn linked_worktree_primary(cwd: &str) -> Option<String> {
    let text = fs::read_to_string(Path::new(cwd).join(".git")).ok()?;
    let first = text.lines().next()?.trim();
    let target = first.strip_prefix("gitdir:")?.trim();
    let target = normalize_path(target);
    let target = if is_absolute(&target) {
        target
    } else {
        join_lexically(&normalize_path(cwd), &target)
    };
    let (rest, name) = target.rsplit_once('/')?;
    if name.is_empty() {
        return None;
    }
    let primary = rest.strip_suffix("/.git/worktrees")?;
    (!primary.is_empty()).then(|| primary.to_string())
}

/// Find the registered repo a session's cwd attributes to: a direct match
/// first (no IO), else — when `cwd` is a linked worktree — its primary repo.
pub(crate) fn find_repo<'a>(repos: &'a [RegisteredRepo], cwd: &str) -> Option<&'a RegisteredRepo> {
    if let Some(r) = repos.iter().find(|r| paths_eq(&r.path, cwd)) {
        return Some(r);
    }
    let primary = linked_worktree_primary(cwd)?;
    repos.iter().find(|r| paths_eq(&r.path, &primary))
}

/// The directories under `<repo_path>/.ralphy/worktrees/` that are linked
/// worktrees OF `repo_path` (ADR-0063 §1 fixed location) — used to resolve a
/// vendor's lossy, already-encoded cwd key "the other way" (Claude's dashed
/// key cannot be reversed, so this enumerates the bounded, known location
/// instead). A missing `.ralphy/worktrees/` yields an empty list, never an
/// error.
pub(crate) fn checkout_dirs(repo_path: &str) -> Vec<PathBuf> {
    let dir = Path::new(repo_path).join(".ralphy").join("worktrees");
    let Ok(entries) = fs::read_dir(&dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.is_dir()
                && linked_worktree_primary(&p.to_string_lossy())
                    .is_some_and(|primary| paths_eq(&primary, repo_path))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fwd(p: &Path) -> String {
        p.to_string_lossy().replace('\\', "/")
    }

    fn write_pointer(path: &Path, gitdir: &str) {
        fs::create_dir_all(path).unwrap();
        fs::write(path.join(".git"), format!("gitdir: {gitdir}\n")).unwrap();
    }

    #[test]
    fn linked_worktree_pointer_attributes_to_its_primary() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        let repo_fwd = fwd(&repo);
        let wt = tmp.path().join("wt");
        write_pointer(&wt, &format!("{repo_fwd}/.git/worktrees/wt"));

        let repos = vec![RegisteredRepo {
            slug: "o/repo".into(),
            path: repo.to_string_lossy().to_string(),
        }];
        assert_eq!(
            find_repo(&repos, &wt.to_string_lossy()).map(|r| r.slug.as_str()),
            Some("o/repo")
        );
        assert_eq!(
            linked_worktree_primary(&wt.to_string_lossy()).as_deref(),
            Some(repo_fwd.as_str())
        );

        // CRLF pointer also attributes.
        let wt_crlf = tmp.path().join("wt-crlf");
        fs::create_dir_all(&wt_crlf).unwrap();
        fs::write(
            wt_crlf.join(".git"),
            format!("gitdir: {repo_fwd}/.git/worktrees/wt-crlf\r\n"),
        )
        .unwrap();
        assert_eq!(
            find_repo(&repos, &wt_crlf.to_string_lossy()).map(|r| r.slug.as_str()),
            Some("o/repo")
        );
    }

    #[test]
    fn plain_directory_pointer_elsewhere_and_missing_git_are_not_attributed() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        let repos = vec![RegisteredRepo {
            slug: "o/repo".into(),
            path: repo.to_string_lossy().to_string(),
        }];

        let plain = tmp.path().join("plain");
        fs::create_dir_all(&plain).unwrap();
        assert_eq!(find_repo(&repos, &plain.to_string_lossy()), None);

        let elsewhere = tmp.path().join("elsewhere");
        let other = tmp.path().join("other");
        write_pointer(&elsewhere, &format!("{}/.git/worktrees/z", fwd(&other)));
        assert_eq!(find_repo(&repos, &elsewhere.to_string_lossy()), None);

        let gone = tmp.path().join("gone");
        assert_eq!(find_repo(&repos, &gone.to_string_lossy()), None);

        // Negative control for the `/.git/worktrees/<name>` suffix rule: a
        // `.git/modules/<name>` target (submodule shape) must NOT attribute.
        let sub = tmp.path().join("sub");
        write_pointer(&sub, &format!("{}/.git/modules/sub", fwd(&repo)));
        assert_eq!(linked_worktree_primary(&sub.to_string_lossy()), None);
    }

    #[test]
    fn relative_gitdir_pointer_resolves_lexically() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        let repo_fwd = fwd(&repo);
        let cwd = repo.join(".ralphy").join("worktrees").join("x");
        write_pointer(&cwd, "../../../.git/worktrees/x");

        assert_eq!(
            linked_worktree_primary(&cwd.to_string_lossy()).as_deref(),
            Some(repo_fwd.as_str())
        );
    }

    #[test]
    fn checkout_dirs_keeps_only_worktrees_of_the_repo() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        let repo_fwd = fwd(&repo);
        let other = tmp.path().join("other");
        let other_fwd = fwd(&other);
        let worktrees = repo.join(".ralphy").join("worktrees");

        write_pointer(
            &worktrees.join("x"),
            &format!("{repo_fwd}/.git/worktrees/x"),
        );
        write_pointer(
            &worktrees.join("y"),
            &format!("{other_fwd}/.git/worktrees/y"),
        );
        fs::create_dir_all(worktrees.join("z")).unwrap();
        fs::write(worktrees.join("README"), "not a directory").unwrap();

        let names: Vec<String> = checkout_dirs(&repo.to_string_lossy())
            .into_iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().to_string())
            .collect();
        assert_eq!(names, vec!["x".to_string()]);

        assert!(checkout_dirs(&tmp.path().join("nowhere").to_string_lossy()).is_empty());
    }
}
