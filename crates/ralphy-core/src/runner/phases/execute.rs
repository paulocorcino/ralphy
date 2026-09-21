//! The execute phase: run the executor over the plan and classify its outcome.

use anyhow::Result;
use tracing::info;

use crate::{Execution, Issue, Outcome, Plan, Usage};

use super::IssueCtx;
use crate::runner::artifacts::{clear_protocol_failure, clear_verify_failure};
use crate::runner::{synthetic_reset, RunLedger, WaitOutcome};

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
    let mut exec_attempts: Vec<Usage> = Vec::new();
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
        // Model attribution happens once, after the loop, via `fold_usage` —
        // the ONE place accumulated-usage model derivation lives (ADR-0008
        // D8); the runner stays vendor-neutral and passes no fallback
        // (ADR-0002 — alias fallback lives in the adapter).
        exec_attempts.push(usage);
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

        // The operator's stop (docs/adr/0054), read BEFORE the resume decision.
        // A stop-killed child ends with no verdict, and a transcript that already
        // carried a limit line classifies as `Limit` — which would send this loop
        // into `wait_for_reset` and then call `execute()` AGAIN, spawning a fresh
        // vendor child for a run the operator just stopped.
        if crate::stop::requested() {
            break outcome;
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

    let exec_usage = Usage::fold_usage(&exec_attempts, None);

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
