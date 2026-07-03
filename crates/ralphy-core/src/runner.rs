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
    acceptance, blocked, gitignore, handoff, knowledge,
    ledger::{FileLedger, LedgerRecord, LedgerSink},
    protocol, references,
    repo::{GitRepo, Repo},
    verify::{self, VerifySpec},
    Agent, Execution, Issue, IssueTracker, Outcome, Plan, PlanLimit, Usage, Workspace,
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
/// one of: an absolute RFC3339 instant (`"2026-06-09T18:00:00Z"`, as some adapters
/// emit), a bare `"HH:mm"`, or a weekday-qualified `"Wkd HH:mm"` (the relative
/// forms others emit). An absolute instant is unambiguous and used as-is — `now` is ignored. A
/// bare time resolves to today, rolled to tomorrow when already past `now`; a
/// weekday-qualified time resolves to the next date carrying that weekday (today
/// only when the time is still ahead, else next week). Pure over its inputs so the
/// rollover edge cases unit-test without sleeping. Returns `None` on an
/// unparseable hint.
fn next_reset(reset: &str, now: DateTime<Local>) -> Option<DateTime<Local>> {
    // Strip trailing sentence punctuation an adapter may leave on the hint (e.g.
    // "… Try again at 2026-06-09T18:00:00Z.").
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

/// Verify preconditions and prepare the branch commits will land on, returning
/// `(orig_branch, branch, compare_ref)`. The single entry point for [`run_queue`]
/// to the clean-tree check, the `.gitignore` ensure, and the detached-HEAD guard.
///
/// In `New` mode a fresh `afk/run-<stamp>` branch is cut off `base_branch` (which
/// must exist) and `compare_ref == base_branch`. In `Current` mode no branch is
/// created and no checkout happens: `branch == orig` and `compare_ref` is the
/// HEAD SHA captured before any work, so the commit count means "work this run
/// added" in both modes.
fn prepare_branch(
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

/// The label that pauses the run before the tagged issue (flow-control, not triage).
pub const STOP_BEFORE_LABEL: &str = "stop-before";

/// The fixed operational label marking an issue awaiting an agent triage pass
/// (`ralphy triage`, ADR-0017). Like `stop-before`/`AFK`/`HITL` it lives outside
/// the five canonical triage roles and outside the setup-pocock mapping table,
/// so it is never resolved through `triage-labels.md`. It is also a human-return
/// label under ADR-0016: while present the issue is parked out of the run queue,
/// so triage and run never race.
pub const TRIAGE_AGENT_LABEL: &str = "triage-agent";

/// Labels that mark an issue as a human gate (ADR-0014): a blocker parked until a
/// person acts, not agent work the queue will clear. The canonical
/// `ready-for-human` triage role and its fixed `HITL` alias (ADR-0001). A human
/// gate is never a queue member (it is never queried), so it only ever surfaces
/// as a *blocker* in another issue's `## Blocked by`.
pub const HUMAN_GATE_LABELS: [&str; 2] = ["ready-for-human", "HITL"];

/// Whether a label set marks a human gate — used to split an open blocker into
/// "waiting on a human" (parked) versus ordinary agent work the queue resolves.
fn is_human_gate(labels: &[String]) -> bool {
    labels
        .iter()
        .any(|l| HUMAN_GATE_LABELS.contains(&l.as_str()))
}

/// The label applied to an issue the planner judged a bundle (multiple backlog
/// tasks under one number): the queue is parked on a human running `/to-issues`
/// to open the children (`## Parent: #N`) and close the bundle — the
/// follow-the-split blocker gate handles the rest.
const NEEDS_SPLIT_LABEL: &str = "needs-split";

/// Everything the core needs to work a whole queue — model-free by construction
/// (model and effort are adapter concerns, set when the adapter is built). The
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
    /// The human-return label set (ADR-0016): any of these on a queued issue
    /// outranks its queue label, so the issue is skipped with a recorded reason
    /// and the queue continues. Resolved by the CLI (via
    /// [`crate::github::resolve_human_return_labels`]) so the core stays
    /// `gh`-free. Unlike `stop-before`, `only_issue` does NOT override these — a
    /// human-return label may record someone else's state (ADR-0016).
    pub human_return_labels: Vec<String>,
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
    /// When true, an issue whose verify resolution lands on [`VerifyPlan::NoGate`]
    /// (no `## Verify` in the plan and no settings fallback) is NOT closed on the
    /// agent's self-report: it is labeled `ready-for-human`, a comment explains
    /// why, and the run continues to the next issue (ADR-0015). `false` keeps the
    /// ADR-0011 warn-and-close behavior. From `settings.json`
    /// `verify.require_verify_gate`.
    pub require_verify_gate: bool,
    /// The literal completion token the active adapter's charter tells the
    /// agent to emit. The runner never DETECTS it — completion detection lives
    /// in the adapters (ADR-0002) — it only quotes it in the verify/protocol
    /// repair briefs so the hand-back speaks the agent's own protocol. Supplied
    /// by the caller (the CLI passes the adapter layer's constant).
    pub done_signal: String,
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
    /// The subset of `blocked_by` that are human gates (`ready-for-human`/`HITL`,
    /// ADR-0014): blockers parked until a person acts, not agent work the queue
    /// will clear. Empty when no blocker is a human gate. The run still continues
    /// past the issue — only this chain stalls; this field is for visibility.
    pub human_blockers: Vec<u64>,
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
    /// The local `ralphy/pre-run-<stamp>` tag marking where the run started —
    /// the undo handle (`git reset --hard <tag>` in `Current` mode). `None` when
    /// tagging failed or the run added no commits (the tag is then deleted:
    /// nothing to undo).
    pub undo_tag: Option<String>,
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

/// One run's ledger identity (ADR-0008 D6/D7, read once from git) plus its
/// token accumulators, threaded through the phases so every phase line is
/// built and folded in one place instead of four hand-rolled copies.
struct RunLedger<'a> {
    sink: &'a dyn LedgerSink,
    project: String,
    actor_email: String,
    actor_name: String,
    /// The adapter label, `agent.name()`.
    agent: &'static str,
    run_usage: Usage,
    run_usage_by_model: BTreeMap<String, Usage>,
}

impl RunLedger<'_> {
    /// Append one phase line (best-effort — a write failure warns, never stops
    /// the run, D9) and fold the usage into the run totals.
    fn record_phase(&mut self, issue: u64, phase: &str, outcome: &str, usage: &Usage) {
        let rec = LedgerRecord {
            project: self.project.clone(),
            actor_email: self.actor_email.clone(),
            actor_name: self.actor_name.clone(),
            ralphy_version: env!("CARGO_PKG_VERSION").into(),
            issue,
            phase: phase.into(),
            agent: self.agent.into(),
            model: usage.model.clone().unwrap_or_else(|| "unknown".into()),
            outcome: outcome.into(),
            tokens: usage.clone(),
            ts: chrono::Utc::now().to_rfc3339(),
        };
        if let Err(e) = self.sink.append(&rec) {
            warn!(number = issue, error = %e, "writing {} usage ledger line failed", phase);
        }
        self.run_usage.add_tokens(usage);
        accumulate_by_model(&mut self.run_usage_by_model, usage);
    }

    /// [`record_phase`](Self::record_phase) for the conditional repair phases:
    /// a phase that consumed nothing writes no line AND folds nothing — an
    /// unconditional fold would plant a zero-usage `unknown` key in the
    /// per-model split the report exposes.
    fn record_phase_if_used(&mut self, issue: u64, phase: &str, outcome: &str, usage: &Usage) {
        if usage.total() > 0 {
            self.record_phase(issue, phase, outcome, usage);
        }
    }
}

/// The close comment the runner leaves on a green queue issue: the ps1-oracle
/// close line plus the protocol-lint result (ADR-0015) — ✓/✗ per structural
/// check, with a loud warning when the issue closed carrying violations.
fn close_comment(stamp: &str, branch: &str, lint: &protocol::ProtocolReport) -> String {
    format!(
        "Closed by Ralphy run {stamp} (green on branch '{branch}'; merge by hand).\n\n{}",
        protocol::comment_block(lint)
    )
}

/// The comment posted when `require_verify_gate` parks a gateless issue for a
/// human (ADR-0015): why the runner did not close on the self-report, and what
/// the human does next.
fn no_gate_comment(stamp: &str, branch: &str) -> String {
    format!(
        "Ralphy run {stamp} did NOT close this issue: the executor reported done, \
         but no verify gate resolved — the plan carries no `## Verify` commands and \
         no `verify.command` fallback is configured — and `verify.require_verify_gate` \
         is set (ADR-0015).\n\n\
         The work is committed on branch '{branch}'. Next step (human): review the \
         branch, run whatever verification applies, then close this issue by hand. \
         The `ready-for-human` label marks this gate; the run continued past it."
    )
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

/// What the verify gate decided for a `Done` issue: proceed to the close,
/// leave it open after a spent repair budget, or — with `require_verify_gate`
/// and no gate resolved — park it for a human (ADR-0015).
enum GateDecision {
    /// Gate passed, was opted out of, or (without `require_verify_gate`) no
    /// gate resolved: proceed to the close path.
    Green,
    /// The gate failed and the repair budget is spent; carries the one-line
    /// failure summary. The issue is left open and the queue continues.
    Failed(String),
    /// `require_verify_gate` is set and no gate resolved: label
    /// `ready-for-human`, leave the issue open, continue the queue.
    NeedsHuman,
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
fn write_verify_failure(
    ws: &Workspace,
    stamp: &str,
    report: &verify::VerifyReport,
    done_signal: &str,
) {
    let path = ws.ralphy_dir().join(VERIFY_FAILURE_FILE);
    if let Err(e) = std::fs::write(&path, verify::repair_brief(stamp, report, done_signal)) {
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

/// The scratch file the runner drops in the workspace to hand a protocol-lint
/// violation back to the executor (ADR-0015) — the same vendor-neutral channel
/// as [`VERIFY_FAILURE_FILE`]. Written on the first violation only (one bounce);
/// cleared at each issue's start and once the lint is settled.
const PROTOCOL_FAILURE_FILE: &str = "protocol-failure.md";

/// Write the protocol repair brief so the next `execute()` can read which
/// structural checks failed and complete the charter's protocol. Best-effort:
/// a write failure means the agent retries blind, no worse than not bouncing.
fn write_protocol_failure(
    ws: &Workspace,
    stamp: &str,
    report: &protocol::ProtocolReport,
    done_signal: &str,
) {
    let path = ws.ralphy_dir().join(PROTOCOL_FAILURE_FILE);
    if let Err(e) = std::fs::write(&path, protocol::failure_brief(stamp, report, done_signal)) {
        warn!(error = %e, "writing the protocol-failure repair brief failed");
    }
}

/// Remove the protocol repair brief. Called at each issue's start and once the
/// lint is settled, so a stale brief never steers a later session.
fn clear_protocol_failure(ws: &Workspace) {
    let path = ws.ralphy_dir().join(PROTOCOL_FAILURE_FILE);
    if path.exists() {
        if let Err(e) = std::fs::remove_file(&path) {
            warn!(error = %e, "removing the stale protocol-failure brief failed");
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
/// source (title, state, body) of every issue the body references — its
/// `## Blocked by` and `## Parent` sections plus any inline `#N` mention — and
/// render them, or remove a stale file when the issue names none. The planner
/// reads this so a `#N` reference reaches it as the referenced issue's actual
/// spec, not a paraphrase it might restate as fact in a child issue. Best-effort
/// and depth-1: a fetch failure (a cross-repo or deleted ref, say) logs a warning
/// and skips that ref, and the fetched bodies' own references are not followed
/// transitively.
fn write_references(ws: &Workspace, issue: &Issue, tracker: &dyn IssueTracker) {
    let refs = blocked::referenced_issues(&issue.body, issue.number);
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

/// Append the close's `**Knowledge used**` citations to the hit-rate log at
/// `.ralphy/knowledge/citations.jsonl` — the input the consolidation curator
/// prunes never-cited `KNOWLEDGE.md` bullets against. An empty list (an honest
/// `none`) is recorded too: it is the denominator of the pruning window.
/// Best-effort like `write_knowledge`: a failure warns, never stops the run.
fn record_citations(ws: &Workspace, issue: &Issue, stamp: &str, citations: Vec<String>) {
    let entry = knowledge::CitationEntry {
        issue: issue.number,
        stamp: stamp.to_string(),
        date: chrono::Local::now().format("%Y-%m-%d").to_string(),
        citations,
    };
    if let Err(e) = knowledge::append_citation(ws, &entry) {
        warn!(number = issue.number, error = %e, "appending citation entry failed");
    } else {
        info!(
            number = issue.number,
            citations = entry.citations.len(),
            "knowledge citations recorded"
        );
    }
}

/// Write the issue the planner reads to `.ralphy/issue.json`.
fn write_issue_json(ws: &Workspace, issue: &Issue) -> Result<()> {
    std::fs::create_dir_all(ws.ralphy_dir())?;
    let json = serde_json::to_string_pretty(issue).context("serializing issue to JSON")?;
    std::fs::write(ws.issue_json_path(), json).context("writing .ralphy/issue.json")?;
    Ok(())
}

/// Everything one issue's phase functions share, built once per run after
/// [`prepare_branch`]. All borrows are shared — the mutable [`RunLedger`]
/// travels as its own argument so a phase can hold both.
struct IssueCtx<'a> {
    cfg: &'a QueueConfig,
    ws: &'a Workspace,
    repo: &'a dyn Repo,
    agent: &'a dyn Agent,
    tracker: &'a dyn IssueTracker,
    clock: &'a dyn RunClock,
    /// The branch commits land on, for close/no-gate comments.
    branch: &'a str,
}

/// What [`prepare_issue`] decided for one queue member.
enum Prepared {
    /// The enriched clone (comment thread attached), persisted to `.ralphy/`
    /// and ready to plan.
    Ready(Issue),
    /// Open blockers gate the issue — skip it. Carries the open blockers and
    /// their human-gate subset for the report.
    Blocked { open: Vec<u64>, human: Vec<u64> },
}

/// Gate and stage one issue before planning: the blocked-by/human-gate
/// classification, the comment-thread enrichment, and the `.ralphy/` staging
/// writes (`issue.json`, handoffs, references). An `Err` is fatal to the run —
/// the caller restores the branch and propagates.
fn prepare_issue(cx: &IssueCtx, issue: &Issue) -> Result<Prepared> {
    // Attach the issue's own comment thread up front, before the blocked-by
    // gate: a `## Blocked by` inside the marked consolidated-spec comment
    // (ADR-0017) gates the queue exactly like one in the body, so the gate must
    // see the comments. Best-effort: a fetch failure degrades to body-only
    // gating (and body-only planning), never a stop. The queue's issue carries
    // no comments (the list query omits them), so this clone is where they land.
    let mut issue = issue.clone();
    match cx.tracker.issue_comments(issue.number) {
        Ok(comments) => issue.comments = comments,
        Err(e) => {
            warn!(number = issue.number, error = %e, "fetching issue comments failed — gating and planning with body only")
        }
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
    //
    // Refs are the union of the body's `## Blocked by` and the marked
    // consolidated-spec comment's (ADR-0017).
    let refs = blocked::parse_blocked_by_all(&issue.body, &issue.comments);
    let mut open_blockers: Vec<u64> = Vec::new();
    let mut closed_blockers: Vec<u64> = Vec::new();
    for n in refs {
        match cx.tracker.is_closed(n) {
            Ok(true) => match cx.tracker.open_children(n) {
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
                Err(e) => return Err(e),
            },
            Ok(false) => open_blockers.push(n),
            Err(e) => return Err(e),
        }
    }
    if !open_blockers.is_empty() {
        // Split the open blockers into human gates (ready-for-human/HITL —
        // parked until a person acts, ADR-0014) and ordinary agent work the
        // queue will clear. A label-fetch failure is non-fatal: it degrades
        // to the generic "blocked" reason rather than aborting the AFK run,
        // since classification is a visibility concern, not a correctness gate.
        let mut human_blockers: Vec<u64> = Vec::new();
        for &n in &open_blockers {
            match cx.tracker.issue_labels(n) {
                Ok(labels) if is_human_gate(&labels) => human_blockers.push(n),
                Ok(_) => {}
                Err(e) => {
                    warn!(blocker = n, error = %e, "could not fetch blocker labels — treating as agent work");
                }
            }
        }
        if human_blockers.is_empty() {
            // consumed by the telegram notifier / presenter — keep stable
            info!(
                number = issue.number,
                blockers = ?open_blockers,
                "blocked by open issue(s) — skipping"
            );
        } else {
            // consumed by the telegram notifier / presenter — keep stable
            info!(
                number = issue.number,
                blockers = ?open_blockers,
                human_blockers = ?human_blockers,
                "blocked — waiting on human"
            );
        }
        return Ok(Prepared::Blocked {
            open: open_blockers,
            human: human_blockers,
        });
    }

    // consumed by the telegram notifier / presenter — keep stable
    info!(number = issue.number, title = %issue.title, "issue started");

    // The comment thread was attached up front (for the blocked-by gate); note
    // it here so the "comments attached for planner" visibility line still fires
    // — the planner and executor read the discussion alongside the body.
    if !issue.comments.is_empty() {
        info!(
            number = issue.number,
            comments = issue.comments.len(),
            "comments attached for planner"
        );
    }

    // Persist the current issue where the planner reads it. The adapter's
    // prompt reads `.ralphy/issue.json`, so the loop must refresh it before
    // each plan — `.ralphy/` is gitignored and survives the branch checkout.
    write_issue_json(cx.ws, &issue)?;

    // Shoulders of giants: collect the handoffs the closed blockers left on
    // their issues into `.ralphy/handoffs.md`, where the planner reads them
    // as predecessor context. Best-effort enrichment — a fetch failure is a
    // warning, never a stop — but the file is always refreshed (or removed)
    // so a previous issue's handoffs never leak into this one.
    write_handoffs(cx.ws, issue.number, &closed_blockers, cx.tracker);

    // Reproduce the source of the issues this one references in its
    // `## Blocked by` / `## Parent` sections into `.ralphy/references.md`, so
    // the planner reads the referenced spec at source rather than restating a
    // `#N` mention as fact in a child issue. Best-effort like the handoffs.
    write_references(cx.ws, &issue, cx.tracker);

    Ok(Prepared::Ready(issue))
}

/// What the plan phase decided for one prepared issue.
enum PlanPhase {
    /// A feasible plan was written — proceed to execute.
    Planned(Plan),
    /// The planner judged the issue infeasible or a bundle; the verdict is
    /// posted on the issue — skip to the next one.
    Infeasible,
    /// A plan-time usage limit stops the run (configured stop, no reset to
    /// wait for, or the no-progress cap).
    StopLimit { reset: Option<String> },
    /// The global deadline cut a reset wait short — stop the run.
    StopDeadline,
}

/// Plan one issue, auto-resuming through usage-limit reset windows the same
/// way execution does, and record the plan's ledger line. A usage limit during
/// planning surfaces as a typed `PlanLimit` (not a generic failure): wait for
/// the reset and re-plan, unless `stop_on_limit_plan`, no reset was parsed, or
/// repeated no-progress limits hit the cap — any of which stops and reports
/// the limit. A genuine (non-limit) planning failure is an `Err`: the caller
/// restores the branch and propagates.
fn plan_phase(cx: &IssueCtx, issue: &Issue, ledger: &mut RunLedger) -> Result<PlanPhase> {
    let mut plan_limit_streak = 0u32;
    let plan = loop {
        let e = match cx.agent.plan(issue, cx.ws) {
            Ok(p) => break p,
            Err(e) => e,
        };
        let limit = e.downcast::<PlanLimit>()?;

        plan_limit_streak += 1;
        let capped = plan_limit_streak > MAX_PLAN_LIMIT_RESUMES;
        // Stop-and-report when configured, when no reset was parsed (nothing
        // to wait for), or when the cap is hit — never delete the branch, so
        // it is handed back exactly like an execute-time limit stop.
        if cx.cfg.stop_on_limit_plan || limit.reset.is_none() || capped {
            info!(
                number = issue.number,
                reset = ?limit.reset,
                "usage limit while planning — stopping run"
            );
            return Ok(PlanPhase::StopLimit { reset: limit.reset });
        }

        // Deadline beats resume: a reset past the deadline stops the run.
        let reset = limit.reset.expect("reset present: checked above");
        if cx.clock.wait_for_reset(&reset) == WaitOutcome::DeadlinePassed {
            info!(
                number = issue.number,
                "deadline beats resume while planning — stopping run"
            );
            return Ok(PlanPhase::StopDeadline);
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
    ledger.record_phase(issue.number, "plan", "ok", &plan.usage);

    // An infeasible plan (no actionable steps) is a skip, not a failure, and
    // not green — the runner neither closes it nor stops the run. The
    // planner's reasoning is posted on the issue so the verdict is
    // actionable instead of dying in the gitignored plan.md.
    if !plan.is_feasible() {
        if let Ok(plan_md) = std::fs::read_to_string(cx.ws.plan_path()) {
            if let Some(reason) = handoff::infeasible_reason(&plan_md) {
                if handoff::is_bundle_reason(&reason) {
                    // consumed by the telegram notifier / presenter — keep stable
                    info!(number = issue.number, "bundle plan — needs split");
                    // Best-effort: a label failure must not stop the run —
                    // the comment below still carries the verdict.
                    if let Err(e) = cx.tracker.add_label(issue.number, NEEDS_SPLIT_LABEL) {
                        warn!(number = issue.number, error = %e, "applying needs-split label failed");
                    }
                    // Best-effort: a failed verdict comment must not abort
                    // the queue over a non-green skip.
                    if let Err(e) = cx
                        .tracker
                        .comment(issue.number, &bundle_comment(&cx.cfg.stamp, &reason))
                    {
                        warn!(number = issue.number, error = %e, "posting bundle verdict comment failed");
                    }
                } else if let Err(e) = cx
                    .tracker
                    .comment(issue.number, &infeasible_comment(&cx.cfg.stamp, &reason))
                {
                    warn!(number = issue.number, error = %e, "posting infeasible verdict comment failed");
                }
            }
        }
        return Ok(PlanPhase::Infeasible);
    }

    Ok(PlanPhase::Planned(plan))
}

/// How one issue's execution ended.
enum ExecPhase {
    /// The executor self-reported done — proceed to the gates. Carries the
    /// accumulated execute usage the close path folds into the issue total.
    Done { exec_usage: Usage },
    /// Any non-`Done` terminal outcome — the loop records it and stops the
    /// run (the execute ledger line is already written). `deadline_cut`
    /// marks a resume wait the deadline cut short.
    NonGreen {
        outcome: Outcome,
        deadline_cut: bool,
    },
}

/// Execute one planned issue, auto-resuming through usage-limit reset windows
/// by default, and record the execute ledger line. On `Outcome::Limit` with a
/// parsed reset (and not `stop_on_limit_exec`), wait for the reset and re-run
/// `execute()` only — never `plan()`, which would delete the on-disk `plan.md`
/// the resume depends on (ADR-0003). A progress-aware cap abandons the issue
/// after two consecutive limit outcomes that commit nothing; any commit resets
/// the streak. The cap is checked *before* the next wait so a stalled issue is
/// abandoned without first burning another reset window. An `execute()` error
/// propagates without a restore, exactly like the pre-extraction loop.
fn execute_phase(
    cx: &IssueCtx,
    issue: &Issue,
    plan: &Plan,
    ledger: &mut RunLedger,
) -> Result<ExecPhase> {
    // Start each issue with no repair brief on disk: the gates only write one
    // when *this* run's verify or protocol lint fails, so a brief left by a
    // prior run (stopped on a red gate, then resumed) never silently steers
    // the first execute.
    clear_verify_failure(cx.ws);
    clear_protocol_failure(cx.ws);

    let mut no_commit_streak = 0u32;
    let mut deadline_cut = false;
    let mut exec_usage = Usage::default();
    let outcome = loop {
        let before_sha = cx.repo.head_sha().ok();
        let Execution { outcome, usage } = cx.agent.execute(plan, cx.ws)?;
        // Accumulate across the resume loop so the single execute ledger line
        // carries the whole issue's execution cost, not just the last attempt.
        exec_usage.add_tokens(&usage);
        let after_sha = cx.repo.head_sha().ok();

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
            Outcome::Limit(Some(r)) if !cx.cfg.stop_on_limit_exec => r.clone(),
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
        if cx.clock.wait_for_reset(&reset) == WaitOutcome::DeadlinePassed {
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
    ledger.record_phase(
        issue.number,
        "execute",
        outcome_label(&outcome),
        &exec_usage,
    );

    Ok(if outcome == Outcome::Done {
        ExecPhase::Done { exec_usage }
    } else {
        ExecPhase::NonGreen {
            outcome,
            deadline_cut,
        }
    })
}

/// How the protocol lint settled for a `Done` issue.
enum ProtocolGate {
    /// The lint settled — passed, or still failing after the one bounce (the
    /// loud warn already logged; the close comment carries the report).
    /// Carries what the verify gate and close path need.
    Settled {
        lint: protocol::ProtocolReport,
        plan_md: String,
        protocol_usage: Usage,
    },
    /// The bounce itself hit a usage limit — that is the run's limit, so stop
    /// on the reset instead of judging the lint again.
    StopLimit { reset: Option<String> },
}

/// Deterministic protocol lint (ADR-0015): before anything else, structurally
/// lint the plan the executor claims is finished — every step ticked, the
/// charter's closing sections present, no planner placeholder left in the
/// ledger. Presence and shape only, never truthfulness. On a violation the
/// session is handed back to the executor ONCE via `protocol-failure.md` (the
/// verify-failure mechanism); a second violation falls back to closing with
/// the lint report and a loud warning in the close comment.
fn protocol_gate(
    cx: &IssueCtx,
    issue: &Issue,
    plan: &Plan,
    ledger: &mut RunLedger,
) -> Result<ProtocolGate> {
    let mut plan_md = std::fs::read_to_string(cx.ws.plan_path()).unwrap_or_default();
    let mut lint = protocol::lint(&plan_md);
    // Tokens the one protocol bounce consumes, accounted as their own
    // phase like verify repairs (ADR-0008).
    let mut protocol_usage = Usage::default();
    // Set when the bounce itself hits a usage limit: that is the run's
    // limit, so stop on the reset instead of judging the lint again.
    let mut protocol_limit: Option<Option<String>> = None;
    if !lint.passed() {
        // consumed by the telegram notifier / presenter — keep stable
        info!(
            number = issue.number,
            failed = %lint.failed_labels().join(", "),
            "protocol lint failed — handing back to the executor once"
        );
        write_protocol_failure(cx.ws, &cx.cfg.stamp, &lint, &cx.cfg.done_signal);
        let Execution {
            outcome: bounce_outcome,
            usage,
        } = cx.agent.execute(plan, cx.ws)?;
        protocol_usage.add_tokens(&usage);
        if let Outcome::Limit(reset) = bounce_outcome {
            protocol_limit = Some(reset);
        } else {
            // Re-run the SAME checks over the (possibly) repaired plan;
            // whatever they say now is final — no second bounce.
            plan_md = std::fs::read_to_string(cx.ws.plan_path()).unwrap_or_default();
            lint = protocol::lint(&plan_md);
        }
    }
    clear_protocol_failure(cx.ws);

    ledger.record_phase_if_used(
        issue.number,
        "protocol-repair",
        if lint.passed() {
            "done"
        } else {
            "protocol-failed"
        },
        &protocol_usage,
    );

    // A usage limit mid-bounce is the run's limit — no tokens are left
    // to work the rest of the queue, so stop on the reset.
    if let Some(reset) = protocol_limit {
        return Ok(ProtocolGate::StopLimit { reset });
    }
    if !lint.passed() {
        warn!(
            number = issue.number,
            failed = %lint.failed_labels().join(", "),
            "protocol lint still failing after the bounce — closing with the report"
        );
    }
    Ok(ProtocolGate::Settled {
        lint,
        plan_md,
        protocol_usage,
    })
}

/// What the verify gate decided for a `Done` issue.
enum VerifyGate {
    /// Gate passed, was opted out of, or (without `require_verify_gate`) no
    /// gate resolved — proceed to the close path. Carries the repair usage
    /// the close folds into the issue total.
    Green { repair_usage: Usage },
    /// The gate failed and the repair budget is spent; carries the one-line
    /// failure summary. The issue is left open and the queue continues.
    Failed { summary: String },
    /// A repair attempt itself hit a usage limit — the run's limit, so stop
    /// on the reset rather than burning the rest of the repair budget on an
    /// agent that cannot work.
    StopLimit { reset: Option<String> },
    /// `require_verify_gate` is set and no gate resolved: label the issue
    /// `ready-for-human`, leave it open, continue the queue (ADR-0015).
    NeedsHuman,
}

/// Runner-enforced verify gate (ADR-0011): before closing on the agent's
/// self-reported `Done`, re-run the plan's `## Verify` commands over the
/// committed state. Only a pass proceeds to the close. On a failure the
/// runner hands the failing commands back to the agent (up to
/// [`VERIFY_MAX_REPAIRS`] times) and re-runs the SAME gate after each
/// attempt. The gate stays the authority: a repair earns the close only by
/// making the runner *see* the commands pass, never by a fresh self-report.
/// `## Verify: none` opts out; an absent section falls back to settings, then
/// — depending on `require_verify_gate` — to a loud warn-and-close or to
/// parking the issue for a human (ADR-0015). Records the `repair` ledger line.
fn verify_gate(
    cx: &IssueCtx,
    issue: &Issue,
    plan: &Plan,
    plan_md: &str,
    ledger: &mut RunLedger,
) -> Result<VerifyGate> {
    // Tokens the agent spends on repairs, accounted as their own phase so
    // the initial execute line stays truthful and the repair cost is never
    // hidden (ADR-0008). Folded into the run totals either way.
    let mut repair_usage = Usage::default();
    // Set when a repair attempt itself hits a usage limit. `None` while the
    // gate is still being worked.
    let mut repair_limit: Option<Outcome> = None;
    let gate: GateDecision = match resolve_verify(plan_md, &cx.cfg.verify_fallback) {
        VerifyPlan::Run(commands) => {
            let mut attempt = 0u32;
            loop {
                // consumed by the telegram notifier / presenter — keep stable
                info!(
                    number = issue.number,
                    commands = commands.len(),
                    "verify gate — running"
                );
                let report = verify::run(&commands, &cx.cfg.repo_root, cx.cfg.verify_timeout);
                // Feed the durable command-cost knowledge the verification-cost
                // gate reads: the gate just measured the real price of each
                // `## Verify` command, so future sessions (this repo, any issue)
                // know which ones are too expensive for an inner loop.
                crate::cmdcost::record_gate_costs(
                    &cx.cfg.repo_root,
                    &report
                        .commands
                        .iter()
                        .map(|c| (c.argv.clone(), c.secs))
                        .collect::<Vec<_>>(),
                );
                // Honesty artifact: every command + its exit code (pass or
                // fail), with the failing tail on a failure. Best-effort — a
                // comment failure must not crash a run that otherwise passed.
                if let Err(e) = cx
                    .tracker
                    .comment(issue.number, &verify::comment(&cx.cfg.stamp, &report))
                {
                    warn!(number = issue.number, error = %e, "posting verify artifact comment failed");
                }
                if report.passed {
                    info!(number = issue.number, "verify gate passed");
                    clear_verify_failure(cx.ws);
                    break GateDecision::Green;
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
                    break GateDecision::Failed(summary);
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
                write_verify_failure(cx.ws, &cx.cfg.stamp, &report, &cx.cfg.done_signal);
                let Execution {
                    outcome: repair_outcome,
                    usage,
                } = cx.agent.execute(plan, cx.ws)?;
                repair_usage.add_tokens(&usage);
                // A usage limit mid-repair stops the run on the limit; we do
                // not re-verify (the agent never got to fix anything) and do
                // not spend another attempt.
                if let Outcome::Limit(_) = repair_outcome {
                    repair_limit = Some(repair_outcome);
                    break GateDecision::Failed(summary);
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
            GateDecision::Green
        }
        VerifyPlan::NoGate if cx.cfg.require_verify_gate => {
            // consumed by the telegram notifier / presenter — keep stable
            info!(
                number = issue.number,
                "no verify gate resolved and require_verify_gate is set — \
                 parking the issue for a human"
            );
            GateDecision::NeedsHuman
        }
        VerifyPlan::NoGate => {
            warn!(
                number = issue.number,
                "issue closed without a verify gate — no `## Verify` in the plan \
                 and no settings.json verify.command resolved"
            );
            GateDecision::Green
        }
    };

    // Account the repair phase before branching on the gate result, so the
    // run totals and the per-issue ledger are honest whether the gate went
    // green or the budget ran out (ADR-0008). One `repair` line per issue,
    // regardless of how many attempts ran. Best-effort.
    ledger.record_phase_if_used(
        issue.number,
        "repair",
        if matches!(gate, GateDecision::Failed(_)) {
            "verify-failed"
        } else {
            "done"
        },
        &repair_usage,
    );

    Ok(match gate {
        GateDecision::Failed(summary) => {
            // A repair that hit a usage limit is the *run's* limit, not this
            // issue's fault: there are no tokens left to work the rest of the
            // queue, so stop on the reset (the same global stance the execute
            // path already takes on a limit).
            if let Some(Outcome::Limit(reset)) = repair_limit {
                VerifyGate::StopLimit { reset }
            } else {
                VerifyGate::Failed { summary }
            }
        }
        GateDecision::NeedsHuman => VerifyGate::NeedsHuman,
        GateDecision::Green => VerifyGate::Green { repair_usage },
    })
}

/// Close a green issue and record what it leaves behind: the close comment
/// (with the lint report), the acceptance evidence, the session handoff, and
/// the knowledge note + citations. Pushes the closed [`IssueResult`] onto
/// `worked` *before* the fallible evidence writes, so the result is always
/// present in the report even if one of them errors out (errors propagate to
/// the caller without a restore, exactly like the pre-extraction loop).
#[allow(clippy::too_many_arguments)]
fn close_and_record(
    cx: &IssueCtx,
    issue: &Issue,
    plan: &Plan,
    lint: &protocol::ProtocolReport,
    exec_usage: &Usage,
    protocol_usage: &Usage,
    repair_usage: &Usage,
    worked: &mut Vec<IssueResult>,
) -> Result<()> {
    // Close the cycle: a green queue issue is closed so it leaves the
    // queue; its labels are untouched and the branch is merged by hand.
    cx.tracker
        .close(issue.number, &close_comment(&cx.cfg.stamp, cx.branch, lint))?;

    // Record the closed issue before writing evidence so the result is
    // always present in the report even if write_evidence errors out.
    // consumed by the telegram notifier / presenter — keep stable. The
    // `tokens` field carries the issue's total (plan + execute + protocol
    // bounce + repair) so the live UI can show inline per-issue tokens
    // (ADR-0008 D11).
    let issue_total =
        plan.usage.total() + exec_usage.total() + protocol_usage.total() + repair_usage.total();
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
        human_blockers: Vec::new(),
    });

    // Write acceptance evidence when the plan carries a ledger, and
    // publish the session's handoff + plan friction so successors (and
    // dependent issues' planners) inherit what this session learned. A
    // missing or empty ledger/handoff is a graceful no-op.
    if let Ok(plan_md) = std::fs::read_to_string(cx.ws.plan_path()) {
        let verdicts = acceptance::parse_ledger(&plan_md);
        if !verdicts.is_empty() {
            cx.tracker
                .write_evidence(issue.number, &issue.body, &verdicts)?;
        }
        if let Some(report) = handoff::close_report(&plan_md) {
            cx.tracker.comment(issue.number, &report)?;
        }
        if let Some(note) = handoff::knowledge_note(&plan_md) {
            write_knowledge(cx.ws, issue, &cx.cfg.stamp, &note);
        } else if handoff::has_handoff(&plan_md) {
            warn!(
                number = issue.number,
                "handoff present but no `Environment facts & traps` / \
                 `Commands that work` blocks — no knowledge note cached"
            );
        }
        match handoff::knowledge_used(&plan_md) {
            Some(citations) => record_citations(cx.ws, issue, &cx.cfg.stamp, citations),
            None if handoff::has_handoff(&plan_md) => warn!(
                number = issue.number,
                "handoff present but no `Knowledge used` block — \
                 hit-rate signal lost for this close"
            ),
            None => {}
        }
    }

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
    // The production seams: real git over the repo root, the JSONL usage file.
    // The 5-arg signature is the frozen public commitment (ADR-0006/0009); the
    // injectable seams live on the private worker below, reached by unit tests.
    let repo = GitRepo::new(&cfg.repo_root);
    run_queue_with(cfg, queue, agent, tracker, clock, &repo, &FileLedger)
}

/// [`run_queue`] with every collaborator injectable — the seam the in-crate
/// unit tests drive with fakes (no on-disk git repo, no usage file).
fn run_queue_with(
    cfg: &QueueConfig,
    queue: &[Issue],
    agent: &dyn Agent,
    tracker: &dyn IssueTracker,
    clock: &dyn RunClock,
    repo: &dyn Repo,
    sink: &dyn LedgerSink,
) -> Result<QueueReport> {
    let ws = Workspace::new(&cfg.repo_root);

    let (orig, branch, compare_ref) = prepare_branch(
        repo,
        &cfg.repo_root,
        &cfg.base_branch,
        &cfg.stamp,
        cfg.branch_mode,
    )?;

    // Pre-run undo marker: a local tag at the compare ref (the base in `New`
    // mode, the pre-run HEAD in `Current` mode), so undoing the whole run is one
    // copyable command instead of reflog archaeology. Best-effort — a run must
    // never fail over its own bookkeeping.
    let undo_tag_name = format!("ralphy/pre-run-{}", cfg.stamp);
    let mut undo_tag = match repo.tag(&undo_tag_name, &compare_ref) {
        Ok(()) => Some(undo_tag_name),
        Err(e) => {
            warn!(tag = %undo_tag_name, error = %e, "creating the pre-run undo tag failed");
            None
        }
    };

    // Identity for every ledger line this run writes (ADR-0008 D6/D7), read once
    // from git: the project slug (remote, or a path-hash fallback) and the actor.
    // The accumulators fold every phase's usage into the run totals; the per-model
    // split feeds the read-time USD footer (D8).
    let mut ledger = RunLedger {
        sink,
        project: repo.project_slug(),
        actor_email: repo.user_email().unwrap_or_default(),
        actor_name: repo.user_name().unwrap_or_default(),
        agent: agent.name(),
        run_usage: Usage::default(),
        run_usage_by_model: BTreeMap::new(),
    };

    let mut worked: Vec<IssueResult> = Vec::new();
    let mut stop: Option<StopReason> = None;

    let cx = IssueCtx {
        cfg,
        ws: &ws,
        repo,
        agent,
        tracker,
        clock,
        branch: &branch,
    };

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

        // Human-return precedence (ADR-0016): a label that returns the issue to a
        // human outranks its queue label. Skip with a recorded reason and CONTINUE
        // the queue (unlike stop-before, which halts). `only_issue` does NOT
        // override this: the label may record someone else's state (a reporter
        // owing info, a parked verify gate) that a run flag must not steamroll.
        if let Some(label) = issue
            .labels
            .iter()
            .find(|l| cfg.human_return_labels.iter().any(|h| h == *l))
        {
            // consumed by the telegram notifier / presenter — keep stable
            info!(
                number = issue.number,
                label = %label,
                "human-return label — skipping issue"
            );
            worked.push(IssueResult {
                number: issue.number,
                outcome: None,
                closed: false,
                blocked_by: Vec::new(),
                human_blockers: Vec::new(),
            });
            continue;
        }

        // Gate and stage the issue (blocked-by, comment enrichment, `.ralphy/`
        // staging). A preparation error is fatal: restore and propagate.
        let issue = match prepare_issue(&cx, issue) {
            Ok(Prepared::Ready(enriched)) => enriched,
            Ok(Prepared::Blocked { open, human }) => {
                worked.push(IssueResult {
                    number: issue.number,
                    outcome: None,
                    closed: false,
                    blocked_by: open,
                    human_blockers: human,
                });
                continue;
            }
            Err(e) => {
                restore(repo, &orig, &branch, &cfg.base_branch, cfg.branch_mode);
                return Err(e);
            }
        };
        let issue = &issue;

        // Plan the issue; a non-limit planning failure restores and propagates.
        let plan = match plan_phase(&cx, issue, &mut ledger) {
            Ok(PlanPhase::Planned(plan)) => plan,
            Ok(PlanPhase::Infeasible) => {
                worked.push(IssueResult {
                    number: issue.number,
                    outcome: None,
                    closed: false,
                    blocked_by: Vec::new(),
                    human_blockers: Vec::new(),
                });
                continue;
            }
            Ok(PlanPhase::StopLimit { reset }) => {
                worked.push(IssueResult {
                    number: issue.number,
                    outcome: Some(Outcome::Limit(reset.clone())),
                    closed: false,
                    blocked_by: Vec::new(),
                    human_blockers: Vec::new(),
                });
                stop = Some(StopReason::Limit {
                    number: issue.number,
                    reset,
                });
                break 'queue;
            }
            Ok(PlanPhase::StopDeadline) => {
                stop = Some(StopReason::Deadline);
                break 'queue;
            }
            Err(e) => {
                restore(repo, &orig, &branch, &cfg.base_branch, cfg.branch_mode);
                return Err(e);
            }
        };

        // A dry run plans only — it executes nothing and closes nothing.
        if cfg.dry_run {
            worked.push(IssueResult {
                number: issue.number,
                outcome: None,
                closed: false,
                blocked_by: Vec::new(),
                human_blockers: Vec::new(),
            });
            continue;
        }

        // Execute the issue; any non-green terminal outcome stops the whole
        // run — later issues are untouched.
        let exec_usage = match execute_phase(&cx, issue, &plan, &mut ledger)? {
            ExecPhase::Done { exec_usage } => exec_usage,
            ExecPhase::NonGreen {
                outcome,
                deadline_cut,
            } => {
                // consumed by the telegram notifier / presenter — keep stable
                info!(number = issue.number, ?outcome, "non-green — stopping run");
                let number = issue.number;
                worked.push(IssueResult {
                    number,
                    outcome: Some(outcome.clone()),
                    closed: false,
                    blocked_by: Vec::new(),
                    human_blockers: Vec::new(),
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
        };

        // Structurally lint the finished plan, with one bounce back to the
        // executor on a violation (ADR-0015).
        let (lint, plan_md, protocol_usage) = match protocol_gate(&cx, issue, &plan, &mut ledger)? {
            ProtocolGate::Settled {
                lint,
                plan_md,
                protocol_usage,
            } => (lint, plan_md, protocol_usage),
            ProtocolGate::StopLimit { reset } => {
                worked.push(IssueResult {
                    number: issue.number,
                    outcome: Some(Outcome::Limit(reset.clone())),
                    closed: false,
                    blocked_by: Vec::new(),
                    human_blockers: Vec::new(),
                });
                stop = Some(StopReason::Limit {
                    number: issue.number,
                    reset,
                });
                break;
            }
        };

        // Re-run the plan's `## Verify` commands over the committed state
        // before trusting the self-report (ADR-0011/0015).
        let repair_usage = match verify_gate(&cx, issue, &plan, &plan_md, &mut ledger)? {
            VerifyGate::Green { repair_usage } => repair_usage,
            VerifyGate::StopLimit { reset } => {
                let number = issue.number;
                worked.push(IssueResult {
                    number,
                    outcome: Some(Outcome::Limit(reset.clone())),
                    closed: false,
                    blocked_by: Vec::new(),
                    human_blockers: Vec::new(),
                });
                stop = Some(StopReason::Limit { number, reset });
                break;
            }
            VerifyGate::Failed { summary } => {
                let number = issue.number;
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
                    human_blockers: Vec::new(),
                });
                continue;
            }
            VerifyGate::NeedsHuman => {
                let number = issue.number;
                // ADR-0015: the one hole where a false self-report closed an
                // issue unchecked is now a human gate. Label + comment are
                // best-effort — the issue staying OPEN is the guarantee, and
                // a failed label must not abort the rest of the queue.
                if let Err(e) = tracker.add_label(number, HUMAN_GATE_LABELS[0]) {
                    warn!(number, error = %e, "applying ready-for-human label failed");
                }
                if let Err(e) = tracker.comment(number, &no_gate_comment(&cfg.stamp, &branch)) {
                    warn!(number, error = %e, "posting the no-gate comment failed");
                }
                // consumed by the telegram notifier / presenter — keep stable
                info!(
                    number,
                    "no verify gate — issue left open for a human, run continues"
                );
                worked.push(IssueResult {
                    number,
                    outcome: Some(Outcome::Done),
                    closed: false,
                    blocked_by: Vec::new(),
                    human_blockers: Vec::new(),
                });
                continue;
            }
        };

        // Close the cycle and publish what the session leaves behind.
        close_and_record(
            &cx,
            issue,
            &plan,
            &lint,
            &exec_usage,
            &protocol_usage,
            &repair_usage,
            &mut worked,
        )?;
    }

    // Count what the run added over the compare ref and capture the oneline log,
    // matching the ps1 `finally` block. Failures here are non-fatal reporting
    // concerns (e.g. a dropped branch in cleanup) — default to zero / empty.
    let range = format!("{compare_ref}..{branch}");
    let commits = repo.rev_list_count(&range).unwrap_or(0);
    let oneline = repo.log_oneline(&range).unwrap_or_default();

    // A run that added nothing has nothing to undo — drop the marker so tags
    // never accumulate for dry runs and empty queues (mirrors the empty-branch
    // delete in `restore`).
    if commits == 0 {
        if let Some(tag) = undo_tag.take() {
            if let Err(e) = repo.delete_tag(&tag) {
                warn!(%tag, error = %e, "deleting the empty run's undo tag failed");
            }
        }
    }

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
                // Force, same as `restore`: `.ralphy/` scratch may modify a
                // tracked file (e.g. a plan.md committed on the base), and a
                // non-force checkout would abort and strand the repo on the
                // run branch after an otherwise green run (ADR-0005, #41).
                if let Err(e) = repo.checkout_force(&orig) {
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
        undo_tag,
        oneline,
        run_usage: ledger.run_usage,
        run_usage_by_model: ledger.run_usage_by_model,
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
fn restore(repo: &dyn Repo, orig: &str, branch: &str, base: &str, mode: BranchMode) {
    if mode == BranchMode::Current {
        return;
    }
    // Force: the run branch may carry the uncommitted `.gitignore` edit (a dry run
    // never commits it), which must be discarded rather than dragged onto `orig`.
    if let Err(e) = repo.checkout_force(orig) {
        warn!("could not return to '{orig}': {e}");
        return;
    }
    let empty = repo
        .rev_list_count(&format!("{base}..{branch}"))
        .unwrap_or(1)
        == 0;
    if empty {
        if let Err(e) = repo.delete_branch(branch) {
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
        let usage_a = |i| Usage {
            input: i,
            output: 0,
            cache_read: 0,
            cache_creation: 0,
            model: Some("model-a".into()),
        };
        accumulate_by_model(&mut by_model, &usage_a(100));
        accumulate_by_model(&mut by_model, &usage_a(200));
        // A phase with no captured model is keyed under `unknown`, not dropped.
        accumulate_by_model(
            &mut by_model,
            &Usage {
                input: 7,
                model: None,
                ..Usage::default()
            },
        );

        assert_eq!(by_model["model-a"].input, 300, "same-model rows summed");
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
        // An absolute instant ignores `now` and resolves to the
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
        // Some adapters emit a sentence: "… Try again at 2026-06-09T18:00:00Z."
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
        // The datetime may be trailed with prose: "…Z (in 3 hours)".
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

    // ------------------------------------------------------------------
    // Queue-loop tests through the injectable seams: `run_queue_with` driven
    // by a FakeRepo and FakeLedger — no on-disk git repository, no usage
    // file, no RALPHY_USAGE_DIR juggling. The workspace is a plain temp dir
    // (the `.ralphy/` scratch and the verify commands only need a
    // filesystem). Complements tests/queue.rs, which proves the same loop
    // over a real repo.
    // ------------------------------------------------------------------

    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::fs;
    use std::path::PathBuf;

    /// Constant answers for the reads, a recorder for the checkouts. The
    /// constant `head_sha` is fine here: the no-commit streak only matters on
    /// limit-resumes, which these tests do not script.
    struct FakeRepo {
        checkouts: RefCell<Vec<String>>,
    }

    impl FakeRepo {
        fn new() -> Self {
            Self {
                checkouts: RefCell::new(Vec::new()),
            }
        }
    }

    impl Repo for FakeRepo {
        fn current_branch(&self) -> Result<String> {
            Ok("main".into())
        }

        fn head_sha(&self) -> Result<String> {
            Ok("abc123".into())
        }

        fn project_slug(&self) -> String {
            "owner/repo".into()
        }

        fn checkout_new_branch(&self, branch: &str, base: &str) -> Result<()> {
            self.checkouts
                .borrow_mut()
                .push(format!("new:{branch}:{base}"));
            Ok(())
        }

        fn checkout_force(&self, refname: &str) -> Result<()> {
            self.checkouts.borrow_mut().push(format!("force:{refname}"));
            Ok(())
        }

        fn delete_branch(&self, branch: &str) -> Result<()> {
            self.checkouts.borrow_mut().push(format!("delete:{branch}"));
            Ok(())
        }

        fn rev_list_count(&self, _range: &str) -> Result<usize> {
            Ok(1)
        }

        fn log_oneline(&self, _range: &str) -> Result<Vec<String>> {
            Ok(vec!["abc123 work".into()])
        }

        fn user_email(&self) -> Option<String> {
            Some("t@example.com".into())
        }

        fn user_name(&self) -> Option<String> {
            Some("Test".into())
        }
    }

    /// Captures every ledger line in memory.
    #[derive(Default)]
    struct FakeLedger {
        records: RefCell<Vec<LedgerRecord>>,
    }

    impl LedgerSink for FakeLedger {
        fn append(&self, rec: &LedgerRecord) -> Result<()> {
            self.records.borrow_mut().push(rec.clone());
            Ok(())
        }
    }

    /// Plans a one-step, lint-clean plan (optionally carrying a `## Verify`
    /// section, or a protocol-dirty shape) and pops a scripted outcome per
    /// `execute` — never touching git.
    struct MiniAgent {
        outcomes: RefCell<VecDeque<Outcome>>,
        planned: RefCell<Vec<u64>>,
        /// Appended verbatim to the plan (e.g. a `## Verify` section).
        extra: Option<String>,
        /// Write a protocol-dirty plan (unticked step, no closing sections).
        lint_dirty: bool,
        /// On `execute`, repair the plan when the ADR-0015 bounce brief is on
        /// disk: tick every step and append the closing sections.
        fix_protocol: bool,
    }

    impl MiniAgent {
        fn new(outcomes: Vec<Outcome>) -> Self {
            Self {
                outcomes: RefCell::new(outcomes.into()),
                planned: RefCell::new(Vec::new()),
                extra: None,
                lint_dirty: false,
                fix_protocol: false,
            }
        }

        fn with_extra(mut self, extra: impl Into<String>) -> Self {
            self.extra = Some(extra.into());
            self
        }

        fn lint_dirty_with_fix(mut self) -> Self {
            self.lint_dirty = true;
            self.fix_protocol = true;
            self
        }
    }

    impl Agent for MiniAgent {
        fn name(&self) -> &'static str {
            "mini"
        }

        fn plan(&self, issue: &Issue, ws: &Workspace) -> Result<Plan> {
            self.planned.borrow_mut().push(issue.number);
            fs::create_dir_all(ws.ralphy_dir())?;
            let step = if self.lint_dirty {
                "- [ ] do a thing\n"
            } else {
                "- [x] do a thing\n"
            };
            let extra = self
                .extra
                .as_deref()
                .map(|e| format!("\n{e}\n"))
                .unwrap_or_default();
            let closing = if self.lint_dirty {
                ""
            } else {
                "\n## Handoff\n\n- **Delivered**: scripted work\n\n## Plan friction\n\n- none\n"
            };
            let body = format!(
                "# Plan for #{}\n\n## Steps\n{step}{extra}{closing}",
                issue.number
            );
            let path = ws.plan_path();
            fs::write(&path, body)?;
            Ok(Plan {
                path,
                open_steps: 1,
                recommended_model: None,
                usage: Usage {
                    output: 3,
                    model: Some("fake-model".into()),
                    ..Usage::default()
                },
            })
        }

        fn execute(&self, _plan: &Plan, ws: &Workspace) -> Result<Execution> {
            if self.fix_protocol && ws.ralphy_dir().join("protocol-failure.md").exists() {
                let plan_md = fs::read_to_string(ws.plan_path())?;
                let fixed = plan_md.replace("- [ ]", "- [x]")
                    + "\n## Handoff\n\n- **Delivered**: repaired\n\n## Plan friction\n\n- none\n";
                fs::write(ws.plan_path(), fixed)?;
            }
            let outcome = self
                .outcomes
                .borrow_mut()
                .pop_front()
                .expect("more execute calls than scripted outcomes");
            Ok(Execution {
                outcome,
                usage: Usage {
                    output: 5,
                    model: Some("fake-model".into()),
                    ..Usage::default()
                },
            })
        }
    }

    /// Records closes/comments/labels; the trait's defaults cover the rest.
    #[derive(Default)]
    struct FakeTracker {
        closes: RefCell<Vec<u64>>,
        comments: RefCell<Vec<(u64, String)>>,
        labels: RefCell<Vec<(u64, String)>>,
    }

    impl IssueTracker for FakeTracker {
        fn close(&self, number: u64, _comment: &str) -> Result<()> {
            self.closes.borrow_mut().push(number);
            Ok(())
        }

        fn comment(&self, number: u64, body: &str) -> Result<()> {
            self.comments.borrow_mut().push((number, body.to_string()));
            Ok(())
        }

        fn add_label(&self, number: u64, label: &str) -> Result<()> {
            self.labels.borrow_mut().push((number, label.to_string()));
            Ok(())
        }
    }

    /// Never expires, never sleeps.
    struct FakeClock;

    impl RunClock for FakeClock {
        fn deadline_passed(&self) -> bool {
            false
        }

        fn wait_for_reset(&self, _reset: &str) -> WaitOutcome {
            WaitOutcome::Resumed
        }
    }

    /// A fresh plain directory (no git) the workspace and verify commands run
    /// in; unique per test so parallel tests never collide.
    fn test_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("ralphy-runner-ut-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn test_cfg(root: &std::path::Path, stamp: &str) -> QueueConfig {
        QueueConfig {
            repo_root: root.to_path_buf(),
            base_branch: "main".into(),
            dry_run: false,
            stamp: stamp.into(),
            branch_mode: BranchMode::New,
            only_issue: None,
            stop_on_limit_plan: false,
            stop_on_limit_exec: false,
            verify_fallback: None,
            verify_timeout: Duration::from_secs(60),
            require_verify_gate: false,
            done_signal: "DONE_TOKEN".into(),
            human_return_labels: vec![
                "ready-for-human".into(),
                "HITL".into(),
                "needs-info".into(),
                "needs-triage".into(),
                "wontfix".into(),
                "triage-agent".into(),
            ],
        }
    }

    fn test_issue(number: u64) -> Issue {
        Issue {
            number,
            title: format!("issue {number}"),
            body: String::new(),
            labels: vec![],
            comments: vec![],
        }
    }

    /// A `## Verify` line whose command exits 0 on every platform.
    fn verify_ok_line() -> &'static str {
        if cfg!(windows) {
            "cmd /c \"exit 0\""
        } else {
            "sh -c \"exit 0\""
        }
    }

    /// A `## Verify` line whose command exits non-zero on every platform.
    fn verify_fail_line() -> &'static str {
        if cfg!(windows) {
            "cmd /c \"exit 3\""
        } else {
            "sh -c \"exit 3\""
        }
    }

    #[test]
    fn green_close_runs_through_fakes_only() {
        let root = test_dir("green");
        let cfg = test_cfg(&root, "ut-green");
        let repo = FakeRepo::new();
        let sink = FakeLedger::default();
        let agent = MiniAgent::new(vec![Outcome::Done])
            .with_extra(format!("## Verify\n\n{}\n", verify_ok_line()));
        let tracker = FakeTracker::default();

        let report = run_queue_with(
            &cfg,
            &[test_issue(7)],
            &agent,
            &tracker,
            &FakeClock,
            &repo,
            &sink,
        )
        .expect("run succeeds");

        assert_eq!(report.worked.len(), 1);
        assert!(report.worked[0].closed, "green issue is closed");
        assert!(report.stop.is_none());
        assert_eq!(report.commits, 1, "commit count read through the fake");
        assert_eq!(*tracker.closes.borrow(), vec![7]);

        // One plan + one execute ledger line, folded into the run totals.
        let phases: Vec<String> = sink
            .records
            .borrow()
            .iter()
            .map(|r| r.phase.clone())
            .collect();
        assert_eq!(phases, vec!["plan", "execute"]);
        assert_eq!(report.run_usage.total(), 8, "3 plan + 5 execute tokens");
        // The plan usage carries its model straight through; the execute usage
        // is an `add_tokens` accumulation, which by design never copies
        // `model`, so its tokens key under `unknown` (pre-refactor behavior).
        assert_eq!(report.run_usage_by_model["fake-model"].total(), 3);
        assert_eq!(report.run_usage_by_model["unknown"].total(), 5);

        // Branch lifecycle: the run branch was cut, and the clean run
        // returned to the original branch.
        let checkouts = repo.checkouts.borrow();
        assert_eq!(checkouts[0], "new:afk/run-ut-green:main");
        assert_eq!(checkouts.last().unwrap(), "force:main");

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn non_green_outcome_stops_the_run() {
        let root = test_dir("nongreen");
        let cfg = test_cfg(&root, "ut-nongreen");
        let repo = FakeRepo::new();
        let sink = FakeLedger::default();
        let agent = MiniAgent::new(vec![Outcome::Stuck]);
        let tracker = FakeTracker::default();

        let report = run_queue_with(
            &cfg,
            &[test_issue(1), test_issue(2)],
            &agent,
            &tracker,
            &FakeClock,
            &repo,
            &sink,
        )
        .expect("run succeeds");

        assert!(matches!(
            report.stop,
            Some(StopReason::NonGreen {
                number: 1,
                outcome: Outcome::Stuck
            })
        ));
        assert_eq!(report.worked.len(), 1, "issue 2 never started");
        assert_eq!(*agent.planned.borrow(), vec![1], "issue 2 never planned");
        assert!(tracker.closes.borrow().is_empty());

        // The execute ledger line carries the terminal outcome.
        let records = sink.records.borrow();
        let exec = records.iter().find(|r| r.phase == "execute").unwrap();
        assert_eq!(exec.outcome, "stuck");

        // A stopped run leaves the repo on the run branch for inspection.
        assert!(
            !repo.checkouts.borrow().iter().any(|c| c == "force:main"),
            "no return to the original branch on a stop"
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn verify_gate_failure_leaves_issue_open_and_run_continues() {
        let root = test_dir("verify-fail");
        let cfg = test_cfg(&root, "ut-vfail");
        let repo = FakeRepo::new();
        let sink = FakeLedger::default();
        // Initial execute + two repair attempts, all `Done`; the gate itself
        // keeps failing, so the repair budget is spent and the issue is left
        // open while the run marches on.
        let agent = MiniAgent::new(vec![Outcome::Done, Outcome::Done, Outcome::Done])
            .with_extra(format!("## Verify\n\n{}\n", verify_fail_line()));
        let tracker = FakeTracker::default();

        let report = run_queue_with(
            &cfg,
            &[test_issue(3)],
            &agent,
            &tracker,
            &FakeClock,
            &repo,
            &sink,
        )
        .expect("run succeeds");

        assert!(
            report.stop.is_none(),
            "verify failure does not stop the run"
        );
        assert_eq!(report.worked.len(), 1);
        assert!(!report.worked[0].closed);
        assert!(
            report.worked[0].outcome.is_none(),
            "reported skipped-on-verify"
        );
        assert!(tracker.closes.borrow().is_empty());

        // The repair phase is on the ledger with the failed-gate outcome.
        let records = sink.records.borrow();
        let repair = records.iter().find(|r| r.phase == "repair").unwrap();
        assert_eq!(repair.outcome, "verify-failed");
        assert_eq!(repair.tokens.total(), 10, "two repair executes accumulated");

        // The honesty artifact was posted on each gate run.
        assert!(tracker
            .comments
            .borrow()
            .iter()
            .any(|(n, b)| *n == 3 && b.contains("## Verify (Ralphy run ut-vfail)")));

        // The run finished cleanly, so the repo returned to the original branch.
        assert_eq!(repo.checkouts.borrow().last().unwrap(), "force:main");

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn protocol_bounce_repairs_then_closes() {
        let root = test_dir("protocol");
        let cfg = test_cfg(&root, "ut-protocol");
        let repo = FakeRepo::new();
        let sink = FakeLedger::default();
        // First execute claims Done over a protocol-dirty plan; the lint
        // bounces the session back once, the executor repairs, the re-lint
        // passes, and the issue closes (no `## Verify` → warn-and-close).
        let agent = MiniAgent::new(vec![Outcome::Done, Outcome::Done]).lint_dirty_with_fix();
        let tracker = FakeTracker::default();

        let report = run_queue_with(
            &cfg,
            &[test_issue(4)],
            &agent,
            &tracker,
            &FakeClock,
            &repo,
            &sink,
        )
        .expect("run succeeds");

        assert_eq!(report.worked.len(), 1);
        assert!(report.worked[0].closed, "closed after the repaired bounce");
        assert_eq!(*tracker.closes.borrow(), vec![4]);

        // The bounce is its own ledger phase, settled green.
        let phases: Vec<String> = sink
            .records
            .borrow()
            .iter()
            .map(|r| r.phase.clone())
            .collect();
        assert_eq!(phases, vec!["plan", "execute", "protocol-repair"]);
        let records = sink.records.borrow();
        let bounce = records
            .iter()
            .find(|r| r.phase == "protocol-repair")
            .unwrap();
        assert_eq!(bounce.outcome, "done");

        let _ = fs::remove_dir_all(&root);
    }
}
