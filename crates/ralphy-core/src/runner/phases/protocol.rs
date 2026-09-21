//! The protocol gate: lint the plan's ledger before the verify gate runs.

use anyhow::Result;
use tracing::{info, warn};

use crate::{protocol, Execution, Issue, Outcome, Plan, Usage};

use super::IssueCtx;
use crate::runner::artifacts::{clear_protocol_failure, write_protocol_failure};
use crate::runner::RunLedger;

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
/// lint the plan the executor claims is finished — no step left open, the
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
    // phase like verify repairs (ADR-0008). Taken from the bounce whole —
    // model included — rather than summed in with `add_tokens`, which drops
    // the model and would land the line in the unpriced `unknown` bucket (D8).
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
        protocol_usage = usage;
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
