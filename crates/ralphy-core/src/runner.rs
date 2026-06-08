//! The run lifecycle: cut a fresh branch off the base, ask the agent to plan,
//! and (on a dry run) return the repo to where it started, dropping the empty
//! run branch. Execution is a later slice; this slice stops after planning.

use std::path::Path;
use std::time::Instant;

use anyhow::{bail, Context, Result};
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

/// How the run places its commits. `New` cuts a fresh `afk/run-*` branch off the
/// base; `Current` commits straight onto the branch the repo is already on (no
/// new branch, `base_branch` ignored). A plain enum so `ralphy-core` stays free
/// of any `clap` dependency — the CLI keeps its own value-enum and converts in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BranchMode {
    New,
    Current,
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
    /// Where commits land: a fresh run branch (`New`) or the current branch
    /// (`Current`). The single-issue `run` path is effectively always `New`.
    pub branch_mode: BranchMode,
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

/// Verify preconditions and prepare the branch commits will land on, returning
/// `(orig_branch, branch, compare_ref)`. Shared by [`run`] and [`run_queue`] so
/// both entry points agree on the clean-tree check, the `.gitignore` ensure, and
/// the detached-HEAD guard.
///
/// In `New` mode a fresh `afk/run-<stamp>` branch is cut off `base_branch` (which
/// must exist) and `compare_ref == base_branch`. In `Current` mode no branch is
/// created and no checkout happens: `branch == orig` and `compare_ref` is the
/// HEAD SHA captured before any work, so the commit count means "work this run
/// added" in both modes.
fn prepare_branch(
    repo: &Path,
    base_branch: &str,
    stamp: &str,
    mode: BranchMode,
) -> Result<(String, String, String)> {
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

    match mode {
        BranchMode::Current => {
            // Commit straight onto the current branch — no new branch, base
            // ignored. Compare against where this branch stood before the run.
            let compare_ref = git::head_sha(repo)?;
            info!(branch = %orig, "running in place on current branch");
            Ok((orig.clone(), orig, compare_ref))
        }
        BranchMode::New => {
            if !git::commitish_exists(repo, base_branch) {
                bail!("base branch '{base_branch}' not found");
            }
            let branch = format!("afk/run-{stamp}");
            git::checkout_new_branch(repo, &branch, base_branch)?;
            info!(%branch, base = %base_branch, was = %orig, "run branch created");
            Ok((orig, branch, base_branch.to_string()))
        }
    }
}

/// Plan (and, in a non-dry run, execute) a single issue onto a fresh run branch.
pub fn run(cfg: &RunConfig, issue: &Issue, agent: &dyn Agent) -> Result<RunReport> {
    let repo = cfg.repo_root.as_path();
    let ws = Workspace::new(repo);

    let (orig, branch, _compare_ref) =
        prepare_branch(repo, &cfg.base_branch, &cfg.stamp, cfg.branch_mode)?;

    // Plan, restoring the repo on any failure so a dry run never strands a branch.
    let plan = match agent.plan(issue, &ws) {
        Ok(p) => p,
        Err(e) => {
            restore(repo, &orig, &branch, &cfg.base_branch, cfg.branch_mode);
            return Err(e);
        }
    };
    info!(open_steps = plan.open_steps, "plan written");

    if cfg.dry_run {
        restore(repo, &orig, &branch, &cfg.base_branch, cfg.branch_mode);
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

/// The label that pauses the run before the tagged issue (flow-control, not triage).
const STOP_BEFORE_LABEL: &str = "stop-before";

/// Everything the core needs to work a whole queue. Like [`RunConfig`] but the
/// issues come from the caller (built via [`crate::github::list_queue`]) so the
/// loop itself stays `gh`-free and testable.
pub struct QueueConfig {
    pub repo_root: std::path::PathBuf,
    pub base_branch: String,
    pub dry_run: bool,
    pub stamp: String,
    /// Where commits land: a fresh `afk/run-*` branch (`New`) or the branch the
    /// repo is already on (`Current`, which ignores `base_branch`).
    pub branch_mode: BranchMode,
    /// When set, the `stop-before` label on this specific issue is ignored and
    /// the issue runs normally. Mirrors ps1's `$OnlyIssue -le 0` guard.
    pub only_issue: Option<u64>,
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
    /// A `stop-before` label halted the run before the tagged issue.
    StopBefore { number: u64 },
    /// The agent hit a usage/rate limit; includes the parsed reset time when
    /// present in the transcript.
    Limit { number: u64, reset: Option<String> },
}

/// The result of working a queue: the branch the commits landed on, where the
/// repo started, the per-issue results, and why the loop stopped (if it did).
#[derive(Debug)]
pub struct QueueReport {
    pub branch: String,
    pub orig_branch: String,
    pub worked: Vec<IssueResult>,
    pub stop: Option<StopReason>,
    /// Number of commits the run added over the compare ref (the base in `New`
    /// mode, the pre-run HEAD in `Current` mode).
    pub commits: usize,
    /// One `git log --oneline` entry per counted commit.
    pub oneline: Vec<String>,
}

/// The close comment the runner leaves on a green queue issue. Mirrors the ps1
/// oracle so overnight-run behaviour is unchanged.
fn close_comment(stamp: &str, branch: &str) -> String {
    format!("Closed by Ralphy run {stamp} (green on branch '{branch}'; merge by hand).")
}

/// Write the issue the planner reads to `.ralphy/issue.json`.
fn write_issue_json(ws: &Workspace, issue: &Issue) -> Result<()> {
    std::fs::create_dir_all(ws.ralphy_dir())?;
    let json = serde_json::to_string_pretty(issue).context("serializing issue to JSON")?;
    std::fs::write(ws.issue_json_path(), json).context("writing .ralphy/issue.json")?;
    Ok(())
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

    let (orig, branch, compare_ref) =
        prepare_branch(repo, &cfg.base_branch, &cfg.stamp, cfg.branch_mode)?;

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

        // Stop-before: a flow-control label that pauses the run before the tagged
        // issue. `only_issue` overrides it (the queue was pre-filtered to that
        // issue, so the operator explicitly wants it to run).
        if cfg.only_issue.is_none() && issue.labels.iter().any(|l| l == STOP_BEFORE_LABEL) {
            info!(
                number = issue.number,
                "stop-before label — halting run before this issue"
            );
            stop = Some(StopReason::StopBefore {
                number: issue.number,
            });
            break;
        }

        // Persist the current issue where the planner reads it. The adapter's
        // prompt reads `.ralphy/issue.json`, so the loop must refresh it before
        // each plan — `.ralphy/` is gitignored and survives the branch checkout.
        if let Err(e) = write_issue_json(&ws, issue) {
            restore(repo, &orig, &branch, &cfg.base_branch, cfg.branch_mode);
            return Err(e);
        }

        // Plan, restoring on planning failure so a dry run never strands a branch.
        let plan = match agent.plan(issue, &ws) {
            Ok(p) => p,
            Err(e) => {
                restore(repo, &orig, &branch, &cfg.base_branch, cfg.branch_mode);
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
        let number = issue.number;
        worked.push(IssueResult {
            number,
            outcome: Some(outcome.clone()),
            closed: false,
        });
        stop = Some(match outcome {
            Outcome::Limit(reset) => StopReason::Limit { number, reset },
            other => StopReason::NonGreen {
                number,
                outcome: other,
            },
        });
        break;
    }

    // Count what the run added over the compare ref and capture the oneline log,
    // matching the ps1 `finally` block. Failures here are non-fatal reporting
    // concerns (e.g. a dropped branch in cleanup) — default to zero / empty.
    let range = format!("{compare_ref}..{branch}");
    let commits = git::rev_list_count(repo, &range).unwrap_or(0);
    let oneline = git::log_oneline(repo, &range).unwrap_or_default();

    // Closing-state matrix, keyed on mode × outcome × dry-run (ps1 `finally`):
    //  - Current: commits already live on the branch — never check out or delete.
    //  - New + dry-run: plans only — return to orig and drop the empty branch.
    //  - New + stop: leave the repo on the run branch for inspection.
    //  - New + clean run: return to orig; the run branch is kept (not deleted).
    match cfg.branch_mode {
        BranchMode::Current => {}
        BranchMode::New => {
            if cfg.dry_run {
                restore(repo, &orig, &branch, &cfg.base_branch, cfg.branch_mode);
            } else if stop.is_none() {
                if let Err(e) = git::checkout(repo, &orig) {
                    warn!("could not return to '{orig}': {e}");
                }
            }
        }
    }

    Ok(QueueReport {
        branch,
        orig_branch: orig,
        worked,
        stop,
        commits,
        oneline,
    })
}

/// Return to the original branch and drop the run branch if it carries no
/// commits over the base. Failures are logged, not propagated — restore runs in
/// cleanup paths where the primary result is already decided.
///
/// A no-op in [`BranchMode::Current`]: there `orig == branch` is the live branch,
/// so checking it out is pointless and the empty-branch delete would target the
/// checked-out branch. Centralizing the guard here keeps every cleanup path —
/// including the mid-loop error paths — from ever touching the live branch.
fn restore(repo: &Path, orig: &str, branch: &str, base: &str, mode: BranchMode) {
    if mode == BranchMode::Current {
        return;
    }
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
