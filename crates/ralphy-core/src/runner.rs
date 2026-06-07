//! The run lifecycle: cut a fresh branch off the base, ask the agent to plan,
//! and (on a dry run) return the repo to where it started, dropping the empty
//! run branch. Execution is a later slice; this slice stops after planning.

use std::path::Path;

use anyhow::{bail, Result};
use tracing::{info, warn};

use crate::{git, gitignore, Agent, Issue, Outcome, Workspace};

/// Everything the core needs for one run — model-free by construction (model and
/// effort are adapter concerns, set when the adapter is built).
pub struct RunConfig {
    /// Git toplevel of the repo to work, in place.
    pub repo_root: std::path::PathBuf,
    /// Commit-ish the run branch is cut from (e.g. `origin/main`).
    pub base_branch: String,
    /// Plan only; make no source changes and leave no branch behind.
    pub dry_run: bool,
    /// Run identifier, also the run branch suffix: `afk/run-<stamp>`.
    pub stamp: String,
}

#[derive(Debug)]
pub enum RunOutcome {
    /// Planned and stopped; the empty run branch was dropped.
    DryRun { open_steps: usize },
    /// Executed to an [`Outcome`] (later slice).
    Executed(Outcome),
}

#[derive(Debug)]
pub struct RunReport {
    pub branch: String,
    pub orig_branch: String,
    pub outcome: RunOutcome,
}

/// Plan (and, in a non-dry run, execute) a single issue onto a fresh run branch.
pub fn run(cfg: &RunConfig, issue: &Issue, agent: &dyn Agent) -> Result<RunReport> {
    let repo = cfg.repo_root.as_path();
    let ws = Workspace::new(repo);

    // Best-effort: make sure the base ref is up to date. A missing remote (e.g.
    // a local-only repo) is not fatal here — base existence is checked below.
    let _ = git::fetch_origin(repo);

    // Precondition: a clean tree. Checked *before* we touch `.gitignore`, so our
    // own ignore edit on a first run can never trip this.
    if !git::is_clean_ignoring_ralphy(repo)? {
        bail!(
            "working tree at {} is not clean — commit or stash first",
            repo.display()
        );
    }
    gitignore::ensure_ralphy_ignored(repo)?;

    let orig = git::current_branch(repo)?;
    if orig == "HEAD" {
        bail!(
            "repo at {} is in detached HEAD — checkout a branch first",
            repo.display()
        );
    }
    if !git::commitish_exists(repo, &cfg.base_branch) {
        bail!("base branch '{}' not found", cfg.base_branch);
    }

    let branch = format!("afk/run-{}", cfg.stamp);
    git::checkout_new_branch(repo, &branch, &cfg.base_branch)?;
    info!(%branch, base = %cfg.base_branch, was = %orig, "run branch created");

    // Plan, restoring the repo on any failure so a dry run never strands a branch.
    let plan = match agent.plan(issue, &ws) {
        Ok(p) => p,
        Err(e) => {
            restore(repo, &orig, &branch, &cfg.base_branch);
            return Err(e);
        }
    };
    info!(open_steps = plan.open_steps, "plan written");

    if cfg.dry_run {
        restore(repo, &orig, &branch, &cfg.base_branch);
        return Ok(RunReport {
            branch,
            orig_branch: orig,
            outcome: RunOutcome::DryRun {
                open_steps: plan.open_steps,
            },
        });
    }

    let outcome = agent.execute(&plan, &ws)?;
    Ok(RunReport {
        branch,
        orig_branch: orig,
        outcome: RunOutcome::Executed(outcome),
    })
}

/// Return to the original branch and drop the run branch if it carries no
/// commits over the base. Failures are logged, not propagated — restore runs in
/// cleanup paths where the primary result is already decided.
fn restore(repo: &Path, orig: &str, branch: &str, base: &str) {
    if let Err(e) = git::checkout(repo, orig) {
        warn!("could not return to '{orig}': {e}");
        return;
    }
    let empty = git::rev_list_count(repo, &format!("{base}..{branch}")).unwrap_or(1) == 0;
    if empty {
        if let Err(e) = git::delete_branch(repo, branch) {
            warn!("could not delete empty run branch '{branch}': {e}");
        }
    }
}
