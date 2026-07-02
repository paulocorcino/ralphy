//! Git access for the queue loop, behind a trait — the way [`IssueTracker`]
//! isolates `gh` — so the loop unit-tests against a fake without a real
//! on-disk repository. The `git::*` free functions stay the implementation
//! (and the CLI's direct interface); [`GitRepo`] is just the runner's view of
//! them with the repo root baked in.
//!
//! [`IssueTracker`]: crate::IssueTracker

use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::git;

/// The git operations the run lifecycle needs, with the repo root baked into
/// the implementor (mirroring [`GhTracker`]'s shape). Methods a fake rarely
/// cares about carry neutral defaults, so a test overrides only what it
/// scripts — the same stance as [`IssueTracker`]'s default bodies.
///
/// [`GhTracker`]: crate::GhTracker
/// [`IssueTracker`]: crate::IssueTracker
pub trait Repo {
    /// The branch HEAD is on (`"HEAD"` when detached).
    fn current_branch(&self) -> Result<String>;

    /// The current HEAD commit SHA.
    fn head_sha(&self) -> Result<String>;

    /// The project identity key (ADR-0008 D7): `owner/repo` or a path-hash
    /// fallback. Always non-empty.
    fn project_slug(&self) -> String;

    /// Best-effort `git fetch origin`; a missing remote is not fatal.
    fn fetch_origin(&self) -> Result<()> {
        Ok(())
    }

    /// Clean ignoring anything under `.ralphy/`.
    fn is_clean_ignoring_ralphy(&self) -> Result<bool> {
        Ok(true)
    }

    /// Whether `refname` resolves to a commit.
    fn commitish_exists(&self, refname: &str) -> bool {
        let _ = refname;
        true
    }

    fn checkout_new_branch(&self, branch: &str, base: &str) -> Result<()> {
        let _ = (branch, base);
        Ok(())
    }

    /// Switch to `refname`, discarding uncommitted tracked changes.
    fn checkout_force(&self, refname: &str) -> Result<()> {
        let _ = refname;
        Ok(())
    }

    fn delete_branch(&self, branch: &str) -> Result<()> {
        let _ = branch;
        Ok(())
    }

    /// Create a lightweight local tag at `target` (the pre-run undo marker).
    fn tag(&self, name: &str, target: &str) -> Result<()> {
        let _ = (name, target);
        Ok(())
    }

    fn delete_tag(&self, name: &str) -> Result<()> {
        let _ = name;
        Ok(())
    }

    /// Number of commits in `range` (e.g. `base..branch`).
    fn rev_list_count(&self, range: &str) -> Result<usize> {
        let _ = range;
        Ok(0)
    }

    /// One-line log entries over `range`, one per commit.
    fn log_oneline(&self, range: &str) -> Result<Vec<String>> {
        let _ = range;
        Ok(Vec::new())
    }

    /// `git config user.email` — the ledger's actor key (ADR-0008 D7).
    fn user_email(&self) -> Option<String> {
        None
    }

    /// `git config user.name` — the actor's display name (ADR-0008 D7).
    fn user_name(&self) -> Option<String> {
        None
    }
}

/// The production [`Repo`]: the `git::*` free functions over a fixed root.
pub struct GitRepo {
    repo_root: PathBuf,
}

impl GitRepo {
    pub fn new(repo_root: impl AsRef<Path>) -> Self {
        Self {
            repo_root: repo_root.as_ref().to_path_buf(),
        }
    }
}

impl Repo for GitRepo {
    fn current_branch(&self) -> Result<String> {
        git::current_branch(&self.repo_root)
    }

    fn head_sha(&self) -> Result<String> {
        git::head_sha(&self.repo_root)
    }

    fn project_slug(&self) -> String {
        git::project_slug(&self.repo_root)
    }

    fn fetch_origin(&self) -> Result<()> {
        git::fetch_origin(&self.repo_root)
    }

    fn is_clean_ignoring_ralphy(&self) -> Result<bool> {
        git::is_clean_ignoring_ralphy(&self.repo_root)
    }

    fn commitish_exists(&self, refname: &str) -> bool {
        git::commitish_exists(&self.repo_root, refname)
    }

    fn checkout_new_branch(&self, branch: &str, base: &str) -> Result<()> {
        git::checkout_new_branch(&self.repo_root, branch, base)
    }

    fn checkout_force(&self, refname: &str) -> Result<()> {
        git::checkout_force(&self.repo_root, refname)
    }

    fn delete_branch(&self, branch: &str) -> Result<()> {
        git::delete_branch(&self.repo_root, branch)
    }

    fn tag(&self, name: &str, target: &str) -> Result<()> {
        git::tag(&self.repo_root, name, target)
    }

    fn delete_tag(&self, name: &str) -> Result<()> {
        git::delete_tag(&self.repo_root, name)
    }

    fn rev_list_count(&self, range: &str) -> Result<usize> {
        git::rev_list_count(&self.repo_root, range)
    }

    fn log_oneline(&self, range: &str) -> Result<Vec<String>> {
        git::log_oneline(&self.repo_root, range)
    }

    fn user_email(&self) -> Option<String> {
        git::user_email(&self.repo_root)
    }

    fn user_name(&self) -> Option<String> {
        git::user_name(&self.repo_root)
    }
}
