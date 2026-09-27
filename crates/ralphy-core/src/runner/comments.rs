use crate::protocol;

/// The close comment the runner leaves on a green queue issue: the ps1-oracle
/// close line plus the protocol-lint result (ADR-0015) — ✓/✗ per structural
/// check, with a loud warning when the issue closed carrying violations.
pub(crate) fn close_comment(stamp: &str, branch: &str, lint: &protocol::ProtocolReport) -> String {
    format!(
        "Closed by Ralphy run {stamp} (green on branch '{branch}'; merge by hand).\n\n{}",
        protocol::comment_block(lint)
    )
}

/// The comment posted when `require_verify_gate` parks a gateless issue for a
/// human (ADR-0015): why the runner did not close on the self-report, and what
/// the human does next.
pub(crate) fn no_gate_comment(stamp: &str, branch: &str) -> String {
    format!(
        "Ralphy run {stamp} did NOT close this issue. The agent reported the work \
         as done, but nothing checked it: the plan has no `## Verify` commands, no \
         `verify.command` is configured, and `verify.require_verify_gate` is on.\n\n\
         The work is committed on branch '{branch}'. Next step, for a person: review \
         the branch, check the work, then close this issue by hand. The \
         `ready-for-human` label marks this issue; the run went on to the next one."
    )
}

/// The comment posted when the planner judges an issue infeasible: the verdict
/// plus the planner's reasoning, so the skip is actionable from the issue
/// itself (split it, respecify it) instead of silent.
pub(crate) fn infeasible_comment(stamp: &str, reason: &str) -> String {
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
pub(crate) fn bundle_comment(stamp: &str, reason: &str) -> String {
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
