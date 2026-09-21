//! The per-issue phase pipeline (ADR-0022 split of `runner.rs`): the blocked-by
//! gate, plan/execute phases, the protocol and verify gates, and the close — plus
//! the small helpers and phase-result enums the orchestrator `run_queue_with`
//! matches on. `run_queue_with` stays in the parent module and drives these in
//! order; everything the boundary crosses is `pub(crate)`.

use anyhow::Result;
use tracing::{info, warn};

use crate::repo::Repo;
use crate::{blocked, Agent, Issue, IssueTracker, Workspace};

use super::artifacts::{write_handoffs, write_issue_json, write_references};
use super::branch::is_human_gate;
use super::{QueueConfig, RunClock};

mod close;
mod execute;
mod plan;
mod protocol;
mod verify;

pub(crate) use close::close_and_record;
pub(crate) use execute::{execute_phase, ExecPhase};
pub(crate) use plan::{plan_phase, PlanPhase};
pub(crate) use protocol::{protocol_gate, ProtocolGate};
pub(crate) use verify::{verify_gate, VerifyGate};

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
    // consolidated-spec comment's (ADR-0017). An issue never blocks itself: a
    // self-ref is malformed, and the follow-the-split path below can name the
    // issue itself when it declares the retired bundle as its own parent —
    // either way it would park the issue forever on a blocker only it can clear.
    let refs = blocked::parse_blocked_by_all(&issue.body, &issue.comments);
    let mut open: Vec<u64> = Vec::new();
    let mut closed: Vec<u64> = Vec::new();
    for n in refs.into_iter().filter(|&n| n != issue.number) {
        match tracker.is_closed(n) {
            Ok(true) => {
                let children: Vec<u64> = tracker
                    .open_children(n)?
                    .into_iter()
                    .filter(|&c| c != issue.number)
                    .collect();
                if children.is_empty() {
                    closed.push(n);
                } else {
                    info!(
                        number = issue.number,
                        blocker = n,
                        children = ?children,
                        "blocker closed but split into open children — still blocking"
                    );
                    open.extend(children);
                }
            }
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

/// Everything one issue's phase functions share, built once per run after
/// [`super::prepare_branch`]. All borrows are shared — the mutable [`super::RunLedger`]
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
