//! The run lifecycle: cut a fresh branch off the base, ask the agent to plan,
//! and (on a dry run) return the repo to where it started, dropping the empty
//! run branch. Execution is a later slice; this slice stops after planning.

use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use chrono::{DateTime, Datelike, Local, NaiveTime, Weekday};
use tracing::{info, warn};

use crate::{
    acceptance, blocked, git, gitignore, Agent, Issue, IssueTracker, Outcome, PlanLimit, Workspace,
};

/// Consecutive plan-time usage limits that make no progress before the runner
/// gives up and stops-and-reports. Guards a past or unparseable reset hint from
/// spinning the resume loop, mirroring the execute-path no-commit cap.
const MAX_PLAN_LIMIT_RESUMES: u32 = 2;

/// How a [`RunClock::wait_for_reset`] wait ended: the reset time arrived and the
/// run may resume, or the global deadline cut the wait short (deadline beats
/// resume).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaitOutcome {
    Resumed,
    DeadlinePassed,
}

/// The run's global deadline, behind a trait so "don't start a new issue past
/// the budget" is deterministically testable — an [`Instant`] can't be
/// fast-forwarded in a unit test, but a scripted clock can. The same indirection
/// lets [`wait_for_reset`](RunClock::wait_for_reset) return instantly under a
/// scripted clock instead of sleeping until a real reset time.
pub trait RunClock {
    fn deadline_passed(&self) -> bool;

    /// Block until the parsed `reset` time (plus a wait-policy buffer), polling so
    /// the wait stays interruptible and emits a heartbeat. Returns
    /// [`WaitOutcome::Resumed`] once the reset arrives, or
    /// [`WaitOutcome::DeadlinePassed`] if the global deadline is already past or
    /// passes during the wait (deadline beats resume).
    fn wait_for_reset(&self, reset: &str) -> WaitOutcome;
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

    fn wait_for_reset(&self, reset: &str) -> WaitOutcome {
        // The 5-minute buffer is a wait policy (wake a little after the reset to
        // avoid re-limiting), applied here rather than baked into `next_reset`.
        let buffer = chrono::Duration::minutes(5);
        let target = match next_reset(reset, Local::now()) {
            Some(t) => t + buffer,
            // An unparseable reset should never reach here (the loop only calls
            // wait_for_reset when a reset was parsed); resume immediately rather
            // than sleep on a guess.
            None => return WaitOutcome::Resumed,
        };

        if self.deadline_passed() {
            return WaitOutcome::DeadlinePassed;
        }
        // A reset beyond the global deadline never sleeps — the deadline wins the
        // moment it would pass.
        if let Some(d) = self.deadline {
            let until_target = (target - Local::now()).num_milliseconds().max(0) as u64;
            if Instant::now() + Duration::from_millis(until_target) >= d {
                info!(%reset, "reset lands beyond the run deadline — not waiting");
                return WaitOutcome::DeadlinePassed;
            }
        }

        info!(%reset, target = %target.format("%Y-%m-%d %H:%M"), target_epoch = target.timestamp(), "usage limit — waiting for reset");
        let mut last_heartbeat = Instant::now();
        loop {
            if self.deadline_passed() {
                return WaitOutcome::DeadlinePassed;
            }
            if Local::now() >= target {
                info!("reset reached — resuming");
                return WaitOutcome::Resumed;
            }
            if last_heartbeat.elapsed() >= Duration::from_secs(60) {
                let remaining = (target - Local::now()).num_minutes().max(0);
                info!(remaining_min = remaining, "waiting for usage-limit reset");
                last_heartbeat = Instant::now();
            }
            thread::sleep(Duration::from_secs(1));
        }
    }
}

/// The next clock occurrence of a parsed reset hint relative to `now`. `reset` is
/// either a bare `"HH:mm"` or a weekday-qualified `"Wkd HH:mm"`. A bare time
/// resolves to today, rolled to tomorrow when already past `now`; a
/// weekday-qualified time resolves to the next date carrying that weekday (today
/// only when the time is still ahead, else next week). Pure over its inputs so the
/// rollover edge cases unit-test without sleeping. Returns `None` on an
/// unparseable hint.
fn next_reset(reset: &str, now: DateTime<Local>) -> Option<DateTime<Local>> {
    let (weekday, hhmm) = match reset.trim().split_once(char::is_whitespace) {
        Some((wd, rest)) => (Some(parse_weekday(wd.trim())?), rest.trim()),
        None => (None, reset.trim()),
    };
    let (h, m) = hhmm.split_once(':')?;
    let hour: u32 = h.parse().ok()?;
    let min: u32 = m.parse().ok()?;
    let time = NaiveTime::from_hms_opt(hour, min, 0)?;

    let today = now.date_naive();
    let target_date = match weekday {
        None => {
            if now.time() < time {
                today
            } else {
                today + chrono::Duration::days(1)
            }
        }
        Some(wd) => {
            let cur = today.weekday().num_days_from_monday() as i64;
            let tgt = wd.num_days_from_monday() as i64;
            let mut days = (tgt - cur).rem_euclid(7);
            // Same weekday today: keep today only if the time is still ahead.
            if days == 0 && now.time() >= time {
                days = 7;
            }
            today + chrono::Duration::days(days)
        }
    };
    target_date
        .and_time(time)
        .and_local_timezone(Local)
        .single()
}

/// Parse a three-letter weekday abbreviation (case-insensitive) into a chrono
/// [`Weekday`]. Returns `None` for anything else.
fn parse_weekday(s: &str) -> Option<Weekday> {
    match s.to_lowercase().as_str() {
        "mon" => Some(Weekday::Mon),
        "tue" => Some(Weekday::Tue),
        "wed" => Some(Weekday::Wed),
        "thu" => Some(Weekday::Thu),
        "fri" => Some(Weekday::Fri),
        "sat" => Some(Weekday::Sat),
        "sun" => Some(Weekday::Sun),
        _ => None,
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

    // Precondition: a clean tree, checked before any mutation (our own `.gitignore`
    // edit included) so a first run can never trip this.
    if !git::is_clean_ignoring_ralphy(repo)? {
        bail!(
            "working tree at {} is not clean — commit or stash first",
            repo.display()
        );
    }

    let orig = git::current_branch(repo)?;
    if orig == "HEAD" {
        bail!(
            "repo at {} is in detached HEAD — checkout a branch first",
            repo.display()
        );
    }

    let prepared = match mode {
        BranchMode::Current => {
            // Commit straight onto the current branch — no new branch, base
            // ignored. Compare against where this branch stood before the run.
            let compare_ref = git::head_sha(repo)?;
            info!(branch = %orig, "running in place on current branch");
            (orig.clone(), orig, compare_ref)
        }
        BranchMode::New => {
            if !git::commitish_exists(repo, base_branch) {
                bail!("base branch '{base_branch}' not found");
            }
            let branch = format!("afk/run-{stamp}");
            git::checkout_new_branch(repo, &branch, base_branch)?;
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
    gitignore::ensure_ralphy_ignored(repo)?;

    Ok(prepared)
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
    /// When true, a usage limit stops the run and reports the reset (the old
    /// behaviour). The default (`false`) waits for the reset and auto-resumes the
    /// same issue. See docs/adr/0003.
    pub stop_on_limit: bool,
}

/// What happened to one issue in the queue.
#[derive(Debug)]
pub struct IssueResult {
    pub number: u64,
    /// The execution outcome, or `None` when the issue was skipped (infeasible
    /// plan, blocked, or dry run).
    pub outcome: Option<Outcome>,
    /// Whether the runner closed the issue (the cycle). Only ever true for a
    /// green, non-dry-run issue.
    pub closed: bool,
    /// Open blocker issue numbers that caused this issue to be skipped. Empty
    /// when the issue was not blocked.
    pub blocked_by: Vec<u64>,
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

    'queue: for issue in queue {
        // Don't start a new issue past the global budget. Work already committed
        // for earlier issues is kept; the branch is handed back as it stands.
        if clock.deadline_passed() {
            // consumed by the telegram notifier / presenter — keep stable
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
            // consumed by the telegram notifier / presenter — keep stable
            info!(
                number = issue.number,
                "stop-before label — halting run before this issue"
            );
            stop = Some(StopReason::StopBefore {
                number: issue.number,
            });
            break;
        }

        // Blocked-by gate: skip any issue whose declared blockers are still open.
        // Checked before write_issue_json so a blocked issue never touches the
        // planner. is_closed errors are fatal (the tracker is authoritative).
        let open_blockers: Vec<u64> = {
            let refs = blocked::parse_blocked_by(&issue.body);
            let mut open = Vec::new();
            for n in refs {
                match tracker.is_closed(n) {
                    Ok(true) => {}
                    Ok(false) => open.push(n),
                    Err(e) => {
                        restore(repo, &orig, &branch, &cfg.base_branch, cfg.branch_mode);
                        return Err(e);
                    }
                }
            }
            open
        };
        if !open_blockers.is_empty() {
            // consumed by the telegram notifier / presenter — keep stable
            info!(
                number = issue.number,
                blockers = ?open_blockers,
                "blocked by open issue(s) — skipping"
            );
            worked.push(IssueResult {
                number: issue.number,
                outcome: None,
                closed: false,
                blocked_by: open_blockers,
            });
            continue;
        }

        // consumed by the telegram notifier / presenter — keep stable
        info!(number = issue.number, title = %issue.title, "issue started");

        // Persist the current issue where the planner reads it. The adapter's
        // prompt reads `.ralphy/issue.json`, so the loop must refresh it before
        // each plan — `.ralphy/` is gitignored and survives the branch checkout.
        if let Err(e) = write_issue_json(&ws, issue) {
            restore(repo, &orig, &branch, &cfg.base_branch, cfg.branch_mode);
            return Err(e);
        }

        // Plan, auto-resuming through usage-limit reset windows the same way
        // execution does. A usage limit during planning surfaces as a typed
        // `PlanLimit` (not a generic failure): wait for the reset and re-plan,
        // unless `stop_on_limit`, no reset was parsed, or repeated no-progress
        // limits hit the cap — any of which stops and reports the limit. A
        // genuine (non-limit) planning failure still restores and propagates.
        let mut plan_limit_streak = 0u32;
        let plan = loop {
            let e = match agent.plan(issue, &ws) {
                Ok(p) => break p,
                Err(e) => e,
            };
            let limit = match e.downcast::<PlanLimit>() {
                Ok(limit) => limit,
                Err(e) => {
                    restore(repo, &orig, &branch, &cfg.base_branch, cfg.branch_mode);
                    return Err(e);
                }
            };

            plan_limit_streak += 1;
            let capped = plan_limit_streak > MAX_PLAN_LIMIT_RESUMES;
            // Stop-and-report when configured, when no reset was parsed (nothing
            // to wait for), or when the cap is hit — never delete the branch, so
            // it is handed back exactly like an execute-time limit stop.
            if cfg.stop_on_limit || limit.reset.is_none() || capped {
                info!(
                    number = issue.number,
                    reset = ?limit.reset,
                    "usage limit while planning — stopping run"
                );
                worked.push(IssueResult {
                    number: issue.number,
                    outcome: Some(Outcome::Limit(limit.reset.clone())),
                    closed: false,
                    blocked_by: Vec::new(),
                });
                stop = Some(StopReason::Limit {
                    number: issue.number,
                    reset: limit.reset,
                });
                break 'queue;
            }

            // Deadline beats resume: a reset past the deadline stops the run.
            let reset = limit.reset.expect("reset present: checked above");
            if clock.wait_for_reset(&reset) == WaitOutcome::DeadlinePassed {
                info!(
                    number = issue.number,
                    "deadline beats resume while planning — stopping run"
                );
                stop = Some(StopReason::Deadline);
                break 'queue;
            }
            // Otherwise loop: re-plan after the reset window.
        };
        // consumed by the telegram notifier / presenter — keep stable
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
                blocked_by: Vec::new(),
            });
            continue;
        }

        // A dry run plans only — it executes nothing and closes nothing.
        if cfg.dry_run {
            worked.push(IssueResult {
                number: issue.number,
                outcome: None,
                closed: false,
                blocked_by: Vec::new(),
            });
            continue;
        }

        // Execute the issue, auto-resuming through usage-limit reset windows by
        // default. On `Outcome::Limit` with a parsed reset (and not
        // `stop_on_limit`), wait for the reset and re-run `execute()` only —
        // never `plan()`, which would delete the on-disk `plan.md` the resume
        // depends on (ADR-0003). A progress-aware cap abandons the issue after
        // two consecutive limit outcomes that commit nothing; any commit resets
        // the streak. The cap is checked *before* the next wait so a stalled
        // issue is abandoned without first burning another reset window.
        let mut no_commit_streak = 0u32;
        let mut deadline_cut = false;
        let outcome = loop {
            let before_sha = git::head_sha(repo).unwrap_or_default();
            let outcome = agent.execute(&plan, &ws)?;
            let after_sha = git::head_sha(repo).unwrap_or_default();

            // Track progress: a commit resets the streak, a no-commit execute
            // advances it. Done/non-limit outcomes break below before it matters.
            if before_sha != after_sha {
                no_commit_streak = 0;
            } else {
                no_commit_streak += 1;
            }

            let reset = match &outcome {
                Outcome::Limit(Some(r)) if !cfg.stop_on_limit => r.clone(),
                // Done, any non-limit outcome, a bare `Limit(None)`, or
                // `stop_on_limit` all leave the loop with the outcome as-is.
                _ => break outcome,
            };

            // Progress-aware cap: two consecutive no-commit limits abandon the
            // issue. Checked before waiting so a stuck issue gives up at once.
            if no_commit_streak >= 2 {
                info!(
                    number = issue.number,
                    "progress-aware cap reached — abandoning issue"
                );
                break outcome;
            }

            // Deadline beats resume: a reset beyond the deadline, or a deadline
            // already/just passed, stops the run instead of waiting.
            if clock.wait_for_reset(&reset) == WaitOutcome::DeadlinePassed {
                info!(
                    number = issue.number,
                    "deadline beats resume — stopping the run"
                );
                deadline_cut = true;
                break outcome;
            }
            // Otherwise loop: re-run execute() against the same on-disk plan.md.
        };

        if outcome == Outcome::Done {
            // Close the cycle: a green queue issue is closed so it leaves the
            // queue; its labels are untouched and the branch is merged by hand.
            tracker.close(issue.number, &close_comment(&cfg.stamp, &branch))?;

            // Record the closed issue before writing evidence so the result is
            // always present in the report even if write_evidence errors out.
            // consumed by the telegram notifier / presenter — keep stable
            info!(number = issue.number, "green — issue closed");
            worked.push(IssueResult {
                number: issue.number,
                outcome: Some(Outcome::Done),
                closed: true,
                blocked_by: Vec::new(),
            });

            // Write acceptance evidence when the plan carries a ledger. A
            // missing or empty ledger is a graceful no-op.
            if let Ok(plan_md) = std::fs::read_to_string(ws.plan_path()) {
                let verdicts = acceptance::parse_ledger(&plan_md);
                if !verdicts.is_empty() {
                    tracker.write_evidence(issue.number, &issue.body, &verdicts)?;
                }
            }

            continue;
        }

        // Any non-green outcome stops the whole run; later issues are untouched.
        // consumed by the telegram notifier / presenter — keep stable
        info!(number = issue.number, ?outcome, "non-green — stopping run");
        let number = issue.number;
        worked.push(IssueResult {
            number,
            outcome: Some(outcome.clone()),
            closed: false,
            blocked_by: Vec::new(),
        });
        stop = Some(if deadline_cut {
            StopReason::Deadline
        } else {
            match outcome {
                Outcome::Limit(reset) => StopReason::Limit { number, reset },
                other => StopReason::NonGreen {
                    number,
                    outcome: other,
                },
            }
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
    // Force: the run branch may carry the uncommitted `.gitignore` edit (a dry run
    // never commits it), which must be discarded rather than dragged onto `orig`.
    if let Err(e) = git::checkout_force(repo, orig) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    /// Build a fixed `DateTime<Local>` for deterministic `next_reset` tests.
    fn at(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> DateTime<Local> {
        Local.with_ymd_and_hms(y, mo, d, h, mi, 0).single().unwrap()
    }

    #[test]
    fn next_reset_same_day_past_rolls_to_tomorrow() {
        // 2026-06-09 is a Tuesday. Now 16:00, bare reset 15:00 already past today.
        let now = at(2026, 6, 9, 16, 0);
        let got = next_reset("15:00", now).unwrap();
        assert_eq!(
            got,
            at(2026, 6, 10, 15, 0),
            "bare past time rolls to tomorrow"
        );
    }

    #[test]
    fn next_reset_future_same_day_stays_today() {
        let now = at(2026, 6, 9, 10, 0);
        let got = next_reset("15:00", now).unwrap();
        assert_eq!(got, at(2026, 6, 9, 15, 0), "bare future time stays today");
    }

    #[test]
    fn next_reset_weekday_picks_next_matching_date() {
        // Now is Tuesday 2026-06-09; the next Friday is 2026-06-12.
        let now = at(2026, 6, 9, 10, 0);
        let got = next_reset("Fri 09:00", now).unwrap();
        assert_eq!(
            got,
            at(2026, 6, 12, 9, 0),
            "weekday picks the next matching date"
        );
    }

    #[test]
    fn next_reset_same_weekday_past_rolls_a_week() {
        // Today is Tuesday; a Tuesday reset already past today lands next Tuesday.
        let now = at(2026, 6, 9, 16, 0);
        let got = next_reset("Tue 15:00", now).unwrap();
        assert_eq!(
            got,
            at(2026, 6, 16, 15, 0),
            "same weekday past rolls a week"
        );
    }

    #[test]
    fn next_reset_unparseable_is_none() {
        let now = at(2026, 6, 9, 10, 0);
        assert_eq!(next_reset("not a time", now), None);
    }
}
