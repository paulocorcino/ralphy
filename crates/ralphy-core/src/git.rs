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
