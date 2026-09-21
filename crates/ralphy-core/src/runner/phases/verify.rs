//! The verify gate (ADR-0011): re-run the plan's `## Verify` commands over
//! the committed state and repair through the executor within a bounded budget.

use anyhow::Result;
use tracing::{info, warn};

use crate::{
    verify::{self, VerifySpec},
    Execution, Issue, Outcome, Plan, Usage,
};

use super::IssueCtx;
use crate::runner::artifacts::{
    clear_verify_failure, verify_failure_summary, verify_spawn_failure_summary,
    write_verify_failure,
};
use crate::runner::RunLedger;

/// How many times a failed verify gate is handed back to the agent to repair
/// before the runner gives up and stops the run (ADR-0011 amendment). The gate
/// stays the authority across every attempt — a repair earns the close only by
/// making the runner *see* the same commands pass; the budget just bounds how
/// long the agent gets to react before the branch is handed back for a human.
const VERIFY_MAX_REPAIRS: u32 = 2;

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
    // hidden (ADR-0008). One usage per attempt, folded after the loop via
    // `fold_usage` — the ONE place accumulated-usage model derivation lives
    // (D8). Accumulating with `add_tokens` instead would drop the model and
    // land the whole repair line in the unpriced `unknown` bucket.
    let mut repair_attempts: Vec<Usage> = Vec::new();
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
                repair_attempts.push(usage);
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

    // Model attribution happens once, after the loop, like the execute phase:
    // the runner stays vendor-neutral and passes no fallback (ADR-0002 — alias
    // fallback lives in the adapter).
    let repair_usage = Usage::fold_usage(&repair_attempts, None);

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
