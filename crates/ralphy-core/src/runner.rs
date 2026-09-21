//! The run lifecycle: cut a fresh branch off the base, ask the agent to plan,
//! and (on a dry run) return the repo to where it started, dropping the empty
//! run branch. Execution is a later slice; this slice stops after planning.

use std::collections::BTreeMap;

use anyhow::Result;
use tracing::{info, warn};

use crate::{
    ledger::{FileLedger, LedgerSink},
    repo::{GitRepo, Repo},
    Agent, Issue, IssueTracker, Outcome, Usage, Workspace,
};

mod artifacts;
mod branch;
mod clock;
mod comments;
mod phases;
mod types;

pub(crate) use branch::prepare_branch;
#[allow(unused_imports)]
pub use branch::BranchMode;
#[allow(unused_imports)]
pub use clock::{synthetic_reset, RunClock, WaitOutcome, WallClock};
pub(crate) use comments::no_gate_comment;
pub(crate) use phases::{
    close_and_record, execute_phase, open_blockers, plan_phase, prepare_issue, protocol_gate,
    verify_gate, ExecPhase, IssueCtx, PlanPhase, Prepared, ProtocolGate, VerifyGate,
};
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
    let _ = std::fs::create_dir_all(ws.ralphy_dir());
    crate::environment::ensure_brief(&ws);

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
        invocations: 0,
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
        // The operator's stop outranks the clock (docs/adr/0054): a run stopped
        // on the same tick its deadline lapsed must report the button, not the
        // budget, or the panel tells the operator something they did not do.
        // Between issues nothing is in flight, so no number is named.
        if crate::stop::requested() {
            crate::emit::run_stopped(None);
            stop = Some(StopReason::Stopped { number: None });
            break;
        }

        // Don't start a new issue past the global budget. Work already committed
        // for earlier issues is kept; the branch is handed back as it stands.
        if clock.deadline_passed() {
            crate::emit::deadline_passed(issue.number);
            stop = Some(StopReason::Deadline);
            break;
        }

        // Stop-before: a flow-control label that pauses the run before the tagged
        // issue. An explicitly named issue (`--only-issue`/`--issues`) overrides it
        // — the queue was pre-filtered to that selection, so the operator clearly
        // wants it to run.
        if first_stop_before(std::slice::from_ref(issue), &cfg.forced_issues).is_some() {
            crate::emit::stop_before_label(issue.number);
            stop = Some(StopReason::StopBefore {
                number: issue.number,
            });
            break;
        }

        // Human-return precedence (ADR-0016): a label that returns the issue to a
        // human outranks its queue label. Skip with a recorded reason and CONTINUE
        // the queue (unlike stop-before, which halts). `forced_issues` does NOT
        // override this: the label may record someone else's state (a reporter
        // owing info, a parked verify gate) that a run flag must not steamroll.
        if let Some(label) = human_return_label(issue, &cfg.human_return_labels) {
            crate::emit::human_return_label(issue.number, label);
            worked.push(IssueResult {
                number: issue.number,
                outcome: None,
                closed: false,
                blocked_by: Vec::new(),
                human_blockers: Vec::new(),
                status: ResultStatus::Skipped,
                skip: Some(SkipReason::HumanReturn),
                review_only: 0,
            });
            continue;
        }

        // Gate and stage the issue (blocked-by, comment enrichment, `.ralphy/`
        // staging). A preparation error is fatal: restore and propagate.
        let issue = match prepare_issue(&cx, issue) {
            Ok(Prepared::Ready(enriched)) => enriched,
            Ok(Prepared::Blocked { open, human }) => {
                // Mirrors the emitter split in `prepare_issue`: a blocker that
                // is a human gate parks the issue as HITL, not a plain skip.
                let status = if human.is_empty() {
                    ResultStatus::Skipped
                } else {
                    ResultStatus::Hitl
                };
                worked.push(IssueResult {
                    number: issue.number,
                    outcome: None,
                    closed: false,
                    blocked_by: open,
                    human_blockers: human,
                    status,
                    skip: (status == ResultStatus::Skipped).then_some(SkipReason::BlockedBy),
                    review_only: 0,
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
            Ok(PlanPhase::Infeasible { needs_split }) => {
                worked.push(IssueResult {
                    number: issue.number,
                    outcome: None,
                    closed: false,
                    blocked_by: Vec::new(),
                    human_blockers: Vec::new(),
                    status: if needs_split {
                        ResultStatus::NeedsSplit
                    } else {
                        ResultStatus::Infeasible
                    },
                    skip: None,
                    review_only: 0,
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
                    status: ResultStatus::NonGreen,
                    skip: None,
                    review_only: 0,
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
                status: ResultStatus::Planned,
                skip: None,
                review_only: 0,
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
                let number = issue.number;
                // The event has to agree with the stop reason below, or the live
                // trail reports a non-green halt for a run the operator stopped
                // while the final panel says something else entirely.
                if crate::stop::requested() {
                    crate::emit::run_stopped(Some(number));
                } else {
                    crate::emit::non_green(number, &outcome);
                }
                worked.push(IssueResult {
                    number,
                    outcome: Some(outcome.clone()),
                    closed: false,
                    blocked_by: Vec::new(),
                    human_blockers: Vec::new(),
                    // Mirrors the fold's `outcome.starts_with("Blocked")` split.
                    status: if matches!(outcome, Outcome::Blocked(_)) {
                        ResultStatus::Blocked
                    } else {
                        ResultStatus::NonGreen
                    },
                    skip: None,
                    review_only: 0,
                });
                // The operator's stop outranks every other reading of this
                // outcome (docs/adr/0054). A stop-killed child ends with no
                // verdict, so the ladder reports `Stuck` — reporting THAT would
                // tell the operator their agent got wedged, when in fact they
                // pressed a button. The `IssueResult` above keeps the adapter's
                // real outcome; only the RUN's stop reason is overridden.
                stop = Some(if crate::stop::requested() {
                    StopReason::Stopped {
                        number: Some(number),
                    }
                } else if deadline_cut {
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

        // A stop that landed on a GREEN execute (docs/adr/0054). Without this the
        // run would go on to the protocol lint and then sit in the verify gate —
        // routinely minutes of test suite — after the operator asked it to stop.
        // The issue is recorded as worked-but-not-delivered and left OPEN: its
        // commits are on the branch, but nothing verified them, so closing it
        // here would be the run vouching for work it never checked.
        if crate::stop::requested() {
            let number = issue.number;
            crate::emit::run_stopped(Some(number));
            worked.push(IssueResult {
                number,
                outcome: None,
                closed: false,
                blocked_by: Vec::new(),
                human_blockers: Vec::new(),
                status: ResultStatus::NonGreen,
                skip: None,
                review_only: 0,
            });
            stop = Some(StopReason::Stopped {
                number: Some(number),
            });
            break;
        }

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
                    status: ResultStatus::NonGreen,
                    skip: None,
                    review_only: 0,
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
                    status: ResultStatus::NonGreen,
                    skip: None,
                    review_only: 0,
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
                crate::emit::verify_gate_failed(number, &summary);
                worked.push(IssueResult {
                    number,
                    outcome: None,
                    closed: false,
                    blocked_by: Vec::new(),
                    human_blockers: Vec::new(),
                    status: ResultStatus::Skipped,
                    skip: Some(SkipReason::VerifyFailed),
                    review_only: 0,
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
                    status: ResultStatus::Done,
                    skip: None,
                    review_only: 0,
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
        invocations: ledger.invocations,
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
mod tests;
