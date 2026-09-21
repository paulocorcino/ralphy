//! The close: record the evidence, post the comments and close the issue.

use anyhow::Result;
use tracing::warn;

use crate::{acceptance, handoff, protocol, Issue, Outcome, Plan, Usage};

use super::IssueCtx;
use crate::runner::artifacts::{record_citations, write_knowledge};
use crate::runner::comments::close_comment;
use crate::runner::{IssueResult, ResultStatus, NEEDS_HUMAN_REVIEW_LABEL};

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
    // Vendor spawns this issue paid for: plan + execute always ran; the protocol
    // bounce and the verify-gate repair each count only if they consumed tokens
    // (a repair writes ONE ledger line regardless of attempts, so this is a floor,
    // matching `RunLedger::record_phase_if_used`).
    let invocations =
        2 + u64::from(protocol_usage.total() > 0) + u64::from(repair_usage.total() > 0);
    // `tokens` stays for the telegram notifier (keep stable); `up/cr/cw/out`
    // carry the *execution* phase breakdown so the live UI can combine it
    // with the planning usage it stashed at `plan written` (ADR-0008 D11).
    crate::emit::issue_closed(issue.number, issue_total, invocations, exec_usage);
    // Read the plan once: the ledger's review-only count rides the result, and
    // the same parse feeds the evidence write below.
    // A failed read is not fatal (the issue is already closed) but must never be
    // silent: it zeroes the review-only count on ALL THREE carriers at once.
    let plan_md = std::fs::read_to_string(cx.ws.plan_path())
        .inspect_err(
            |e| warn!(number = issue.number, error = %e, "reading the plan at close failed — no review debt, no evidence, no handoff recorded"),
        )
        .ok();
    let verdicts = plan_md
        .as_deref()
        .map(acceptance::parse_ledger)
        .unwrap_or_default();
    let review_only = verdicts
        .iter()
        .filter(|v| v.kind == acceptance::VerdictKind::ReviewOnly)
        .count() as u64;
    worked.push(IssueResult {
        number: issue.number,
        outcome: Some(Outcome::Done),
        closed: true,
        blocked_by: Vec::new(),
        human_blockers: Vec::new(),
        status: ResultStatus::Done,
        skip: None,
        review_only,
    });

    // Best-effort, and BEFORE every fallible write below: the issue closed
    // carrying criteria only a person can certify, and the label is what
    // survives the terminal scrollback.
    if review_only > 0 {
        if let Err(e) = cx.tracker.add_label(issue.number, NEEDS_HUMAN_REVIEW_LABEL) {
            warn!(number = issue.number, error = %e, "applying needs-human-review label failed");
        }
    }

    // Write acceptance evidence when the plan carries a ledger, and
    // publish the session's handoff + plan friction so successors (and
    // dependent issues' planners) inherit what this session learned. A
    // missing ledger is now caught by the ADR-0015 lint before this point
    // (the close proceeds anyway after the one bounce); a missing or empty
    // handoff stays a graceful no-op.
    if let Some(plan_md) = plan_md {
        // Capture the raw plan at close (before the next issue's `plan()` overwrites
        // it) so the sink can map it to `dev.ralphy.plan.closed` (#96). Keep stable.
        crate::emit::plan_closed(issue.number, &plan_md);
        if !verdicts.is_empty() {
            cx.tracker
                .write_evidence(issue.number, &issue.body, &verdicts)?;
        } else {
            warn!(
                number = issue.number,
                "no acceptance ledger in the plan — no evidence comment written"
            );
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
