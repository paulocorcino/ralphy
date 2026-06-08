//! The run lifecycle: cut a fresh branch off the base, ask the agent to plan,
//! and (on a dry run) return the repo to where it started, dropping the empty
//! run branch. Execution is a later slice; this slice stops after planning.

use std::path::Path;
use std::time::Instant;

use anyhow::{bail, Result};
use tracing::{info, warn};

use crate::{git, gitignore, Agent, Issue, IssueTracker, Outcome, Workspace};

/// The run's global deadline, behind a trait so "don't start a new issue past
/// the budget" is deterministically testable — an [`Instant`] can't be
/// fast-forwarded in a unit test, but a scripted clock can.
pub trait RunClock {
    fn deadline_passed(&self) -> bool;
}

/// The production clock: a wall-clock deadline. `None` never expires.
pub struct WallClock {
    pub deadline: Option<Instant>,
}

impl RunClock for WallClock {
    fn deadline_passed(&self) -> bool {
        match self.deadline {
            Some(d) => Instant::now() >= d,
            None => false,
        }
    }
}

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

/// Verify preconditions and cut a fresh run branch off the base, returning
/// `(orig_branch, run_branch)`. Shared by [`run`] and [`run_queue`]: the
/// clean-tree check, the `.gitignore` ensure, the detached-HEAD and missing-base
/// guards, and the branch creation all live here so both entry points agree.
fn prepare_branch(repo: &Path, base_branch: &str, stamp: &str) -> Result<(String, String)> {
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
    if !git::commitish_exists(repo, base_branch) {
        bail!("base branch '{base_branch}' not found");
    }

    let branch = format!("afk/run-{stamp}");
    git::checkout_new_branch(repo, &branch, base_branch)?;
    info!(%branch, base = %base_branch, was = %orig, "run branch created");
    Ok((orig, branch))
}

/// Plan (and, in a non-dry run, execute) a single issue onto a fresh run branch.
pub fn run(cfg: &RunConfig, issue: &Issue, agent: &dyn Agent) -> Result<RunReport> {
    let repo = cfg.repo_root.as_path();
    let ws = Workspace::new(repo);

    let (orig, branch) = prepare_branch(repo, &cfg.base_branch, &cfg.stamp)?;

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

/// Everything the core needs to work a whole queue. Like [`RunConfig`] but the
/// issues come from the caller (built via [`crate::github::list_queue`]) so the
/// loop itself stays `gh`-free and testable.
pub struct QueueConfig {
    pub repo_root: std::path::PathBuf,
    pub base_branch: String,
    pub dry_run: bool,
    pub stamp: String,
}

/// What happened to one issue in the queue.
#[derive(Debug)]
pub struct IssueResult {
    pub number: u64,
    /// The execution outcome, or `None` when the issue was skipped (infeasible
    /// plan) or only planned (dry run).
    pub outcome: Option<Outcome>,
    /// Whether the runner closed the issue (the cycle). Only ever true for a
    /// green, non-dry-run issue.
    pub closed: bool,
}

/// Why the queue loop stopped before reaching the end.
#[derive(Debug)]
pub enum StopReason {
    /// The deadline passed before the next issue could be started.
    Deadline,
    /// An issue finished non-green; the run hands back the branch as it stands.
    NonGreen { number: u64, outcome: Outcome },
}

/// The result of working a queue: the branch the commits landed on, where the
/// repo started, the per-issue results, and why the loop stopped (if it did).
#[derive(Debug)]
pub struct QueueReport {
    pub branch: String,
    pub orig_branch: String,
    pub worked: Vec<IssueResult>,
    pub stop: Option<StopReason>,
}

/// The close comment the runner leaves on a green queue issue. Mirrors the ps1
/// oracle so overnight-run behaviour is unchanged.
fn close_comment(stamp: &str, branch: &str) -> String {
    format!("Closed by Ralphy run {stamp} (green on branch '{branch}'; merge by hand).")
}

/// Work the whole queue in order: plan → execute each issue, close every green
/// one, and stop the moment one finishes non-green — handing back the branch as
/// it stands. The deadline is checked at the top of each iteration so a passed
/// budget prevents *starting* the next issue (work already done is kept).
pub fn run_queue(
    cfg: &QueueConfig,
    queue: &[Issue],
    agent: &dyn Agent,
    tracker: &dyn IssueTracker,
    clock: &dyn RunClock,
) -> Result<QueueReport> {
    let repo = cfg.repo_root.as_path();
    let ws = Workspace::new(repo);

    let (orig, branch) = prepare_branch(repo, &cfg.base_branch, &cfg.stamp)?;

    let mut worked: Vec<IssueResult> = Vec::new();
    let mut stop: Option<StopReason> = None;

    for issue in queue {
        // Don't start a new issue past the global budget. Work already committed
        // for earlier issues is kept; the branch is handed back as it stands.
        if clock.deadline_passed() {
            info!(
                number = issue.number,
                "deadline passed — not starting issue"
            );
            stop = Some(StopReason::Deadline);
            break;
        }

        // Plan, restoring on planning failure so a dry run never strands a branch.
        let plan = match agent.plan(issue, &ws) {
            Ok(p) => p,
            Err(e) => {
                restore(repo, &orig, &branch, &cfg.base_branch);
                return Err(e);
            }
        };
        info!(
            number = issue.number,
            open_steps = plan.open_steps,
            "plan written"
        );

        // An infeasible plan (no actionable steps) is a skip, not a failure, and
        // not green — the runner neither closes it nor stops the run.
        if !plan.is_feasible() {
            worked.push(IssueResult {
                number: issue.number,
                outcome: None,
                closed: false,
            });
            continue;
        }

        // A dry run plans only — it executes nothing and closes nothing.
        if cfg.dry_run {
            worked.push(IssueResult {
                number: issue.number,
                outcome: None,
                closed: false,
            });
            continue;
        }

        let outcome = agent.execute(&plan, &ws)?;
        if outcome == Outcome::Done {
            // Close the cycle: a green queue issue is closed so it leaves the
            // queue; its labels are untouched and the branch is merged by hand.
            tracker.close(issue.number, &close_comment(&cfg.stamp, &branch))?;
            info!(number = issue.number, "green — issue closed");
            worked.push(IssueResult {
                number: issue.number,
                outcome: Some(Outcome::Done),
                closed: true,
            });
            continue;
        }

        // Any non-green outcome stops the whole run; later issues are untouched.
        info!(number = issue.number, ?outcome, "non-green — stopping run");
        worked.push(IssueResult {
            number: issue.number,
            outcome: Some(outcome.clone()),
            closed: false,
        });
        stop = Some(StopReason::NonGreen {
            number: issue.number,
            outcome,
        });
        break;
    }

    // On a dry run, or when nothing landed on the branch, return the repo to
    // where it started and drop the empty run branch. Otherwise leave the repo on
    // the run branch for the human to inspect and merge.
    if cfg.dry_run {
        restore(repo, &orig, &branch, &cfg.base_branch);
    } else {
        let empty = git::rev_list_count(repo, &format!("{}..{}", cfg.base_branch, branch))
            .unwrap_or(1)
            == 0;
        if empty {
            restore(repo, &orig, &branch, &cfg.base_branch);
        }
    }

    Ok(QueueReport {
        branch,
        orig_branch: orig,
        worked,
        stop,
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
