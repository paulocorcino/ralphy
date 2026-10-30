//! The run lifecycle: cut a fresh branch off the base, ask the agent to plan,
//! and (on a dry run) return the repo to where it started, dropping the empty
//! run branch. Execution is a later slice; this slice stops after planning.

use std::collections::BTreeMap;

use anyhow::{Context, Result};
use tracing::warn;

use crate::{
    ledger::{FileLedger, LedgerSink},
    repo::{GitRepo, Repo},
    Agent, Issue, IssueTracker, Usage, Workspace,
};

mod artifacts;
mod branch;
mod clock;
mod comments;
mod phases;
mod queue_run;
mod types;

pub(crate) use branch::prepare_branch;
pub use branch::BranchMode;
pub use clock::{synthetic_reset, RunClock, WaitOutcome, WallClock};
pub(crate) use comments::no_gate_comment;
pub(crate) use phases::{
    close_and_record, execute_phase, open_blockers, plan_phase, prepare_issue, protocol_gate,
    verify_gate, ExecPhase, IssueCtx, PlanPhase, Prepared, ProtocolGate, VerifyGate,
};
use queue_run::{Early, QueueRun};
pub(crate) use types::RunLedger;
pub use types::{IssueResult, QueueConfig, QueueReport, ResultStatus, SkipReason, StopReason};

/// The label that pauses the run before the tagged issue (flow-control, not triage).
pub const STOP_BEFORE_LABEL: &str = "stop-before";

/// The fixed operational label marking an issue awaiting an agent triage pass
/// (`ralphy triage`, ADR-0017). Like `stop-before`/`AFK`/`HITL` it lives outside
/// the five canonical triage roles and outside the setup-pocock mapping table,
/// so it is never resolved through `triage-labels.md`. It is also a human-return
/// label under ADR-0016: while present the issue is parked out of the run queue,
/// so triage and run never race.
pub const TRIAGE_AGENT_LABEL: &str = "triage-agent";

/// The label applied to an issue the planner judged a bundle (multiple backlog
/// tasks under one number): the queue is parked on a human running `/to-issues`
/// to open the children (`## Parent: #N`) and close the bundle — the
/// follow-the-split blocker gate handles the rest. Fixed-name like
/// `stop-before`/`triage-agent`, never resolved through `triage-labels.md`.
pub const NEEDS_SPLIT_LABEL: &str = "needs-split";

/// The label applied to an issue the runner closed **green** whose acceptance
/// ledger still carried `[review-only]` criteria — delivered work whose evidence
/// is "a human reads it", which the runner cannot self-certify. It marks
/// *attention* debt, not unfinished work, and deliberately sits outside
/// [`HUMAN_GATE_LABELS`]: a closed issue must never re-enter the ADR-0014
/// blocker path. Fixed-name, never resolved through `triage-labels.md`.
pub const NEEDS_HUMAN_REVIEW_LABEL: &str = "needs-human-review";

/// Labels that mark an issue as a human gate (ADR-0014): a blocker parked until a
/// person acts, not agent work the queue will clear. The canonical
/// `ready-for-human` triage role and its fixed `HITL` alias (ADR-0001). A human
/// gate is never a queue member (it is never queried), so it only ever surfaces
/// as a *blocker* in another issue's `## Blocked by`.
pub const HUMAN_GATE_LABELS: [&str; 2] = ["ready-for-human", "HITL"];

/// The first queued issue carrying [`STOP_BEFORE_LABEL`] whose number is NOT in
/// `forced` — the point the run halts before, or `None` when the queue has no
/// (non-forced) stop-before. An explicit selection (`--only-issue`/`--issues`)
/// suppresses the label on exactly those numbers, so a forced stop-before runs
/// normally. Shared by the runner loop, the CLI's `queue built` boundary, and
/// [`crate::queue_view::resolve_queue_view`] so all three agree on where a run
/// stops.
pub fn first_stop_before(queue: &[Issue], forced: &[u64]) -> Option<u64> {
    queue
        .iter()
        .find(|i| !forced.contains(&i.number) && i.labels.iter().any(|l| l == STOP_BEFORE_LABEL))
        .map(|i| i.number)
}

/// The human-return label (ADR-0016) on `issue`, if any: the first of its labels
/// that appears in `human_return_labels`. Such a label outranks the queue label,
/// so the runner skips the issue with this label as the reason. Shared by the
/// runner loop and [`crate::queue_view::resolve_queue_view`] so both classify a
/// parked issue identically. Unlike `stop-before`, a forced selection does NOT
/// suppress it (the label may record someone else's state).
pub fn human_return_label<'a>(
    issue: &'a Issue,
    human_return_labels: &[String],
) -> Option<&'a String> {
    issue
        .labels
        .iter()
        .find(|l| human_return_labels.iter().any(|h| h == *l))
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
#[expect(
    clippy::too_many_arguments,
    reason = "the queue config, the queue and the five collaborators the unit tests replace"
)]
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

    // Write the build-environment brief once (no-op if it already exists) so the
    // planner and executor see the machine their `## Verify`/smoke commands run
    // on, before the first plan pass reads it.
    std::fs::create_dir_all(ws.ralphy_dir())
        .with_context(|| format!("creating {}", ws.ralphy_dir().display()))?;
    crate::environment::ensure_brief(&ws);

    let (orig, branch, compare_ref) = prepare_branch(
        repo,
        &cfg.repo_root,
        &cfg.base_branch,
        &cfg.stamp,
        cfg.branch_mode,
    )?;

    let mut undo_tag = create_undo_tag(repo, &cfg.stamp, &compare_ref);

    // Identity for every ledger line this run writes (ADR-0008 D6/D7), read once
    // from git: the project slug (remote, or a path-hash fallback) and the actor.
    // The accumulators fold every phase's usage into the run totals; the per-model
    // split feeds the read-time USD footer (D8).
    let ledger = RunLedger {
        sink,
        project: repo.project_slug(),
        actor_email: repo.user_email().unwrap_or_default(),
        actor_name: repo.user_name().unwrap_or_default(),
        agent: agent.name(),
        run_usage: Usage::default(),
        run_usage_by_model: BTreeMap::new(),
        invocations: 0,
    };

    let mut run = QueueRun {
        cx: IssueCtx {
            cfg,
            ws: &ws,
            repo,
            agent,
            tracker,
            clock,
            branch: &branch,
        },
        orig: &orig,
        ledger,
        worked: Vec::new(),
    };
    let mut stop: Option<StopReason> = None;

    for issue in queue {
        match run.issue(issue) {
            Ok(()) | Err(Early::Next) => {}
            Err(Early::Stop(reason)) => {
                stop = Some(reason);
                break;
            }
            Err(Early::Fail(e)) => return Err(e),
        }
    }
    let QueueRun { ledger, worked, .. } = run;
    let RunLedger {
        run_usage,
        run_usage_by_model,
        invocations,
        ..
    } = ledger;

    // Count what the run added over the compare ref and capture the oneline log,
    // matching the ps1 `finally` block. Failures here are non-fatal reporting
    // concerns (e.g. a dropped branch in cleanup) — default to zero / empty.
    let range = format!("{compare_ref}..{branch}");
    let commits = repo.rev_list_count(&range).unwrap_or(0);
    let oneline = repo.log_oneline(&range).unwrap_or_default();

    if commits == 0 {
        drop_undo_tag(repo, &mut undo_tag);
    }
    close_branch(repo, cfg, &orig, &branch, stop.is_none());

    Ok(QueueReport {
        branch,
        orig_branch: orig,
        worked,
        stop,
        commits,
        undo_tag,
        oneline,
        run_usage,
        run_usage_by_model,
        invocations,
    })
}

/// Pre-run undo marker: a local tag at the compare ref (the base in `New`
/// mode, the pre-run HEAD in `Current` mode), so undoing the whole run is one
/// copyable command instead of reflog archaeology. Best-effort — a run must
/// never fail over its own bookkeeping.
fn create_undo_tag(repo: &dyn Repo, stamp: &str, compare_ref: &str) -> Option<String> {
    let name = format!("ralphy/pre-run-{stamp}");
    match repo.tag(&name, compare_ref) {
        Ok(()) => Some(name),
        Err(e) => {
            warn!(tag = %name, error = %e, "creating the pre-run undo tag failed");
            None
        }
    }
}

/// A run that added nothing has nothing to undo — drop the marker so tags
/// never accumulate for dry runs and empty queues (mirrors the empty-branch
/// delete in `restore`).
fn drop_undo_tag(repo: &dyn Repo, undo_tag: &mut Option<String>) {
    if let Some(tag) = undo_tag.take() {
        if let Err(e) = repo.delete_tag(&tag) {
            warn!(%tag, error = %e, "deleting the empty run's undo tag failed");
        }
    }
}

/// Closing-state matrix, keyed on mode × outcome × dry-run (ps1 `finally`):
///  - Current: commits already live on the branch — never check out or delete.
///  - New + dry-run: plans only — return to orig and drop the empty branch.
///  - New + stop: leave the repo on the run branch for inspection.
///  - New + clean run: return to orig; the run branch is kept (not deleted).
fn close_branch(repo: &dyn Repo, cfg: &QueueConfig, orig: &str, branch: &str, clean: bool) {
    match cfg.branch_mode {
        BranchMode::Current => {}
        BranchMode::New => {
            if cfg.dry_run {
                restore(repo, orig, branch, &cfg.base_branch, cfg.branch_mode);
            } else if clean {
                // Force, same as `restore`: `.ralphy/` scratch may modify a
                // tracked file (e.g. a plan.md committed on the base), and a
                // non-force checkout would abort and strand the repo on the
                // run branch after an otherwise green run (ADR-0005, #41).
                if let Err(e) = repo.checkout_force(orig) {
                    warn!("could not return to '{orig}': {e}");
                }
            }
        }
    }
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
mod tests;
