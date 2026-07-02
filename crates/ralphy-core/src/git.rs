//! Thin wrappers over the `git` CLI. Branch lifecycle is a core concern; the
//! commands themselves are an implementation detail kept in one place.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use anyhow::{bail, Context, Result};

fn raw(repo: &Path, args: &[&str]) -> Result<Output> {
    Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .with_context(|| format!("failed to spawn `git {}`", args.join(" ")))
}

/// Run a git command, returning trimmed stdout. Errors carry git's stderr.
pub fn git(repo: &Path, args: &[&str]) -> Result<String> {
    let out = raw(repo, args)?;
    if !out.status.success() {
        bail!(
            "`git {}` failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Resolve any path inside a repo to its git toplevel.
pub fn resolve_toplevel(path: &Path) -> Result<PathBuf> {
    let out = raw(path, &["rev-parse", "--show-toplevel"])?;
    if !out.status.success() {
        bail!(
            "not a git repository: {} (pass --repo <repo>)",
            path.display()
        );
    }
    Ok(PathBuf::from(
        String::from_utf8_lossy(&out.stdout).trim().to_string(),
    ))
}

/// Whether `path` is inside a git work tree. `ralphy init` uses this to decide
/// whether to bootstrap a fresh repo (`git init` + a GitHub remote) before
/// resolving the toplevel, rather than failing hard on a non-repo directory.
pub fn is_repo(path: &Path) -> bool {
    raw(path, &["rev-parse", "--is-inside-work-tree"])
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// `git init` a fresh repository at `path`, creating the directory first when it
/// does not exist yet. Used by `ralphy init`'s bootstrap to turn a plain directory
/// into a git repo before the environment gate runs.
pub fn init(path: &Path) -> Result<()> {
    std::fs::create_dir_all(path).with_context(|| format!("creating {}", path.display()))?;
    let out = raw(path, &["init", "--quiet"])?;
    if !out.status.success() {
        bail!(
            "`git init` failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(())
}

/// Stage everything and record the first commit. `--allow-empty` keeps a brand-new
/// empty directory working — it still gets a born branch and a commit to push, so
/// the later `current_branch` / GitHub-push steps in `ralphy init` don't trip over
/// an unborn HEAD. Used right after [`init`], before the GitHub remote is created.
pub fn initial_commit(repo: &Path) -> Result<()> {
    git(repo, &["add", "-A"])?;
    git(
        repo,
        &[
            "commit",
            "--allow-empty",
            "-m",
            "chore: initial commit",
            "--quiet",
        ],
    )?;
    Ok(())
}

/// Best-effort `git fetch origin`. A missing remote is not fatal here.
pub fn fetch_origin(repo: &Path) -> Result<()> {
    let _ = raw(repo, &["fetch", "origin", "--quiet"])?;
    Ok(())
}

pub fn current_branch(repo: &Path) -> Result<String> {
    git(repo, &["rev-parse", "--abbrev-ref", "HEAD"])
}

/// Best-effort `git remote get-url origin` for the header's repo link. `None` when
/// there is no `origin` remote (a local-only repo), so the caller simply omits the
/// link rather than failing the run.
pub fn origin_url(repo: &Path) -> Option<String> {
    git(repo, &["remote", "get-url", "origin"])
        .ok()
        .filter(|s| !s.is_empty())
}

/// Extract an `owner/repo` slug from a git remote URL (ADR-0008 D7). Handles the
/// `https://host/owner/repo[.git]`, `git@host:owner/repo[.git]`, and
/// `ssh://git@host/owner/repo[.git]` forms by stripping the scheme/host (or the
/// `git@host:` prefix) and the trailing `.git`, then taking the last two path
/// segments. Pure over its input. `None` when fewer than two segments remain.
pub fn slug_from_url(url: &str) -> Option<String> {
    let s = url.trim();
    let s = s.strip_suffix(".git").unwrap_or(s);
    // Drop the scheme+host (`scheme://host/…`) or the SCP-style `user@host:` prefix
    // so only the path remains. The first colon in the SCP form is the host/path
    // separator, so normalize it to `/` before splitting.
    let after_host = if let Some((_, rest)) = s.split_once("://") {
        rest
    } else if let Some((_, rest)) = s.split_once('@') {
        rest
    } else {
        s
    };
    let after_host = after_host.replacen(':', "/", 1);
    let segments: Vec<&str> = after_host
        .split('/')
        .filter(|seg| !seg.is_empty())
        .collect();
    if segments.len() >= 2 {
        let owner = segments[segments.len() - 2];
        let repo = segments[segments.len() - 1];
        Some(format!("{owner}/{repo}"))
    } else {
        None
    }
}

/// The project identity key (ADR-0008 D7): the `origin` remote normalized to an
/// `owner/repo` slug, or — for a local-only repo with no remote — a stable
/// `path-<hash>` slug derived from the repo-root path string (single-machine, but
/// never wrong). Always returns a non-empty string.
pub fn project_slug(repo: &Path) -> String {
    if let Some(slug) = origin_url(repo).as_deref().and_then(slug_from_url) {
        return slug;
    }
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    repo.to_string_lossy().hash(&mut hasher);
    format!("path-{:x}", hasher.finish())
}

/// `git config user.email` for the run's actor (ADR-0008 D7). `None` when unset
/// or empty — the caller substitutes a default rather than failing the run.
pub fn user_email(repo: &Path) -> Option<String> {
    git(repo, &["config", "user.email"])
        .ok()
        .filter(|s| !s.is_empty())
}

/// `git config user.name` for the actor's display name (ADR-0008 D7).
pub fn user_name(repo: &Path) -> Option<String> {
    git(repo, &["config", "user.name"])
        .ok()
        .filter(|s| !s.is_empty())
}

/// Whether `refname` resolves to a commit (branch, tag, remote-tracking, or SHA).
pub fn commitish_exists(repo: &Path, refname: &str) -> bool {
    raw(
        repo,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("{refname}^{{commit}}"),
        ],
    )
    .map(|o| o.status.success())
    .unwrap_or(false)
}

pub fn checkout_new_branch(repo: &Path, branch: &str, base: &str) -> Result<()> {
    git(repo, &["checkout", "-b", branch, base, "--quiet"])?;
    Ok(())
}

pub fn checkout(repo: &Path, refname: &str) -> Result<()> {
    git(repo, &["checkout", refname, "--quiet"])?;
    Ok(())
}

/// Switch to `refname`, discarding any uncommitted tracked changes. Used when
/// abandoning a run (dry-run cleanup or a plan failure): the run branch may carry
/// the uncommitted `.gitignore` edit, which must not follow us back to the
/// original branch.
pub fn checkout_force(repo: &Path, refname: &str) -> Result<()> {
    git(repo, &["checkout", "-f", refname, "--quiet"])?;
    Ok(())
}

pub fn delete_branch(repo: &Path, branch: &str) -> Result<()> {
    git(repo, &["branch", "-D", branch, "--quiet"])?;
    Ok(())
}

/// Create a lightweight local tag at `target`. Used for the run's pre-run undo
/// marker (`ralphy/pre-run-<stamp>`) — never pushed, so it stays a local recibo.
pub fn tag(repo: &Path, name: &str, target: &str) -> Result<()> {
    git(repo, &["tag", name, target])?;
    Ok(())
}

pub fn delete_tag(repo: &Path, name: &str) -> Result<()> {
    git(repo, &["tag", "-d", name])?;
    Ok(())
}

/// The current HEAD commit SHA. Used as the compare ref in `current` branch mode,
/// captured before any work so commit counts mean "work this run added".
pub fn head_sha(repo: &Path) -> Result<String> {
    git(repo, &["rev-parse", "HEAD"])
}

/// One-line log entries over `range` (e.g. `base..branch`), one per commit, in
/// `git log --oneline` order. An empty range yields an empty vec.
pub fn log_oneline(repo: &Path, range: &str) -> Result<Vec<String>> {
    let out = git(repo, &["log", "--oneline", range])?;
    Ok(out.lines().map(|l| l.to_string()).collect())
}

/// Number of commits in `range` (e.g. `base..branch`). Zero == empty branch.
pub fn rev_list_count(repo: &Path, range: &str) -> Result<usize> {
    let out = git(repo, &["rev-list", "--count", range])?;
    Ok(out.trim().parse().unwrap_or(0))
}

/// Stage everything (`git add -A`) and commit a snapshot of the working tree.
/// Used by `ralphy init` to isolate the dev's uncommitted changes before init
/// writes its own scaffold, so the two never mingle in one diff.
pub fn commit_all_snapshot(repo: &Path) -> Result<()> {
    git(repo, &["add", "-A"])?;
    git(
        repo,
        &[
            "commit",
            "-m",
            "chore: snapshot before ralphy init",
            "--quiet",
        ],
    )?;
    Ok(())
}

/// Clean ignoring anything under `.ralphy/` — scratch and logs never count as
/// a dirty tree (they live in the gitignored run dir).
pub fn is_clean_ignoring_ralphy(repo: &Path) -> Result<bool> {
    let status = git(repo, &["status", "--porcelain"])?;
    for line in status.lines() {
        if line.trim().is_empty() {
            continue;
        }
        if line.contains(".ralphy/") || line.contains(".ralphy\\") {
            continue;
        }
        return Ok(false);
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_slug_from_remote_and_path_fallback() {
        // Both common remote forms normalize to the same `owner/repo` slug.
        assert_eq!(
            slug_from_url("git@github.com:owner/repo.git").as_deref(),
            Some("owner/repo")
        );
        assert_eq!(
            slug_from_url("https://github.com/owner/repo").as_deref(),
            Some("owner/repo")
        );
        // The ssh:// form and a trailing `.git` are handled too.
        assert_eq!(
            slug_from_url("ssh://git@github.com/owner/repo.git").as_deref(),
            Some("owner/repo")
        );

        // A path with no git remote (a non-existent dir → `origin_url` is `None`)
        // falls back to a non-empty `path-<hash>` slug.
        let no_remote = std::env::temp_dir().join("ralphy-no-such-repo-xyz");
        let slug = project_slug(&no_remote);
        assert!(slug.starts_with("path-"), "fallback slug form: {slug}");
        assert!(
            slug.len() > "path-".len(),
            "fallback slug non-empty: {slug}"
        );
    }

    fn init_repo(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ralphy-git-{}-{}", std::process::id(), name));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        git(&dir, &["init", "-q", "-b", "main"]).unwrap();
        git(&dir, &["config", "user.email", "t@example.com"]).unwrap();
        git(&dir, &["config", "user.name", "Test"]).unwrap();
        dir
    }

    #[test]
    fn commit_all_snapshot_clears_dirty_tree_with_fixed_subject() {
        let dir = init_repo("snapshot");
        // Seed an initial commit so HEAD exists, then dirty the tree.
        std::fs::write(dir.join("README.md"), "hello\n").unwrap();
        git(&dir, &["add", "."]).unwrap();
        git(&dir, &["commit", "-q", "-m", "init"]).unwrap();
        std::fs::write(dir.join("README.md"), "changed\n").unwrap();
        std::fs::write(dir.join("new.txt"), "added\n").unwrap();

        commit_all_snapshot(&dir).unwrap();

        let porcelain = git(&dir, &["status", "--porcelain"]).unwrap();
        assert!(
            porcelain.is_empty(),
            "tree must be clean after snapshot, got:\n{porcelain}"
        );
        let subject = git(&dir, &["log", "-1", "--format=%s"]).unwrap();
        assert_eq!(subject, "chore: snapshot before ralphy init");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
