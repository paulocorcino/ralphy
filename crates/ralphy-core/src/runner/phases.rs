//! The per-issue phase pipeline (ADR-0022 split of `runner.rs`): the blocked-by
//! gate, plan/execute phases, the protocol and verify gates, and the close — plus
//! the small helpers and phase-result enums the orchestrator `run_queue_with`
//! matches on. `run_queue_with` stays in the parent module and drives these in
//! order; everything the boundary crosses is `pub(crate)`.

use anyhow::Result;
use tracing::{info, warn};

use crate::repo::Repo;
use crate::{
    acceptance, blocked, handoff, protocol,
    verify::{self, VerifySpec},
    Agent, Execution, Issue, IssueTracker, Outcome, Plan, PlanLimit, Usage, Workspace,
};

use super::artifacts::{
    clear_protocol_failure, clear_verify_failure, record_citations, verify_failure_summary,
    verify_spawn_failure_summary, write_handoffs, write_issue_json, write_knowledge,
    write_protocol_failure, write_references, write_verify_failure,
};
use super::branch::is_human_gate;
use super::comments::{bundle_comment, close_comment, infeasible_comment};
use super::{
    synthetic_reset, IssueResult, QueueConfig, ResultStatus, RunClock, RunLedger, WaitOutcome,
};

/// Consecutive plan-time usage limits that make no progress before the runner
/// gives up and stops-and-reports. Guards a past or unparseable reset hint from
/// spinning the resume loop, mirroring the execute-path no-commit cap.
const MAX_PLAN_LIMIT_RESUMES: u32 = 2;

/// The label applied to an issue the planner judged a bundle (multiple backlog
/// tasks under one number): the queue is parked on a human running `/to-issues`
/// to open the children (`## Parent: #N`) and close the bundle — the
/// follow-the-split blocker gate handles the rest.
const NEEDS_SPLIT_LABEL: &str = "needs-split";

/// How many times a failed verify gate is handed back to the agent to repair
/// before the runner gives up and stops the run (ADR-0011 amendment). The gate
/// stays the authority across every attempt — a repair earns the close only by
/// making the runner *see* the same commands pass; the budget just bounds how
/// long the agent gets to react before the branch is handed back for a human.
const VERIFY_MAX_REPAIRS: u32 = 2;

/// The blocked-by classification of one issue against the tracker: the open
/// blockers (still-open declared blockers, plus the open children of any retired
/// bundle a closed blocker split into), the closed blockers (handoff sources),
/// and the human-gate subset of the open blockers (`ready-for-human`/`HITL`,
/// ADR-0014). Lifted out of [`prepare_issue`] so the read-only
/// [`crate::queue_view::resolve_queue_view`] gates a candidate through the SAME
/// resolution the runner uses. Pure data — no orchestration side effects; the
/// only log line is the diagnostic "closed but split" visibility note. An `Err`
/// (an `is_closed`/`open_children` failure) is fatal to the caller, exactly as
/// in the runner loop; a label-fetch failure degrades to "agent work" with a warn.
pub(crate) struct OpenBlockers {
    pub open: Vec<u64>,
    pub closed: Vec<u64>,
    pub human: Vec<u64>,
}

pub(crate) fn open_blockers(issue: &Issue, tracker: &dyn IssueTracker) -> Result<OpenBlockers> {
    // Refs are the union of the body's `## Blocked by` and the marked
    // consolidated-spec comment's (ADR-0017).
    let refs = blocked::parse_blocked_by_all(&issue.body, &issue.comments);
    let mut open: Vec<u64> = Vec::new();
    let mut closed: Vec<u64> = Vec::new();
    for n in refs {
        match tracker.is_closed(n) {
            Ok(true) => match tracker.open_children(n) {
                Ok(children) if children.is_empty() => closed.push(n),
                Ok(children) => {
                    info!(
                        number = issue.number,
                        blocker = n,
                        children = ?children,
                        "blocker closed but split into open children — still blocking"
                    );
                    open.extend(children);
                }
                Err(e) => return Err(e),
            },
            Ok(false) => open.push(n),
            Err(e) => return Err(e),
        }
    }
    // Split the open blockers into human gates (ready-for-human/HITL — parked
    // until a person acts, ADR-0014) and ordinary agent work the queue clears.
    // A label-fetch failure is non-fatal: degrade to "agent work" rather than
    // abort, since classification is a visibility concern, not a correctness gate.
    let mut human: Vec<u64> = Vec::new();
    for &n in &open {
        match tracker.issue_labels(n) {
            Ok(labels) if is_human_gate(&labels) => human.push(n),
            Ok(_) => {}
            Err(e) => {
                warn!(blocker = n, error = %e, "could not fetch blocker labels — treating as agent work");
            }
        }
    }
    Ok(OpenBlockers {
        open,
        closed,
        human,
    })
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

/// What the runner-enforced verify gate resolves to for one issue (ADR-0011),
/// folding the plan's `## Verify` section with the per-repo settings fallback.
enum VerifyPlan {
    /// Run these commands as the gate.
    Run(Vec<Vec<String>>),
    /// The plan opted out with `## Verify: none` — close on the self-report, no
    /// warning (the absence of verification was a deliberate, visible decision).
    OptedOut,
    /// The plan's `## Verify` section is malformed (a markdown checklist instead of
    /// bare commands, #181). Carries the operator-facing error; the gate cannot run,
    /// so the issue is left open with this summary rather than closed silently.
    Invalid(String),
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
        VerifySpec::Invalid(error) => VerifyPlan::Invalid(error),
        VerifySpec::Unspecified => match fallback {
            Some(commands) if !commands.is_empty() => VerifyPlan::Run(commands.clone()),
            _ => VerifyPlan::NoGate,
        },
    }
}

/// Parse a plan's checkbox lines into `(text, status)` pairs (#96): a `- [ ]` line
/// is `open`, `- [x]`/`- [X]` is `checked`, `- [!]` is `noticed`. `text` is the raw
/// step text after the marker, trimmed. Non-checkbox lines are ignored. Used to
/// build the `steps_json` field carried on `plan written` (the CloudEvents sink maps
/// it to `plan.written.data.steps`), keeping the envelope mapper free of file I/O.
pub(crate) fn parse_plan_steps(md: &str) -> Vec<(String, &'static str)> {
    md.lines()
        .filter_map(|line| {
            let t = line.trim_start();
            let (status, rest) = if let Some(r) = t.strip_prefix("- [ ]") {
                ("open", r)
            } else if let Some(r) = t.strip_prefix("- [x]").or_else(|| t.strip_prefix("- [X]")) {
                ("checked", r)
            } else {
                let r = t.strip_prefix("- [!]")?;
                ("noticed", r)
            };
            Some((rest.trim().to_string(), status))
        })
        .collect()
}

/// Serialize the plan's checkbox steps to the `steps_json` wire string (`[{text,
/// status}]`) carried on `plan written` (#96); empty string on a serialize failure.
fn plan_steps_json(plan_md: &str) -> String {
    let steps: Vec<serde_json::Value> = parse_plan_steps(plan_md)
        .into_iter()
        .map(|(text, status)| serde_json::json!({ "text": text, "status": status }))
        .collect();
    serde_json::to_string(&steps).unwrap_or_default()
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

/// Everything one issue's phase functions share, built once per run after
/// [`super::prepare_branch`]. All borrows are shared — the mutable [`RunLedger`]
/// travels as its own argument so a phase can hold both.
pub(crate) struct IssueCtx<'a> {
    pub(crate) cfg: &'a QueueConfig,
    pub(crate) ws: &'a Workspace,
    pub(crate) repo: &'a dyn Repo,
    pub(crate) agent: &'a dyn Agent,
    pub(crate) tracker: &'a dyn IssueTracker,
    pub(crate) clock: &'a dyn RunClock,
    /// The branch commits land on, for close/no-gate comments.
    pub(crate) branch: &'a str,
}

/// What [`prepare_issue`] decided for one queue member.
pub(crate) enum Prepared {
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
pub(crate) fn prepare_issue(cx: &IssueCtx, issue: &Issue) -> Result<Prepared> {
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
    // The classification (open/closed/human split) is shared with the
    // read-only `ralphy issues` surface via [`open_blockers`] so the two agree.
    let OpenBlockers {
        open: open_blockers,
        closed: closed_blockers,
        human: human_blockers,
    } = open_blockers(&issue, cx.tracker)?;
    if !open_blockers.is_empty() {
        if human_blockers.is_empty() {
            crate::emit::blocked_by_open(issue.number, &open_blockers);
        } else {
            crate::emit::blocked_waiting_human(issue.number, &open_blockers, &human_blockers);
        }
        return Ok(Prepared::Blocked {
            open: open_blockers,
            human: human_blockers,
        });
    }

    // consumed by the telegram notifier / presenter — keep stable
    crate::emit::issue_started(issue.number, &issue.title);

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
pub(crate) enum PlanPhase {
    /// A feasible plan was written — proceed to execute.
    Planned(Plan),
    /// The planner judged the issue infeasible or a bundle; the verdict is
    /// posted on the issue — skip to the next one. `needs_split` distinguishes
    /// the bundle verdict (the `needs-split` label was applied) from a plain
    /// infeasible one; the two are separate statuses on the wire.
    Infeasible { needs_split: bool },
    /// A plan-time usage limit stops the run (configured `stop_on_limit_plan`, or a
    /// scheduled reset that hit the no-progress cap). A limit with no parseable reset
    /// no longer stops here — it parks a synthetic wait and re-plans (ADR-0030).
    StopLimit { reset: Option<String> },
    /// The global deadline cut a reset wait short — stop the run.
    StopDeadline,
}

/// Plan one issue, auto-resuming through usage-limit reset windows the same
/// way execution does, and record the plan's ledger line. A usage limit during
/// planning surfaces as a typed `PlanLimit` (not a generic failure): wait for
/// the reset and re-plan. A limit with no parseable reset parks a synthetic
/// ~30-min window instead of stopping (ADR-0030); only `stop_on_limit_plan`, or a
/// *scheduled* reset that hits the no-progress cap, stops and reports the limit. A
/// genuine (non-limit) planning failure is an `Err`: the caller restores the branch
/// and propagates.
pub(crate) fn plan_phase(
    cx: &IssueCtx,
    issue: &Issue,
    ledger: &mut RunLedger,
) -> Result<PlanPhase> {
    let mut plan_limit_streak = 0u32;
    let plan = loop {
        let e = match cx.agent.plan(issue, cx.ws) {
            Ok(p) => break p,
            Err(e) => e,
        };
        let limit = e.downcast::<PlanLimit>()?;

        plan_limit_streak += 1;
        let capped = plan_limit_streak > MAX_PLAN_LIMIT_RESUMES;
        // A limit that carries no parseable reset is an account-wide pause: instead
        // of stopping, park a synthetic ~30-min window and re-plan, unbounded until
        // the deadline or a human interrupt decides to give up (ADR-0030). The
        // no-progress cap only guards the *scheduled*-reset resume path — a synthetic
        // wait makes no per-issue progress by definition, so counting it would abandon
        // the issue the moment the account is throttled.
        let synthetic = limit.reset.is_none();
        // Stop-and-report when configured, or when a real reset hit the no-progress
        // cap — never delete the branch, so it is handed back exactly like an
        // execute-time limit stop.
        if cx.cfg.stop_on_limit_plan || (!synthetic && capped) {
            info!(
                number = issue.number,
                reset = ?limit.reset,
                "usage limit while planning — stopping run"
            );
            return Ok(PlanPhase::StopLimit { reset: limit.reset });
        }

        // Deadline beats resume: a reset past the deadline stops the run.
        let reset = limit.reset.unwrap_or_else(synthetic_reset);
        if cx.clock.wait_for_reset(&reset) == WaitOutcome::DeadlinePassed {
            info!(
                number = issue.number,
                "deadline beats resume while planning — stopping run"
            );
            return Ok(PlanPhase::StopDeadline);
        }
        // Otherwise loop: re-plan after the reset window.
    };
    // Read the on-disk plan once so the CloudEvents sink can carry the plan's steps
    // and the raw snapshot without any file I/O in the envelope mapper (#96):
    // `steps_json` rides `plan written` (→ `plan.written.data.steps`), and the raw
    // markdown rides a stable `plan opened` event (→ `dev.ralphy.plan.opened`).
    let plan_md = std::fs::read_to_string(cx.ws.plan_path()).unwrap_or_default();
    let steps_json = plan_steps_json(&plan_md);
    crate::emit::plan_written(
        issue.number,
        plan.open_steps as u64,
        &plan.usage,
        &steps_json,
    );
    // The raw plan snapshot at the write point (issue-scoped); the sink maps it to
    // `dev.ralphy.plan.opened`.
    crate::emit::plan_opened(issue.number, &plan_md);

    // Record the plan phase's token usage (ADR-0008 D6). Written before the
    // feasibility branch so even an infeasible plan's planning cost is on the
    // ledger. The plan line carries `ok` — the issue's terminal outcome is its
    // execute line's, joined by `issue` at read-time. Best-effort: a write
    // failure warns, never stops the run (D9).
    ledger.record_phase(
        issue.number,
        "plan",
        "ok",
        &plan.usage,
        plan.session_id.as_deref(),
    );

    // An infeasible plan (no actionable steps) is a skip, not a failure, and
    // not green — the runner neither closes it nor stops the run. The
    // planner's reasoning is posted on the issue so the verdict is
    // actionable instead of dying in the gitignored plan.md.
    if !plan.is_feasible() {
        let mut needs_split = false;
        if let Ok(plan_md) = std::fs::read_to_string(cx.ws.plan_path()) {
            if let Some(reason) = handoff::infeasible_reason(&plan_md) {
                if handoff::is_bundle_reason(&reason) {
                    needs_split = true;
                    crate::emit::needs_split(issue.number);
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
        return Ok(PlanPhase::Infeasible { needs_split });
    }

    Ok(PlanPhase::Planned(plan))
}

/// How one issue's execution ended.
pub(crate) enum ExecPhase {
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
pub(crate) fn execute_phase(
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
    // Last non-empty vendor session across the resume loop — the terminal
    // attempt's session is the one the single execute ledger line records
    // (ADR-0033 §5, last-non-empty-wins).
    let mut exec_session_id: Option<String> = None;
    let outcome = loop {
        let before_sha = cx.repo.head_sha().ok();
        let Execution {
            outcome,
            usage,
            session_id,
        } = cx.agent.execute(plan, cx.ws)?;
        // Accumulate across the resume loop so the single execute ledger line
        // carries the whole issue's execution cost, not just the last attempt.
        exec_usage.add_tokens(&usage);
        if session_id.is_some() {
            exec_session_id = session_id;
        }
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

        let (reset, synthetic) = match &outcome {
            // A scheduled reset (Codex/Claude) auto-resumes at its target time.
            Outcome::Limit(Some(r)) if !cx.cfg.stop_on_limit_exec => (r.clone(), false),
            // A limit with no parseable reset is an account-wide pause: park a
            // synthetic ~30-min window and retry, unbounded until the deadline or a
            // human interrupt (ADR-0030). Marked `synthetic` so the no-progress cap
            // below skips it.
            Outcome::Limit(None) if !cx.cfg.stop_on_limit_exec => (synthetic_reset(), true),
            // Done, any non-limit outcome, or `stop_on_limit_exec` leave the loop
            // with the outcome as-is.
            _ => break outcome,
        };

        // Progress-aware cap: two consecutive no-commit limits abandon the issue.
        // Only the scheduled-reset path is capped — a synthetic wait makes no
        // per-issue progress by definition (the whole account is throttled), so the
        // human resolves it (re-running continues the work), not the cap (B2).
        if !synthetic && no_commit_streak >= 2 {
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
        exec_session_id.as_deref(),
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
pub(crate) enum ProtocolGate {
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
pub(crate) fn protocol_gate(
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
    // The vendor session of the protocol bounce, so the repair line carries it.
    let mut protocol_session_id: Option<String> = None;
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
            session_id,
        } = cx.agent.execute(plan, cx.ws)?;
        protocol_usage.add_tokens(&usage);
        if session_id.is_some() {
            protocol_session_id = session_id;
        }
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
        protocol_session_id.as_deref(),
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
pub(crate) enum VerifyGate {
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
pub(crate) fn verify_gate(
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
    // Last non-empty vendor session across the repair loop — the repair line
    // carries the terminal attempt's session (ADR-0033 §5, last-non-empty-wins).
    let mut repair_session_id: Option<String> = None;
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
                // Short-circuit a non-repairable spawn failure (#182): the gate's
                // deciding command never ran (program not found / typo'd binary),
                // so re-running the SAME argv can never make it pass. Handing it
                // back would burn the whole VERIFY_MAX_REPAIRS budget on a fix the
                // agent has no way to win. Skip immediately with a spawn-specific
                // artifact and summary — still honoring ADR-0011 (the runner never
                // lets a self-report past a red gate); it just stops wasting the
                // budget on a structural failure it can already see.
                if report.spawn_failed() {
                    let summary = verify_spawn_failure_summary(&report);
                    // consumed by the telegram notifier / presenter — keep stable
                    info!(
                        number = issue.number,
                        %summary,
                        "verify gate — command could not spawn, non-repairable, issue not closed"
                    );
                    // Distinct honesty artifact: names it a spec/spawn problem, not
                    // a test failure. Best-effort — a comment failure must not crash
                    // the run.
                    if let Err(e) = cx.tracker.comment(
                        issue.number,
                        &verify::spawn_failure_comment(&cx.cfg.stamp, &report),
                    ) {
                        warn!(number = issue.number, error = %e, "posting verify artifact comment failed");
                    }
                    break GateDecision::Failed(summary);
                }
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
                    session_id,
                } = cx.agent.execute(plan, cx.ws)?;
                repair_usage.add_tokens(&usage);
                if session_id.is_some() {
                    repair_session_id = session_id;
                }
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
        VerifyPlan::Invalid(error) => {
            // A malformed `## Verify` section cannot be run: leave the issue open
            // with the parse error as its summary rather than close it silently
            // (the gate never saw anything pass). The plan author fixes the section.
            // consumed by the telegram notifier / presenter — keep stable
            info!(
                number = issue.number,
                %error,
                "verify gate — malformed `## Verify` section, issue not closed"
            );
            // Honesty artifact on the issue itself, like every other gate outcome
            // (#181). Best-effort — a comment failure must not crash the run.
            if let Err(e) = cx.tracker.comment(
                issue.number,
                &verify::invalid_comment(&cx.cfg.stamp, &error),
            ) {
                warn!(number = issue.number, error = %e, "posting verify artifact comment failed");
            }
            GateDecision::Failed(error)
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
        repair_session_id.as_deref(),
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
pub(crate) fn close_and_record(
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
    crate::emit::issue_closed(issue.number, issue_total, exec_usage);
    worked.push(IssueResult {
        number: issue.number,
        outcome: Some(Outcome::Done),
        closed: true,
        blocked_by: Vec::new(),
        human_blockers: Vec::new(),
        status: ResultStatus::Done,
        skip: None,
    });

    // Write acceptance evidence when the plan carries a ledger, and
    // publish the session's handoff + plan friction so successors (and
    // dependent issues' planners) inherit what this session learned. A
    // missing or empty ledger/handoff is a graceful no-op.
    if let Ok(plan_md) = std::fs::read_to_string(cx.ws.plan_path()) {
        // Capture the raw plan at close (before the next issue's `plan()` overwrites
        // it) so the sink can map it to `dev.ralphy.plan.closed` (#96). Keep stable.
        crate::emit::plan_closed(issue.number, &plan_md);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_plan_steps_maps_the_three_markers() {
        let md = "# Plan\n\n## Steps\n- [ ] open one\n- [x] done two\n- [!] noticed three\n\
                  - not a step\nprose line\n  - [X] indented checked\n";
        assert_eq!(
            parse_plan_steps(md),
            vec![
                ("open one".to_string(), "open"),
                ("done two".to_string(), "checked"),
                ("noticed three".to_string(), "noticed"),
                ("indented checked".to_string(), "checked"),
            ]
        );
        // The serialized wire form parses back to a JSON array of {text,status}.
        let json = plan_steps_json("- [ ] a\n- [x] b\n");
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(
            parsed,
            serde_json::json!([
                {"text": "a", "status": "open"},
                {"text": "b", "status": "checked"},
            ])
        );
    }
}
