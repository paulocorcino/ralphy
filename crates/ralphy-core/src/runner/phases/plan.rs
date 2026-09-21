//! The plan phase: run the planner, replan through a plan-time limit, and
//! read the steps off the plan it wrote.

use anyhow::Result;
use tracing::{info, warn};

use crate::{handoff, Issue, Plan, PlanLimit};

use super::IssueCtx;
use crate::runner::comments::{bundle_comment, infeasible_comment};
use crate::runner::{synthetic_reset, RunLedger, WaitOutcome, NEEDS_SPLIT_LABEL};

/// Consecutive plan-time usage limits that make no progress before the runner
/// gives up and stops-and-reports. Guards a past or unparseable reset hint from
/// spinning the resume loop, mirroring the execute-path no-commit cap.
const MAX_PLAN_LIMIT_RESUMES: u32 = 2;

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

#[cfg(test)]
mod tests;
