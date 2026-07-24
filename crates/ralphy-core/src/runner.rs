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
                crate::emit::non_green(issue.number, &outcome);
                let number = issue.number;
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
                });
                stop = Some(if deadline_cut {
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
mod tests {
    use super::*;

    fn labeled(number: u64, labels: &[&str]) -> Issue {
        Issue {
            number,
            title: format!("issue {number}"),
            body: String::new(),
            labels: labels.iter().map(|s| s.to_string()).collect(),
            comments: Vec::new(),
        }
    }

    #[test]
    fn first_stop_before_finds_first_and_respects_forced() {
        let queue = vec![
            labeled(1, &[]),
            labeled(2, &[STOP_BEFORE_LABEL]),
            labeled(3, &[STOP_BEFORE_LABEL]),
        ];
        // The first stop-before in order.
        assert_eq!(first_stop_before(&queue, &[]), Some(2));
        // Forcing the first stop-before skips to the next one.
        assert_eq!(first_stop_before(&queue, &[2]), Some(3));
        // Forcing every stop-before yields none.
        assert_eq!(first_stop_before(&queue, &[2, 3]), None);
        // A queue with no stop-before yields none.
        assert_eq!(first_stop_before(&[labeled(1, &[])], &[]), None);
    }

    #[test]
    fn human_return_label_matches_first_configured_label() {
        let labels = vec!["needs-info".to_string(), "wontfix".to_string()];
        let parked = labeled(5, &["queue", "wontfix"]);
        assert_eq!(
            human_return_label(&parked, &labels).map(String::as_str),
            Some("wontfix")
        );
        let plain = labeled(6, &["queue"]);
        assert_eq!(human_return_label(&plain, &labels), None);
    }

    // ------------------------------------------------------------------
    // Queue-loop tests through the injectable seams: `run_queue_with` driven
    // by a FakeRepo and FakeLedger — no on-disk git repository, no usage
    // file, no RALPHY_USAGE_DIR juggling. The workspace is a plain temp dir
    // (the `.ralphy/` scratch and the verify commands only need a
    // filesystem). Complements tests/queue.rs, which proves the same loop
    // over a real repo.
    // ------------------------------------------------------------------

    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::fs;
    use std::path::PathBuf;
    use std::time::Duration;

    use crate::ledger::LedgerRecord;
    use crate::{Execution, Plan};

    /// Constant answers for the reads, a recorder for the checkouts. The
    /// constant `head_sha` is fine here: the no-commit streak only matters on
    /// limit-resumes, which these tests do not script.
    struct FakeRepo {
        checkouts: RefCell<Vec<String>>,
    }

    impl FakeRepo {
        fn new() -> Self {
            Self {
                checkouts: RefCell::new(Vec::new()),
            }
        }
    }

    impl Repo for FakeRepo {
        fn current_branch(&self) -> Result<String> {
            Ok("main".into())
        }

        fn head_sha(&self) -> Result<String> {
            Ok("abc123".into())
        }

        fn project_slug(&self) -> String {
            "owner/repo".into()
        }

        fn checkout_new_branch(&self, branch: &str, base: &str) -> Result<()> {
            self.checkouts
                .borrow_mut()
                .push(format!("new:{branch}:{base}"));
            Ok(())
        }

        fn checkout_force(&self, refname: &str) -> Result<()> {
            self.checkouts.borrow_mut().push(format!("force:{refname}"));
            Ok(())
        }

        fn delete_branch(&self, branch: &str) -> Result<()> {
            self.checkouts.borrow_mut().push(format!("delete:{branch}"));
            Ok(())
        }

        fn rev_list_count(&self, _range: &str) -> Result<usize> {
            Ok(1)
        }

        fn log_oneline(&self, _range: &str) -> Result<Vec<String>> {
            Ok(vec!["abc123 work".into()])
        }

        fn user_email(&self) -> Option<String> {
            Some("t@example.com".into())
        }

        fn user_name(&self) -> Option<String> {
            Some("Test".into())
        }
    }

    /// Captures every ledger line in memory.
    #[derive(Default)]
    struct FakeLedger {
        records: RefCell<Vec<LedgerRecord>>,
    }

    impl LedgerSink for FakeLedger {
        fn append(&self, rec: &LedgerRecord) -> Result<()> {
            self.records.borrow_mut().push(rec.clone());
            Ok(())
        }
    }

    /// Plans a one-step, lint-clean plan (optionally carrying a `## Verify`
    /// section, or a protocol-dirty shape) and pops a scripted outcome per
    /// `execute` — never touching git.
    struct MiniAgent {
        outcomes: RefCell<VecDeque<Outcome>>,
        planned: RefCell<Vec<u64>>,
        /// Appended verbatim to the plan (e.g. a `## Verify` section).
        extra: Option<String>,
        /// Write a protocol-dirty plan (unticked step, no closing sections).
        lint_dirty: bool,
        /// On `execute`, repair the plan when the ADR-0015 bounce brief is on
        /// disk: tick every step and append the closing sections.
        fix_protocol: bool,
    }

    impl MiniAgent {
        fn new(outcomes: Vec<Outcome>) -> Self {
            Self {
                outcomes: RefCell::new(outcomes.into()),
                planned: RefCell::new(Vec::new()),
                extra: None,
                lint_dirty: false,
                fix_protocol: false,
            }
        }

        fn with_extra(mut self, extra: impl Into<String>) -> Self {
            self.extra = Some(extra.into());
            self
        }

        fn lint_dirty_with_fix(mut self) -> Self {
            self.lint_dirty = true;
            self.fix_protocol = true;
            self
        }
    }

    impl Agent for MiniAgent {
        fn name(&self) -> &'static str {
            "mini"
        }

        fn plan(&self, issue: &Issue, ws: &Workspace) -> Result<Plan> {
            self.planned.borrow_mut().push(issue.number);
            fs::create_dir_all(ws.ralphy_dir())?;
            let step = if self.lint_dirty {
                "- [ ] do a thing\n"
            } else {
                "- [x] do a thing\n"
            };
            let extra = self
                .extra
                .as_deref()
                .map(|e| format!("\n{e}\n"))
                .unwrap_or_default();
            let closing = if self.lint_dirty {
                ""
            } else {
                "\n## Handoff\n\n- **Delivered**: scripted work\n\n## Plan friction\n\n- none\n"
            };
            let body = format!(
                "# Plan for #{}\n\n## Steps\n{step}{extra}{closing}",
                issue.number
            );
            let path = ws.plan_path();
            fs::write(&path, body)?;
            Ok(Plan {
                path,
                open_steps: 1,
                recommended_model: None,
                usage: Usage {
                    output: 3,
                    model: Some("fake-model".into()),
                    ..Usage::default()
                },
                session_id: None,
            })
        }

        fn execute(&self, _plan: &Plan, ws: &Workspace) -> Result<Execution> {
            if self.fix_protocol && ws.ralphy_dir().join("protocol-failure.md").exists() {
                let plan_md = fs::read_to_string(ws.plan_path())?;
                let fixed = plan_md.replace("- [ ]", "- [x]")
                    + "\n## Handoff\n\n- **Delivered**: repaired\n\n## Plan friction\n\n- none\n";
                fs::write(ws.plan_path(), fixed)?;
            }
            let outcome = self
                .outcomes
                .borrow_mut()
                .pop_front()
                .expect("more execute calls than scripted outcomes");
            Ok(Execution {
                outcome,
                usage: Usage {
                    output: 5,
                    model: Some("fake-model".into()),
                    ..Usage::default()
                },
                session_id: None,
            })
        }
    }

    /// Records closes/comments/labels; the trait's defaults cover the rest.
    #[derive(Default)]
    struct FakeTracker {
        closes: RefCell<Vec<u64>>,
        comments: RefCell<Vec<(u64, String)>>,
        labels: RefCell<Vec<(u64, String)>>,
    }

    impl IssueTracker for FakeTracker {
        fn close(&self, number: u64, _comment: &str) -> Result<()> {
            self.closes.borrow_mut().push(number);
            Ok(())
        }

        fn comment(&self, number: u64, body: &str) -> Result<()> {
            self.comments.borrow_mut().push((number, body.to_string()));
            Ok(())
        }

        fn add_label(&self, number: u64, label: &str) -> Result<()> {
            self.labels.borrow_mut().push((number, label.to_string()));
            Ok(())
        }
    }

    /// Never expires, never sleeps.
    struct FakeClock;

    impl RunClock for FakeClock {
        fn deadline_passed(&self) -> bool {
            false
        }

        fn wait_for_reset(&self, _reset: &str) -> WaitOutcome {
            WaitOutcome::Resumed
        }
    }

    /// A fresh plain directory (no git) the workspace and verify commands run
    /// in; unique per test so parallel tests never collide.
    fn test_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("ralphy-runner-ut-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn test_cfg(root: &std::path::Path, stamp: &str) -> QueueConfig {
        QueueConfig {
            repo_root: root.to_path_buf(),
            base_branch: "main".into(),
            dry_run: false,
            stamp: stamp.into(),
            branch_mode: BranchMode::New,
            forced_issues: Vec::new(),
            stop_on_limit_plan: false,
            stop_on_limit_exec: false,
            verify_fallback: None,
            verify_timeout: Duration::from_secs(60),
            require_verify_gate: false,
            done_signal: "DONE_TOKEN".into(),
            human_return_labels: vec![
                "ready-for-human".into(),
                "HITL".into(),
                "needs-info".into(),
                "needs-triage".into(),
                "wontfix".into(),
                "triage-agent".into(),
            ],
        }
    }

    fn test_issue(number: u64) -> Issue {
        Issue {
            number,
            title: format!("issue {number}"),
            body: String::new(),
            labels: vec![],
            comments: vec![],
        }
    }

    /// A `## Verify` line whose command exits 0 on every platform.
    fn verify_ok_line() -> &'static str {
        if cfg!(windows) {
            "cmd /c \"exit 0\""
        } else {
            "sh -c \"exit 0\""
        }
    }

    /// A `## Verify` line whose command exits non-zero on every platform.
    fn verify_fail_line() -> &'static str {
        if cfg!(windows) {
            "cmd /c \"exit 3\""
        } else {
            "sh -c \"exit 3\""
        }
    }

    #[test]
    fn green_close_runs_through_fakes_only() {
        let root = test_dir("green");
        let cfg = test_cfg(&root, "ut-green");
        let repo = FakeRepo::new();
        let sink = FakeLedger::default();
        let agent = MiniAgent::new(vec![Outcome::Done])
            .with_extra(format!("## Verify\n\n{}\n", verify_ok_line()));
        let tracker = FakeTracker::default();

        let report = run_queue_with(
            &cfg,
            &[test_issue(7)],
            &agent,
            &tracker,
            &FakeClock,
            &repo,
            &sink,
        )
        .expect("run succeeds");

        assert_eq!(report.worked.len(), 1);
        assert!(report.worked[0].closed, "green issue is closed");
        assert!(report.stop.is_none());
        assert_eq!(report.commits, 1, "commit count read through the fake");
        assert_eq!(*tracker.closes.borrow(), vec![7]);

        // One plan + one execute ledger line, folded into the run totals.
        let phases: Vec<String> = sink
            .records
            .borrow()
            .iter()
            .map(|r| r.phase.clone())
            .collect();
        assert_eq!(phases, vec!["plan", "execute"]);
        assert_eq!(report.run_usage.total(), 8, "3 plan + 5 execute tokens");
        // Both phases carry the same model: the plan usage straight through,
        // and the execute usage via `Usage::fold_usage` over the resume loop's
        // attempts (#225) — the exec phase no longer drops model to `unknown`.
        assert_eq!(report.run_usage_by_model["fake-model"].total(), 8);
        assert!(!report.run_usage_by_model.contains_key("unknown"));

        // Branch lifecycle: the run branch was cut, and the clean run
        // returned to the original branch.
        let checkouts = repo.checkouts.borrow();
        assert_eq!(checkouts[0], "new:afk/run-ut-green:main");
        assert_eq!(checkouts.last().unwrap(), "force:main");

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn non_green_outcome_stops_the_run() {
        let root = test_dir("nongreen");
        let cfg = test_cfg(&root, "ut-nongreen");
        let repo = FakeRepo::new();
        let sink = FakeLedger::default();
        let agent = MiniAgent::new(vec![Outcome::Stuck]);
        let tracker = FakeTracker::default();

        let report = run_queue_with(
            &cfg,
            &[test_issue(1), test_issue(2)],
            &agent,
            &tracker,
            &FakeClock,
            &repo,
            &sink,
        )
        .expect("run succeeds");

        assert!(matches!(
            report.stop,
            Some(StopReason::NonGreen {
                number: 1,
                outcome: Outcome::Stuck
            })
        ));
        assert_eq!(report.worked.len(), 1, "issue 2 never started");
        assert_eq!(*agent.planned.borrow(), vec![1], "issue 2 never planned");
        assert!(tracker.closes.borrow().is_empty());

        // The execute ledger line carries the terminal outcome.
        let records = sink.records.borrow();
        let exec = records.iter().find(|r| r.phase == "execute").unwrap();
        assert_eq!(exec.outcome, "stuck");

        // A stopped run leaves the repo on the run branch for inspection.
        assert!(
            !repo.checkouts.borrow().iter().any(|c| c == "force:main"),
            "no return to the original branch on a stop"
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn verify_gate_failure_leaves_issue_open_and_run_continues() {
        let root = test_dir("verify-fail");
        let cfg = test_cfg(&root, "ut-vfail");
        let repo = FakeRepo::new();
        let sink = FakeLedger::default();
        // Initial execute + two repair attempts, all `Done`; the gate itself
        // keeps failing, so the repair budget is spent and the issue is left
        // open while the run marches on.
        let agent = MiniAgent::new(vec![Outcome::Done, Outcome::Done, Outcome::Done])
            .with_extra(format!("## Verify\n\n{}\n", verify_fail_line()));
        let tracker = FakeTracker::default();

        let report = run_queue_with(
            &cfg,
            &[test_issue(3)],
            &agent,
            &tracker,
            &FakeClock,
            &repo,
            &sink,
        )
        .expect("run succeeds");

        assert!(
            report.stop.is_none(),
            "verify failure does not stop the run"
        );
        assert_eq!(report.worked.len(), 1);
        assert!(!report.worked[0].closed);
        assert!(
            report.worked[0].outcome.is_none(),
            "reported skipped-on-verify"
        );
        assert!(tracker.closes.borrow().is_empty());

        // The repair phase is on the ledger with the failed-gate outcome.
        let records = sink.records.borrow();
        let repair = records.iter().find(|r| r.phase == "repair").unwrap();
        assert_eq!(repair.outcome, "verify-failed");
        assert_eq!(repair.tokens.total(), 10, "two repair executes accumulated");

        // The honesty artifact was posted on each gate run.
        assert!(tracker
            .comments
            .borrow()
            .iter()
            .any(|(n, b)| *n == 3 && b.contains("## Verify (Ralphy run ut-vfail)")));

        // The run finished cleanly, so the repo returned to the original branch.
        assert_eq!(repo.checkouts.borrow().last().unwrap(), "force:main");

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn verify_spawn_failure_short_circuits_without_repairs() {
        let root = test_dir("verify-spawn");
        let cfg = test_cfg(&root, "ut-vspawn");
        let repo = FakeRepo::new();
        let sink = FakeLedger::default();
        // A `## Verify` command that can't be spawned (a typo'd binary). The
        // MiniAgent is scripted with a SINGLE Done: if the gate handed the failure
        // back for repair, the repair execute would pop a second scripted outcome
        // and panic — so a clean run proves no repair attempt was ever spent (#182).
        let agent = MiniAgent::new(vec![Outcome::Done])
            .with_extra("## Verify\n\ndefinitely-not-a-real-binary-xyz\n");
        let tracker = FakeTracker::default();

        let report = run_queue_with(
            &cfg,
            &[test_issue(8)],
            &agent,
            &tracker,
            &FakeClock,
            &repo,
            &sink,
        )
        .expect("run succeeds");

        // Skipped, not stopped: the issue is left open, the run marches on, and
        // nothing was closed on a gate that could not run.
        assert!(report.stop.is_none(), "a spawn failure skips, never stops");
        assert_eq!(report.worked.len(), 1);
        assert!(!report.worked[0].closed);
        assert!(tracker.closes.borrow().is_empty());

        // No repair attempt was spent: only plan + the initial execute ran. A
        // repair phase would appear here (and would have panicked the MiniAgent).
        let phases: Vec<String> = sink
            .records
            .borrow()
            .iter()
            .map(|r| r.phase.clone())
            .collect();
        assert_eq!(
            phases,
            vec!["plan", "execute"],
            "no repair phase — the budget was untouched"
        );

        // The honesty artifact names it a spec/spawn problem, not a test failure.
        assert!(tracker.comments.borrow().iter().any(|(n, b)| *n == 8
            && b.contains("## Verify (Ralphy run ut-vspawn)")
            && b.contains("spec/spawn problem")));

        // The run finished cleanly, so the repo returned to the original branch.
        assert_eq!(repo.checkouts.borrow().last().unwrap(), "force:main");

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn protocol_bounce_repairs_then_closes() {
        let root = test_dir("protocol");
        let cfg = test_cfg(&root, "ut-protocol");
        let repo = FakeRepo::new();
        let sink = FakeLedger::default();
        // First execute claims Done over a protocol-dirty plan; the lint
        // bounces the session back once, the executor repairs, the re-lint
        // passes, and the issue closes (no `## Verify` → warn-and-close).
        let agent = MiniAgent::new(vec![Outcome::Done, Outcome::Done]).lint_dirty_with_fix();
        let tracker = FakeTracker::default();

        let report = run_queue_with(
            &cfg,
            &[test_issue(4)],
            &agent,
            &tracker,
            &FakeClock,
            &repo,
            &sink,
        )
        .expect("run succeeds");

        assert_eq!(report.worked.len(), 1);
        assert!(report.worked[0].closed, "closed after the repaired bounce");
        assert_eq!(*tracker.closes.borrow(), vec![4]);

        // The bounce is its own ledger phase, settled green.
        let phases: Vec<String> = sink
            .records
            .borrow()
            .iter()
            .map(|r| r.phase.clone())
            .collect();
        assert_eq!(phases, vec!["plan", "execute", "protocol-repair"]);
        let records = sink.records.borrow();
        let bounce = records
            .iter()
            .find(|r| r.phase == "protocol-repair")
            .unwrap();
        assert_eq!(bounce.outcome, "done");

        let _ = fs::remove_dir_all(&root);
    }
}
