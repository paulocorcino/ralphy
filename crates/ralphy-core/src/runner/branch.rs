use std::path::Path;

use anyhow::{bail, Result};
use tracing::info;

use crate::{gitignore, repo::Repo};

/// How the run places its commits. `New` cuts a fresh `afk/run-*` branch off the
/// base; `Current` commits straight onto the branch the repo is already on (no
/// new branch, `base_branch` ignored). A plain enum so `ralphy-core` stays free
/// of any `clap` dependency — the CLI keeps its own value-enum and converts in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BranchMode {
    New,
    Current,
}

/// Verify preconditions and prepare the branch commits will land on, returning
/// `(orig_branch, branch, compare_ref)`. The single entry point for [`run_queue`]
/// to the clean-tree check, the `.gitignore` ensure, and the detached-HEAD guard.
///
/// In `New` mode a fresh `afk/run-<stamp>` branch is cut off `base_branch` (which
/// must exist) and `compare_ref == base_branch`. In `Current` mode no branch is
/// created and no checkout happens: `branch == orig` and `compare_ref` is the
/// HEAD SHA captured before any work, so the commit count means "work this run
/// added" in both modes.
pub(crate) fn prepare_branch(
    repo: &dyn Repo,
    repo_root: &Path,
    base_branch: &str,
    stamp: &str,
    mode: BranchMode,
) -> Result<(String, String, String)> {
    // Best-effort: make sure the base ref is up to date. A missing remote (e.g.
    // a local-only repo) is not fatal here — base existence is checked below.
    let _ = repo.fetch_origin();

    // Precondition: a clean tree, checked before any mutation (our own `.gitignore`
    // edit included) so a first run can never trip this.
    if !repo.is_clean_ignoring_ralphy()? {
        bail!(
            "working tree at {} is not clean — commit or stash first",
            repo_root.display()
        );
    }

    let orig = repo.current_branch()?;
    if orig == "HEAD" {
        bail!(
            "repo at {} is in detached HEAD — checkout a branch first",
            repo_root.display()
        );
    }

    let prepared = match mode {
        BranchMode::Current => {
            // Commit straight onto the current branch — no new branch, base
            // ignored. Compare against where this branch stood before the run.
            let compare_ref = repo.head_sha()?;
            info!(branch = %orig, "running in place on current branch");
            (orig.clone(), orig, compare_ref)
        }
        BranchMode::New => {
            if !repo.commitish_exists(base_branch) {
                bail!("base branch '{base_branch}' not found");
            }
            let branch = format!("afk/run-{stamp}");
            repo.checkout_new_branch(&branch, base_branch)?;
            info!(%branch, base = %base_branch, was = %orig, "run branch created");
            (orig, branch, base_branch.to_string())
        }
    };

    // Ignore `.ralphy/` *after* the run branch is checked out, so the edit lands on
    // the working tree the agent commits from. Doing it before the checkout would
    // inspect the original branch's `.gitignore` — which may already ignore
    // `.ralphy/` from a prior run — and no-op; the run branch, cut from a base that
    // does NOT ignore it, would then let the agent's `git add` sweep scratch
    // (`plan.md`, logs) into the deliverable.
    gitignore::ensure_ralphy_ignored(repo_root)?;

    Ok(prepared)
}

/// Whether a label set marks a human gate — used to split an open blocker into
/// "waiting on a human" (parked) versus ordinary agent work the queue resolves.
pub(crate) fn is_human_gate(labels: &[String]) -> bool {
    labels
        .iter()
        .any(|l| super::HUMAN_GATE_LABELS.contains(&l.as_str()))
}
