//! The run lifecycle: cut a fresh branch off the base, ask the agent to plan,
//! and (on a dry run) return the repo to where it started, dropping the empty
//! run branch. Execution is a later slice; this slice stops after planning.

use std::collections::BTreeMap;
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use chrono::{DateTime, Datelike, Local, NaiveTime, Weekday};
use tracing::{info, warn};

use crate::{
    acceptance, blocked, git, gitignore, handoff,
    ledger::{self, LedgerRecord},
    references,
    verify::{self, VerifySpec},
    Agent, Execution, Issue, IssueTracker, Outcome, PlanLimit, Usage, Workspace,
};

/// Consecutive plan-time usage limits that make no progress before the runner
/// gives up and stops-and-reports. Guards a past or unparseable reset hint from
/// spinning the resume loop, mirroring the execute-path no-commit cap.
const MAX_PLAN_LIMIT_RESUMES: u32 = 2;

/// Upper bound on a single usage-limit wait. A reset hint that resolves farther
/// out than this is treated as a stop (`DeadlinePassed`) rather than parked on —
/// guards against a malformed/hostile hint parking the run for the unbounded
/// issue horizon (~365 days) when no run-level deadline is set.
const MAX_RESET_WAIT: Duration = Duration::from_secs(12 * 60 * 60);

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
        // A reset farther out than the max wait is a stop, regardless of whether a
        // run deadline is set — without this, a hint resolving to the unbounded
        // issue horizon would park the run for ~365 days.
        if target - Local::now()
            > chrono::Duration::from_std(MAX_RESET_WAIT)
                .unwrap_or_else(|_| chrono::Duration::hours(12))
        {
            info!(%reset, "reset lands beyond the max wait — not waiting");
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
/// one of: an absolute RFC3339 instant (`"2026-06-09T18:00:00Z"`, as Codex emits),
/// a bare `"HH:mm"`, or a weekday-qualified `"Wkd HH:mm"` (the relative forms Claude
/// emits). An absolute instant is unambiguous and used as-is — `now` is ignored. A
/// bare time resolves to today, rolled to tomorrow when already past `now`; a
/// weekday-qualified time resolves to the next date carrying that weekday (today
/// only when the time is still ahead, else next week). Pure over its inputs so the
/// rollover edge cases unit-test without sleeping. Returns `None` on an
/// unparseable hint.
fn next_reset(reset: &str, now: DateTime<Local>) -> Option<DateTime<Local>> {
    // Strip trailing sentence punctuation an adapter may leave on the hint (e.g.
    // Codex's "… Try again at 2026-06-09T18:00:00Z.").
    let trimmed = reset.trim().trim_end_matches('.').trim();

    // An absolute RFC3339 instant is unambiguous (carries its own date and zone):
    // use it directly, converted to local time. No next-occurrence guess is needed,
    // unlike the relative forms handled below. Try the whole hint, then its leading
    // token — the datetime may be trailed by prose ("…Z (in 3 hours)"). The relative
    // forms never parse as RFC3339 ("Fri"/"15:00" both fail), so this stays additive.
    let leading = trimmed.split_whitespace().next().unwrap_or(trimmed);
    for cand in [trimmed, leading.trim_end_matches('.')] {
        if let Ok(dt) = DateTime::parse_from_rfc3339(cand) {
            return Some(dt.with_timezone(&Local));
        }
    }

    let (weekday, hhmm) = match trimmed.split_once(char::is_whitespace) {
        Some((wd, rest)) => (Some(parse_weekday(wd.trim())?), rest.trim()),
        // Use `trimmed` (trailing punctuation already stripped), not the raw
        // `reset`, so a bare hint like "15:00." parses instead of failing.
        None => (None, trimmed),
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
    info!(
        open_steps = plan.open_steps,
        up = plan.usage.input,
        cr = plan.usage.cache_read,
        cw = plan.usage.cache_creation,
        out = plan.usage.output,
        model = plan.usage.model.as_deref().unwrap_or(""),
        "plan written"
    );

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

    let Execution { outcome, usage: _ } = agent.execute(&plan, &ws)?;
    Ok(RunReport {
        branch,
        orig_branch: orig,
        outcome: RunOutcome::Executed(outcome),
    })
}

/// The label that pauses the run before the tagged issue (flow-control, not triage).
pub const STOP_BEFORE_LABEL: &str = "stop-before";

/// The label applied to an issue the planner judged a bundle (multiple backlog
/// tasks under one number): the queue is parked on a human running `/to-issues`
/// to open the children (`## Parent: #N`) and close the bundle — the
/// follow-the-split blocker gate handles the rest.
const NEEDS_SPLIT_LABEL: &str = "needs-split";

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
    /// When true, a usage limit during the *plan* phase stops the run and reports
    /// the reset (the old behaviour). The default (`false`) waits for the reset
    /// and auto-resumes the same issue. Derived from the planner agent so a split
    /// run can resume through a plan-time reset while still stopping on an
    /// execute-time limit. See docs/adr/0003 and docs/adr/0009.
    pub stop_on_limit_plan: bool,
    /// When true, a usage limit during the *execute* phase stops the run and
    /// reports the reset. The default (`false`) waits and auto-resumes. Derived
    /// from the executor agent. See docs/adr/0003 and docs/adr/0009.
    pub stop_on_limit_exec: bool,
    /// The per-repo fallback verify command(s) resolved from `settings.json`
    /// `verify.command` (ADR-0011). Used only when a plan's `## Verify` section
    /// is *absent or empty* (`VerifySpec::Unspecified`); a plan's own commands
    /// take precedence and `## Verify: none` skips this fallback. `None` here
    /// means no per-repo default — an unspecified plan then closes on the agent's
    /// self-report with a loud warning.
    pub verify_fallback: Option<Vec<Vec<String>>>,
    /// The bounded time budget for one issue's verify gate (ADR-0011). A gate
    /// that runs past it is killed and counts as a failure. Derived from
    /// `--max-minutes-per-issue`.
    pub verify_timeout: Duration,
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
    /// The token usage this run consumed across every phase it worked — the sum
    /// of each plan and execute [`Usage`] (ADR-0008). The console footer's run
    /// total (D11) reads off it.
    pub run_usage: Usage,
    /// The run's token usage split **per model** (keyed by the phase's `model`, or
    /// `unknown` when the adapter captured none). The footer's read-time USD (D8)
    /// needs this split because price resolves per model — `run_usage` alone cannot
    /// be priced once a run mixes models.
    pub run_usage_by_model: BTreeMap<String, Usage>,
}

/// The terminal-status label written to the ledger's `outcome` field (ADR-0008
/// D6), one of `done`/`blocked`/`timeout`/`stuck`/`limit`. A read-time report
/// joins it with the plan line by `issue` to ask "what fraction of tokens bought
/// a `done`?".
fn outcome_label(outcome: &Outcome) -> &'static str {
    match outcome {
        Outcome::Done => "done",
        Outcome::Blocked(_) => "blocked",
        Outcome::Timeout => "timeout",
        Outcome::Stuck => "stuck",
        Outcome::Limit(_) => "limit",
    }
}

/// Fold one phase's [`Usage`] into a per-model accumulator, keyed by its `model`
/// (or `unknown` when the adapter captured none). The read-time USD footer (D8)
/// needs this split because price resolves per model.
fn accumulate_by_model(by_model: &mut BTreeMap<String, Usage>, usage: &Usage) {
    by_model
        .entry(usage.model.clone().unwrap_or_else(|| "unknown".into()))
        .or_default()
        .add_tokens(usage);
}

/// The close comment the runner leaves on a green queue issue. Mirrors the ps1
/// oracle so overnight-run behaviour is unchanged.
fn close_comment(stamp: &str, branch: &str) -> String {
    format!("Closed by Ralphy run {stamp} (green on branch '{branch}'; merge by hand).")
}

/// The comment posted when the planner judges an issue infeasible: the verdict
/// plus the planner's reasoning, so the skip is actionable from the issue
/// itself (split it, respecify it) instead of silent.
fn infeasible_comment(stamp: &str, reason: &str) -> String {
    format!(
        "Ralphy run {stamp} skipped this issue — the planner judged it not \
         autonomously implementable as written.\n\n## Planner reasoning\n\n{reason}\n\n\
         The issue stays open; act on the reasoning above (split, respecify, or \
         label) and the next run will pick it up again."
    )
}

/// The comment posted on a bundle verdict: unlike a generic infeasible skip,
/// the issue is well-specified but covers several backlog tasks, so the next
/// step is a human split — spelled out so the parked queue has an owner.
fn bundle_comment(stamp: &str, reason: &str) -> String {
    format!(
        "Ralphy run {stamp} skipped this issue — the planner judged it a \
         **bundle**: several backlog tasks under one issue number. The queue is \
         parked on this until it is split.\n\n## Planner reasoning\n\n{reason}\n\n\
         Next step (human): run `/to-issues` against the source PRD using the \
         split recommended above as a draft, open one child issue per task with \
         a `## Parent` reference to this issue, then close this issue — \
         dependents follow the open children automatically."
    )
}

/// What the runner-enforced verify gate resolves to for one issue (ADR-0011),
/// folding the plan's `## Verify` section with the per-repo settings fallback.
enum VerifyPlan {
    /// Run these commands as the gate.
    Run(Vec<Vec<String>>),
    /// The plan opted out with `## Verify: none` — close on the self-report, no
    /// warning (the absence of verification was a deliberate, visible decision).
    OptedOut,
    /// Nothing resolved — no plan section and no settings fallback. Close on the
    /// agent's self-report but warn loudly (no-silent-caps: a missing gate is
    /// always a visible decision, never a silent hole).
    NoGate,
}

/// Apply the ADR-0011 resolution precedence: a plan's `## Verify` commands win;
/// `## Verify: none` is the explicit opt-out; an absent/empty section falls back
/// to the per-repo `settings.json` `verify.command`, and if that is unset too the
/// issue closes on the self-report with a loud warning.
fn resolve_verify(plan_md: &str, fallback: &Option<Vec<Vec<String>>>) -> VerifyPlan {
    match verify::parse_verify(plan_md) {
        VerifySpec::Commands(commands) => VerifyPlan::Run(commands),
        VerifySpec::None => VerifyPlan::OptedOut,
        VerifySpec::Unspecified => match fallback {
            Some(commands) if !commands.is_empty() => VerifyPlan::Run(commands.clone()),
            _ => VerifyPlan::NoGate,
        },
    }
}

/// How many times a failed verify gate is handed back to the agent to repair
/// before the runner gives up and stops the run (ADR-0011 amendment). The gate
/// stays the authority across every attempt — a repair earns the close only by
/// making the runner *see* the same commands pass; the budget just bounds how
/// long the agent gets to react before the branch is handed back for a human.
const VERIFY_MAX_REPAIRS: u32 = 2;

/// The scratch file the runner drops in the workspace to hand a failed gate back
/// to the executor (read by the exec charter's repair clause). Vendor-neutral —
/// the runner writes it, any adapter's prompt reads it. Cleared once the gate
/// goes green so it never bleeds into a later run on the same worktree.
const VERIFY_FAILURE_FILE: &str = "verify-failure.md";

/// Write the repair brief for a failed gate so the next `execute()` can read why
/// it failed and fix the root cause. Best-effort: a write failure just means the
/// agent retries blind, which is strictly no worse than not repairing at all.
fn write_verify_failure(ws: &Workspace, stamp: &str, report: &verify::VerifyReport) {
    let path = ws.ralphy_dir().join(VERIFY_FAILURE_FILE);
    if let Err(e) = std::fs::write(&path, verify::repair_brief(stamp, report)) {
        warn!(error = %e, "writing the verify-failure repair brief failed");
    }
}

/// Remove the repair brief. Called when the gate passes and at each issue's start
/// so the file only ever reflects the current run's gate state. Absent file is a
/// no-op.
fn clear_verify_failure(ws: &Workspace) {
    let path = ws.ralphy_dir().join(VERIFY_FAILURE_FILE);
    if path.exists() {
        if let Err(e) = std::fs::remove_file(&path) {
            warn!(error = %e, "removing the stale verify-failure brief failed");
        }
    }
}

/// A one-line digest of a failed gate for the skip log/artifact: the failing
/// command and why it failed (exit code or timeout).
fn verify_failure_summary(report: &verify::VerifyReport) -> String {
    match report.commands.iter().find(|c| !c.passed()) {
        Some(c) if c.timed_out => format!("`{}` timed out", c.argv.join(" ")),
        Some(c) => format!(
            "`{}` exited {}",
            c.argv.join(" "),
            c.exit_code
                .map(|n| n.to_string())
                .unwrap_or_else(|| "non-zero".into())
        ),
        None => "verify gate failed".into(),
    }
}

/// Refresh `.ralphy/handoffs.md` for the issue about to be planned: collect the
/// handoff comments its closed blockers left, render them, and write the file —
/// or remove a stale one when there is nothing to feed. Best-effort: a fetch
/// failure logs a warning and skips that blocker, never stopping the run.
fn write_handoffs(
    ws: &Workspace,
    number: u64,
    closed_blockers: &[u64],
    tracker: &dyn IssueTracker,
) {
    let mut entries: Vec<(u64, String)> = Vec::new();
    for &n in closed_blockers {
        match tracker.handoff_comment(n) {
            Ok(Some(h)) => entries.push((n, h)),
            Ok(None) => {}
            Err(e) => warn!(number, blocker = n, error = %e, "fetching handoff failed — skipping"),
        }
    }
    let path = ws.handoffs_path();
    match handoff::render_handoffs_file(&entries) {
        Some(content) => {
            if let Err(e) = std::fs::write(&path, content) {
                warn!(number, error = %e, "writing .ralphy/handoffs.md failed");
            } else {
                info!(
                    number,
                    handoffs = entries.len(),
                    "handoffs collected for planner"
                );
            }
        }
        None => {
            let _ = std::fs::remove_file(&path);
        }
    }
}

/// Refresh `.ralphy/references.md` for the issue about to be planned: fetch the
/// source (title, state, body) of every issue named in its `## Blocked by` and
/// `## Parent` sections and render them, or remove a stale file when the issue
/// names none. The planner reads this so a `#N` reference reaches it as the
/// referenced issue's actual spec, not a paraphrase it might restate as fact in
/// a child issue. Best-effort and depth-1: a fetch failure (a cross-repo or
/// deleted ref, say) logs a warning and skips that ref, and the fetched bodies'
/// own references are not followed transitively.
fn write_references(ws: &Workspace, issue: &Issue, tracker: &dyn IssueTracker) {
    let refs = blocked::structured_refs(&issue.body, issue.number);
    let mut entries: Vec<references::Reference> = Vec::new();
    for n in refs {
        match tracker.reference(n) {
            Ok(Some(r)) => entries.push(r),
            Ok(None) => {}
            Err(e) => {
                warn!(number = issue.number, reference = n, error = %e, "fetching referenced issue failed — skipping")
            }
        }
    }
    let path = ws.references_path();
    match references::render_references_file(&entries) {
        Some(content) => {
            if let Err(e) = std::fs::write(&path, content) {
                warn!(number = issue.number, error = %e, "writing .ralphy/references.md failed");
            } else {
                info!(
                    number = issue.number,
                    references = entries.len(),
                    "references collected for planner"
                );
            }
        }
        None => {
            let _ = std::fs::remove_file(&path);
        }
    }
}

/// Persist the durable knowledge a green close leaves behind: the environment
/// facts and working commands extracted from the plan's `## Handoff`, written
/// to `.ralphy/knowledge/issue-<N>.md`. The folder accumulates across issues
/// and runs (never cleared), so any future session — sibling or dependent —
/// can grep it instead of re-deriving an environment procedure. Best-effort:
/// a write failure logs a warning, never stopping the run.
fn write_knowledge(ws: &Workspace, issue: &Issue, stamp: &str, note: &str) {
    let dir = ws.knowledge_dir();
    if let Err(e) = std::fs::create_dir_all(&dir) {
        warn!(number = issue.number, error = %e, "creating .ralphy/knowledge failed");
        return;
    }
    let content = format!(
        "# Knowledge from #{}: {}\n\nExtracted {} (run {}) from the session's \
         handoff at close. Leads, not truths — verify before relying on one.\n\n{}\n",
        issue.number,
        issue.title,
        chrono::Local::now().format("%Y-%m-%d"),
        stamp,
        note.trim_end(),
    );
    let path = ws.knowledge_path(issue.number);
    if let Err(e) = std::fs::write(&path, content) {
        warn!(number = issue.number, error = %e, "writing knowledge note failed");
    } else {
        info!(number = issue.number, path = %path.display(), "knowledge note written");
    }
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

    // Identity for every ledger line this run writes (ADR-0008 D6/D7), read once
    // from git: the project slug (remote, or a path-hash fallback) and the actor.
    let project = git::project_slug(repo);
    let actor_email = git::user_email(repo).unwrap_or_default();
    let actor_name = git::user_name(repo).unwrap_or_default();

    let mut worked: Vec<IssueResult> = Vec::new();
    let mut stop: Option<StopReason> = None;
    let mut run_usage = Usage::default();
    // Per-model token accumulation for the read-time USD footer (D8): keyed by the
    // phase's resolved model, summed across every phase the run worked.
    let mut run_usage_by_model: BTreeMap<String, Usage> = BTreeMap::new();

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
        // Closed blockers are kept: they are the handoff sources below.
        //
        // A closed blocker can be a retired bundle whose work was split into
        // child issues (their `## Parent` references it). Closing the bundle
        // does not finish its work — the gate follows the split: while any
        // child is open, the dependent stays blocked on those children.
        let refs = blocked::parse_blocked_by(&issue.body);
        let mut open_blockers: Vec<u64> = Vec::new();
        let mut closed_blockers: Vec<u64> = Vec::new();
        for n in refs {
            match tracker.is_closed(n) {
                Ok(true) => match tracker.open_children(n) {
                    Ok(children) if children.is_empty() => closed_blockers.push(n),
                    Ok(children) => {
                        info!(
                            number = issue.number,
                            blocker = n,
                            children = ?children,
                            "blocker closed but split into open children — still blocking"
                        );
                        open_blockers.extend(children);
                    }
                    Err(e) => {
                        restore(repo, &orig, &branch, &cfg.base_branch, cfg.branch_mode);
                        return Err(e);
                    }
                },
                Ok(false) => open_blockers.push(n),
                Err(e) => {
                    restore(repo, &orig, &branch, &cfg.base_branch, cfg.branch_mode);
                    return Err(e);
                }
            }
        }
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

        // Attach the issue's own comment thread so the planner and executor read
        // the discussion alongside the body — guidance, clarifications, and prior
        // attempts a human left in comments rather than in the original body.
        // Best-effort enrichment: a fetch failure is a warning, never a stop, and
        // planning proceeds with the body alone. The queue's issue carries no
        // comments (the list query omits them), so this clone is where they land.
        let mut issue = issue.clone();
        match tracker.issue_comments(issue.number) {
            Ok(comments) => {
                if !comments.is_empty() {
                    info!(
                        number = issue.number,
                        comments = comments.len(),
                        "comments attached for planner"
                    );
                }
                issue.comments = comments;
            }
            Err(e) => {
                warn!(number = issue.number, error = %e, "fetching issue comments failed — planning with body only")
            }
        }
        let issue = &issue;

        // Persist the current issue where the planner reads it. The adapter's
        // prompt reads `.ralphy/issue.json`, so the loop must refresh it before
        // each plan — `.ralphy/` is gitignored and survives the branch checkout.
        if let Err(e) = write_issue_json(&ws, issue) {
            restore(repo, &orig, &branch, &cfg.base_branch, cfg.branch_mode);
            return Err(e);
        }

        // Shoulders of giants: collect the handoffs the closed blockers left on
        // their issues into `.ralphy/handoffs.md`, where the planner reads them
        // as predecessor context. Best-effort enrichment — a fetch failure is a
        // warning, never a stop — but the file is always refreshed (or removed)
        // so a previous issue's handoffs never leak into this one.
        write_handoffs(&ws, issue.number, &closed_blockers, tracker);

        // Reproduce the source of the issues this one references in its
        // `## Blocked by` / `## Parent` sections into `.ralphy/references.md`, so
        // the planner reads the referenced spec at source rather than restating a
        // `#N` mention as fact in a child issue. Best-effort like the handoffs.
        write_references(&ws, issue, tracker);

        // Plan, auto-resuming through usage-limit reset windows the same way
        // execution does. A usage limit during planning surfaces as a typed
        // `PlanLimit` (not a generic failure): wait for the reset and re-plan,
        // unless `stop_on_limit_plan`, no reset was parsed, or repeated no-progress
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
            if cfg.stop_on_limit_plan || limit.reset.is_none() || capped {
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
            up = plan.usage.input,
            cr = plan.usage.cache_read,
            cw = plan.usage.cache_creation,
            out = plan.usage.output,
            model = plan.usage.model.as_deref().unwrap_or(""),
            "plan written"
        );

        // Record the plan phase's token usage (ADR-0008 D6). Written before the
        // feasibility branch so even an infeasible plan's planning cost is on the
        // ledger. The plan line carries `ok` — the issue's terminal outcome is its
        // execute line's, joined by `issue` at read-time. Best-effort: a write
        // failure warns, never stops the run (D9).
        let plan_rec = LedgerRecord {
            project: project.clone(),
            actor_email: actor_email.clone(),
            actor_name: actor_name.clone(),
            ralphy_version: env!("CARGO_PKG_VERSION").into(),
            issue: issue.number,
            phase: "plan".into(),
            agent: agent.name().into(),
            model: plan.usage.model.clone().unwrap_or_else(|| "unknown".into()),
            outcome: "ok".into(),
            tokens: plan.usage.clone(),
            ts: chrono::Utc::now().to_rfc3339(),
        };
        if let Err(e) = ledger::append(&plan_rec) {
            warn!(number = issue.number, error = %e, "writing plan usage ledger line failed");
        }
        run_usage.add_tokens(&plan.usage);
        accumulate_by_model(&mut run_usage_by_model, &plan.usage);

        // An infeasible plan (no actionable steps) is a skip, not a failure, and
        // not green — the runner neither closes it nor stops the run. The
        // planner's reasoning is posted on the issue so the verdict is
        // actionable instead of dying in the gitignored plan.md.
        if !plan.is_feasible() {
            if let Ok(plan_md) = std::fs::read_to_string(ws.plan_path()) {
                if let Some(reason) = handoff::infeasible_reason(&plan_md) {
                    if handoff::is_bundle_reason(&reason) {
                        // consumed by the telegram notifier / presenter — keep stable
                        info!(number = issue.number, "bundle plan — needs split");
                        // Best-effort: a label failure must not stop the run —
                        // the comment below still carries the verdict.
                        if let Err(e) = tracker.add_label(issue.number, NEEDS_SPLIT_LABEL) {
                            warn!(number = issue.number, error = %e, "applying needs-split label failed");
                        }
                        // Best-effort: a failed verdict comment must not abort
                        // the queue over a non-green skip.
                        if let Err(e) =
                            tracker.comment(issue.number, &bundle_comment(&cfg.stamp, &reason))
                        {
                            warn!(number = issue.number, error = %e, "posting bundle verdict comment failed");
                        }
                    } else if let Err(e) =
                        tracker.comment(issue.number, &infeasible_comment(&cfg.stamp, &reason))
                    {
                        warn!(number = issue.number, error = %e, "posting infeasible verdict comment failed");
                    }
                }
            }
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
        // `stop_on_limit_exec`), wait for the reset and re-run `execute()` only —
        // never `plan()`, which would delete the on-disk `plan.md` the resume
        // depends on (ADR-0003). A progress-aware cap abandons the issue after
        // two consecutive limit outcomes that commit nothing; any commit resets
        // the streak. The cap is checked *before* the next wait so a stalled
        // issue is abandoned without first burning another reset window.
        // Start each issue with no repair brief on disk: the gate only writes one
        // when *this* run's verify fails, so a brief left by a prior run (stopped
        // on a red gate, then resumed) never silently steers the first execute.
        clear_verify_failure(&ws);

        let mut no_commit_streak = 0u32;
        let mut deadline_cut = false;
        let mut exec_usage = Usage::default();
        let outcome = loop {
            let before_sha = git::head_sha(repo).ok();
            let Execution { outcome, usage } = agent.execute(&plan, &ws)?;
            // Accumulate across the resume loop so the single execute ledger line
            // carries the whole issue's execution cost, not just the last attempt.
            exec_usage.add_tokens(&usage);
            let after_sha = git::head_sha(repo).ok();

            // Track progress: a commit resets the streak, a no-commit execute
            // advances it. Done/non-limit outcomes break below before it matters.
            // If either SHA read failed, progress is unknown — leave the streak
            // untouched rather than collapse both errors to "" and read it as a
            // false no-commit.
            match (&before_sha, &after_sha) {
                (Some(b), Some(a)) if b != a => no_commit_streak = 0,
                (Some(_), Some(_)) => no_commit_streak += 1,
                _ => {}
            }

            let reset = match &outcome {
                Outcome::Limit(Some(r)) if !cfg.stop_on_limit_exec => r.clone(),
                // Done, any non-limit outcome, a bare `Limit(None)`, or
                // `stop_on_limit_exec` all leave the loop with the outcome as-is.
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

        // Record the execute phase's accumulated token usage with this issue's
        // terminal outcome (ADR-0008 D6). One line per issue regardless of how
        // many resume attempts ran. Best-effort (D9).
        let exec_rec = LedgerRecord {
            project: project.clone(),
            actor_email: actor_email.clone(),
            actor_name: actor_name.clone(),
            ralphy_version: env!("CARGO_PKG_VERSION").into(),
            issue: issue.number,
            phase: "execute".into(),
            agent: agent.name().into(),
            model: exec_usage.model.clone().unwrap_or_else(|| "unknown".into()),
            outcome: outcome_label(&outcome).into(),
            tokens: exec_usage.clone(),
            ts: chrono::Utc::now().to_rfc3339(),
        };
        if let Err(e) = ledger::append(&exec_rec) {
            warn!(number = issue.number, error = %e, "writing execute usage ledger line failed");
        }
        run_usage.add_tokens(&exec_usage);
        accumulate_by_model(&mut run_usage_by_model, &exec_usage);

        if outcome == Outcome::Done {
            // Runner-enforced verify gate (ADR-0011): before closing on the
            // agent's self-reported `Done`, re-run the plan's `## Verify`
            // commands over the committed state. Only a pass proceeds to the
            // close path. On a failure the runner no longer stops outright — it
            // hands the failing commands back to the agent (up to
            // `VERIFY_MAX_REPAIRS` times) and re-runs the SAME gate after each
            // attempt. The gate stays the authority: a repair earns the close
            // only by making the runner *see* the commands pass, never by a
            // fresh self-report. Only when the repair budget is exhausted does
            // the run stop with the branch handed back. `## Verify: none` opts
            // out; an absent section falls back to settings, then to a loud
            // warn-and-close.
            let plan_md = std::fs::read_to_string(ws.plan_path()).unwrap_or_default();
            // Tokens the agent spends on repairs, accounted as their own phase so
            // the initial execute line stays truthful and the repair cost is never
            // hidden (ADR-0008). Folded into the run totals below either way.
            let mut repair_usage = Usage::default();
            // Set when a repair attempt itself hits a usage limit: that is the
            // run's limit, not a verify failure, so it stops with the reset rather
            // than burning the rest of the repair budget on an agent that cannot
            // work. `None` while the gate is still being worked.
            let mut repair_limit: Option<Outcome> = None;
            // `None` once the gate is green (proceed to close); `Some(summary)`
            // when the repair budget is spent (stop, branch handed back).
            let gate_failure: Option<String> = match resolve_verify(&plan_md, &cfg.verify_fallback)
            {
                VerifyPlan::Run(commands) => {
                    let mut attempt = 0u32;
                    loop {
                        // consumed by the telegram notifier / presenter — keep stable
                        info!(
                            number = issue.number,
                            commands = commands.len(),
                            "verify gate — running"
                        );
                        let report = verify::run(&commands, repo, cfg.verify_timeout);
                        // Honesty artifact: every command + its exit code (pass or
                        // fail), with the failing tail on a failure. Best-effort — a
                        // comment failure must not crash a run that otherwise passed.
                        if let Err(e) =
                            tracker.comment(issue.number, &verify::comment(&cfg.stamp, &report))
                        {
                            warn!(number = issue.number, error = %e, "posting verify artifact comment failed");
                        }
                        if report.passed {
                            info!(number = issue.number, "verify gate passed");
                            clear_verify_failure(&ws);
                            break None;
                        }

                        let summary = verify_failure_summary(&report);
                        if attempt >= VERIFY_MAX_REPAIRS {
                            // consumed by the telegram notifier / presenter — keep stable
                            info!(
                                number = issue.number,
                                %summary,
                                attempts = attempt,
                                "verify gate failed — issue not closed"
                            );
                            break Some(summary);
                        }

                        attempt += 1;
                        info!(
                            number = issue.number,
                            %summary,
                            attempt,
                            max = VERIFY_MAX_REPAIRS,
                            "verify gate failed — handing back to the agent to repair"
                        );
                        // Hand the failure to the executor through the workspace
                        // (the same vendor-neutral channel as plan.md), then re-run
                        // execute() against the unchanged plan. The repair runs
                        // within the issue's own time budget, like every execute.
                        write_verify_failure(&ws, &cfg.stamp, &report);
                        let Execution {
                            outcome: repair_outcome,
                            usage,
                        } = agent.execute(&plan, &ws)?;
                        repair_usage.add_tokens(&usage);
                        // A usage limit mid-repair stops the run on the limit; we do
                        // not re-verify (the agent never got to fix anything) and do
                        // not spend another attempt.
                        if let Outcome::Limit(_) = repair_outcome {
                            repair_limit = Some(repair_outcome);
                            break Some(summary);
                        }
                        // Any other outcome (Done, Blocked, …) loops back to re-run
                        // the gate: the deterministic commands — not the agent's
                        // word — decide whether the repair earned the close.
                    }
                }
                VerifyPlan::OptedOut => {
                    info!(
                        number = issue.number,
                        "verify gate skipped — plan declared `## Verify: none`"
                    );
                    None
                }
                VerifyPlan::NoGate => {
                    warn!(
                        number = issue.number,
                        "issue closed without a verify gate — no `## Verify` in the plan \
                         and no settings.json verify.command resolved"
                    );
                    None
                }
            };

            // Account the repair phase before branching on the gate result, so the
            // run totals and the per-issue ledger are honest whether the gate went
            // green or the budget ran out (ADR-0008). One `repair` line per issue,
            // regardless of how many attempts ran. Best-effort.
            if repair_usage.total() > 0 {
                let repair_rec = LedgerRecord {
                    project: project.clone(),
                    actor_email: actor_email.clone(),
                    actor_name: actor_name.clone(),
                    ralphy_version: env!("CARGO_PKG_VERSION").into(),
                    issue: issue.number,
                    phase: "repair".into(),
                    agent: agent.name().into(),
                    model: repair_usage
                        .model
                        .clone()
                        .unwrap_or_else(|| "unknown".into()),
                    outcome: if gate_failure.is_none() {
                        "done"
                    } else {
                        "verify-failed"
                    }
                    .into(),
                    tokens: repair_usage.clone(),
                    ts: chrono::Utc::now().to_rfc3339(),
                };
                if let Err(e) = ledger::append(&repair_rec) {
                    warn!(number = issue.number, error = %e, "writing repair usage ledger line failed");
                }
                run_usage.add_tokens(&repair_usage);
                accumulate_by_model(&mut run_usage_by_model, &repair_usage);
            }

            if let Some(summary) = gate_failure {
                let number = issue.number;
                // A repair that hit a usage limit is the *run's* limit, not this
                // issue's fault: there are no tokens left to work the rest of the
                // queue, so stop on the reset (the same global stance the execute
                // path already takes on a limit).
                if let Some(Outcome::Limit(reset)) = repair_limit {
                    worked.push(IssueResult {
                        number,
                        outcome: Some(Outcome::Limit(reset.clone())),
                        closed: false,
                        blocked_by: Vec::new(),
                    });
                    stop = Some(StopReason::Limit { number, reset });
                    break;
                }
                // A verify failure no longer halts the queue: the repair budget is
                // spent, so leave THIS issue open (its commits stay on the branch
                // for a human to pick up — see the artifact comment) and march on
                // to the next issue. The issue is reported skipped-on-verify so the
                // miss is visible, never a silent close.
                // consumed by the telegram notifier / presenter — keep stable
                info!(number, %summary, "verify gate failed — skipping issue");
                worked.push(IssueResult {
                    number,
                    outcome: None,
                    closed: false,
                    blocked_by: Vec::new(),
                });
                continue;
            }

            // Close the cycle: a green queue issue is closed so it leaves the
            // queue; its labels are untouched and the branch is merged by hand.
            tracker.close(issue.number, &close_comment(&cfg.stamp, &branch))?;

            // Record the closed issue before writing evidence so the result is
            // always present in the report even if write_evidence errors out.
            // consumed by the telegram notifier / presenter — keep stable. The
            // `tokens` field carries the issue's total (plan + execute + repair)
            // so the live UI can show inline per-issue tokens (ADR-0008 D11).
            let issue_total = plan.usage.total() + exec_usage.total() + repair_usage.total();
            // `tokens` stays for the telegram notifier (keep stable); `up/cr/cw/out`
            // carry the *execution* phase breakdown so the live UI can combine it
            // with the planning usage it stashed at `plan written` (ADR-0008 D11).
            info!(
                number = issue.number,
                tokens = issue_total,
                up = exec_usage.input,
                cr = exec_usage.cache_read,
                cw = exec_usage.cache_creation,
                out = exec_usage.output,
                model = exec_usage.model.as_deref().unwrap_or(""),
                "green — issue closed"
            );
            worked.push(IssueResult {
                number: issue.number,
                outcome: Some(Outcome::Done),
                closed: true,
                blocked_by: Vec::new(),
            });

            // Write acceptance evidence when the plan carries a ledger, and
            // publish the session's handoff + plan friction so successors (and
            // dependent issues' planners) inherit what this session learned. A
            // missing or empty ledger/handoff is a graceful no-op.
            if let Ok(plan_md) = std::fs::read_to_string(ws.plan_path()) {
                let verdicts = acceptance::parse_ledger(&plan_md);
                if !verdicts.is_empty() {
                    tracker.write_evidence(issue.number, &issue.body, &verdicts)?;
                }
                if let Some(report) = handoff::close_report(&plan_md) {
                    tracker.comment(issue.number, &report)?;
                }
                if let Some(note) = handoff::knowledge_note(&plan_md) {
                    write_knowledge(&ws, issue, &cfg.stamp, &note);
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
        run_usage,
        run_usage_by_model,
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
    fn accumulate_by_model_splits_and_sums_per_model_with_unknown_fallback() {
        let mut by_model: BTreeMap<String, Usage> = BTreeMap::new();
        let opus = |i| Usage {
            input: i,
            output: 0,
            cache_read: 0,
            cache_creation: 0,
            model: Some("claude-opus-4-8".into()),
        };
        accumulate_by_model(&mut by_model, &opus(100));
        accumulate_by_model(&mut by_model, &opus(200));
        // A phase with no captured model is keyed under `unknown`, not dropped.
        accumulate_by_model(
            &mut by_model,
            &Usage {
                input: 7,
                model: None,
                ..Usage::default()
            },
        );

        assert_eq!(by_model["claude-opus-4-8"].input, 300, "opus rows summed");
        assert_eq!(
            by_model["unknown"].input, 7,
            "model-less rows fall to unknown"
        );
        assert_eq!(by_model.len(), 2);
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
    fn next_reset_bare_time_tolerates_trailing_period() {
        // A bare hint with trailing sentence punctuation must parse like the
        // absolute form does, not fall through to None.
        let now = at(2026, 6, 9, 10, 0);
        let got = next_reset("15:00.", now).unwrap();
        assert_eq!(
            got,
            at(2026, 6, 9, 15, 0),
            "bare time strips trailing period"
        );
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

    #[test]
    fn next_reset_absolute_rfc3339_used_directly() {
        // An absolute instant (Codex's format) ignores `now` and resolves to the
        // exact instant it names. Compare epochs so the assertion is timezone-
        // independent (the result is the same instant regardless of local zone).
        let now = at(2026, 6, 9, 10, 0);
        let expected = DateTime::parse_from_rfc3339("2026-06-09T18:00:00Z")
            .unwrap()
            .timestamp();
        assert_eq!(
            next_reset("2026-06-09T18:00:00Z", now).unwrap().timestamp(),
            expected
        );
    }

    #[test]
    fn next_reset_absolute_tolerates_trailing_period() {
        // Codex's message is a sentence: "… Try again at 2026-06-09T18:00:00Z."
        let now = at(2026, 6, 9, 10, 0);
        let expected = DateTime::parse_from_rfc3339("2026-06-09T18:00:00Z")
            .unwrap()
            .timestamp();
        assert_eq!(
            next_reset("2026-06-09T18:00:00Z.", now)
                .unwrap()
                .timestamp(),
            expected
        );
    }

    #[test]
    fn next_reset_absolute_ignores_trailing_prose() {
        // Codex may trail the datetime with prose: "…Z (in 3 hours)".
        let now = at(2026, 6, 9, 10, 0);
        let expected = DateTime::parse_from_rfc3339("2026-06-09T18:00:00Z")
            .unwrap()
            .timestamp();
        assert_eq!(
            next_reset("2026-06-09T18:00:00Z (in 3 hours)", now)
                .unwrap()
                .timestamp(),
            expected
        );
    }

    #[test]
    fn next_reset_absolute_honours_offset() {
        let now = at(2026, 6, 9, 10, 0);
        let expected = DateTime::parse_from_rfc3339("2026-06-09T15:00:00-03:00")
            .unwrap()
            .timestamp();
        assert_eq!(
            next_reset("2026-06-09T15:00:00-03:00", now)
                .unwrap()
                .timestamp(),
            expected
        );
    }
}
