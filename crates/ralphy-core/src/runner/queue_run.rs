//! The per-issue steps of the queue loop (ADR-0022 split of `runner.rs`).
//! [`QueueRun::issue`] walks one queue member through the gates and phases in
//! order; every step that ends the issue early says how through [`Early`], so
//! `run_queue_with` only has to read that answer.

use tracing::{info, warn};

use super::artifacts::{clear_protocol_failure, clear_verify_failure};
use super::{
    close_and_record, execute_phase, first_stop_before, human_return_label, no_gate_comment,
    plan_phase, prepare_issue, protocol_gate, restore, verify_gate, ExecPhase, IssueCtx,
    IssueResult, PlanPhase, Prepared, ProtocolGate, ResultStatus, RunLedger, SkipReason,
    StopReason, VerifyGate, HUMAN_GATE_LABELS,
};
use crate::{Issue, Outcome, Plan, ResetAt, Usage};

/// Why a step ended its issue before the close.
pub(super) enum Early {
    /// The issue is recorded; the loop goes on to the next one.
    Next,
    /// The run halts for this reason.
    Stop(StopReason),
    /// A fatal error: the run returns it.
    Fail(anyhow::Error),
}

impl From<anyhow::Error> for Early {
    fn from(e: anyhow::Error) -> Self {
        Early::Fail(e)
    }
}

/// What the protocol gate leaves for the verify gate and the close.
struct Settled {
    lint: crate::protocol::ProtocolReport,
    plan_md: String,
    protocol_usage: Usage,
}

/// The state one run carries from issue to issue.
pub(super) struct QueueRun<'a> {
    pub(super) cx: IssueCtx<'a>,
    pub(super) orig: &'a str,
    pub(super) ledger: RunLedger<'a>,
    pub(super) worked: Vec<IssueResult>,
}

/// An `IssueResult` with nothing set beyond its number and status.
fn result(number: u64, status: ResultStatus) -> IssueResult {
    IssueResult {
        number,
        outcome: None,
        closed: false,
        blocked_by: Vec::new(),
        human_blockers: Vec::new(),
        status,
        skip: None,
        review_only: 0,
    }
}

/// The run's stop reason for a non-green execute. The operator's stop outranks
/// every other reading of the outcome (docs/adr/0054): a stop-killed child ends
/// with no verdict, so the ladder reports `Stuck`, and reporting THAT would tell
/// the operator their agent got wedged when they pressed a button.
fn non_green_stop(number: u64, outcome: Outcome, deadline_cut: bool) -> StopReason {
    if crate::stop::requested() {
        return StopReason::Stopped {
            number: Some(number),
        };
    }
    if deadline_cut {
        return StopReason::Deadline;
    }
    match outcome {
        Outcome::Limit(reset) => StopReason::Limit { number, reset },
        other @ (Outcome::Done | Outcome::Blocked(_) | Outcome::Timeout | Outcome::Stuck) => {
            StopReason::NonGreen {
                number,
                outcome: other,
            }
        }
    }
}

impl QueueRun<'_> {
    /// Work one queue member: gates, plan, execute, protocol lint, verify, close.
    pub(super) fn issue(&mut self, issue: &Issue) -> Result<(), Early> {
        self.preflight(issue)?;
        let issue = self.prepare(issue)?;
        let issue = &issue;
        let (plan, executed) = self.plan(issue)?;

        // A dry run plans only — it executes nothing and closes nothing.
        if self.cx.cfg.dry_run {
            self.worked
                .push(result(issue.number, ResultStatus::Planned));
            return Ok(());
        }

        let exec_usage = self.execute(issue, &plan, executed)?;
        self.stop_if_requested(issue.number)?;
        let Settled {
            lint,
            plan_md,
            protocol_usage,
        } = self.protocol(issue, &plan)?;
        let repair_usage = self.verify(issue, &plan, &plan_md)?;

        // Close the cycle and publish what the session leaves behind.
        close_and_record(
            &self.cx,
            issue,
            &plan,
            &lint,
            &exec_usage,
            &protocol_usage,
            &repair_usage,
            &mut self.worked,
        )?;
        Ok(())
    }

    /// The gates that run before the issue is touched.
    fn preflight(&mut self, issue: &Issue) -> Result<(), Early> {
        // The operator's stop outranks the clock (docs/adr/0054): a run stopped
        // on the same tick its deadline lapsed must report the button, not the
        // budget, or the panel tells the operator something they did not do.
        // Between issues nothing is in flight, so no number is named.
        if crate::stop::requested() {
            crate::emit::run_stopped(None);
            return Err(Early::Stop(StopReason::Stopped { number: None }));
        }

        // Don't start a new issue past the global budget. Work already committed
        // for earlier issues is kept; the branch is handed back as it stands.
        if self.cx.clock.deadline_passed() {
            crate::emit::deadline_passed(issue.number);
            return Err(Early::Stop(StopReason::Deadline));
        }

        // Stop-before: a flow-control label that pauses the run before the tagged
        // issue. An explicitly named issue (`--only-issue`/`--issues`) overrides it
        // — the queue was pre-filtered to that selection, so the operator clearly
        // wants it to run.
        if first_stop_before(std::slice::from_ref(issue), &self.cx.cfg.forced_issues).is_some() {
            crate::emit::stop_before_label(issue.number);
            return Err(Early::Stop(StopReason::StopBefore {
                number: issue.number,
            }));
        }

        // Human-return precedence (ADR-0016): a label that returns the issue to a
        // human outranks its queue label. Skip with a recorded reason and CONTINUE
        // the queue (unlike stop-before, which halts). `forced_issues` does NOT
        // override this: the label may record someone else's state (a reporter
        // owing info, a parked verify gate) that a run flag must not steamroll.
        if let Some(label) = human_return_label(issue, &self.cx.cfg.human_return_labels) {
            crate::emit::human_return_label(issue.number, label);
            self.worked.push(IssueResult {
                skip: Some(SkipReason::HumanReturn),
                ..result(issue.number, ResultStatus::Skipped)
            });
            return Err(Early::Next);
        }
        Ok(())
    }

    /// Restore the repo and turn `e` into the fatal answer.
    fn fail(&self, e: anyhow::Error) -> Early {
        let cx = &self.cx;
        restore(
            cx.repo,
            self.orig,
            cx.branch,
            &cx.cfg.base_branch,
            cx.cfg.branch_mode,
        );
        Early::Fail(e)
    }

    /// Gate and stage the issue (blocked-by, comment enrichment, `.ralphy/`
    /// staging). A preparation error is fatal: restore and propagate.
    fn prepare(&mut self, issue: &Issue) -> Result<Issue, Early> {
        match prepare_issue(&self.cx, issue) {
            Ok(Prepared::Ready(enriched)) => Ok(enriched),
            Ok(Prepared::Blocked { open, human }) => {
                // Mirrors the emitter split in `prepare_issue`: a blocker that
                // is a human gate parks the issue as HITL, not a plain skip.
                let status = if human.is_empty() {
                    ResultStatus::Skipped
                } else {
                    ResultStatus::Hitl
                };
                self.worked.push(IssueResult {
                    blocked_by: open,
                    human_blockers: human,
                    skip: (status == ResultStatus::Skipped).then_some(SkipReason::BlockedBy),
                    ..result(issue.number, status)
                });
                Err(Early::Next)
            }
            Err(e) => Err(self.fail(e)),
        }
    }

    /// Record an issue the operator's stop cut short (docs/adr/0054 D6): worked
    /// but not delivered, left open, with no adapter verdict of its own.
    fn record_stop(&mut self, number: u64) -> StopReason {
        crate::emit::run_stopped(Some(number));
        self.worked.push(result(number, ResultStatus::NonGreen));
        StopReason::Stopped {
            number: Some(number),
        }
    }

    /// Record an issue that stopped the run on a usage limit.
    fn limit_stop(&mut self, number: u64, reset: Option<ResetAt>) -> Early {
        self.worked.push(IssueResult {
            outcome: Some(Outcome::Limit(reset.clone())),
            ..result(number, ResultStatus::NonGreen)
        });
        Early::Stop(StopReason::Limit { number, reset })
    }

    /// Plan the issue; a non-limit planning failure restores and propagates.
    /// The result says whether the plan on disk was already executed.
    fn plan(&mut self, issue: &Issue) -> Result<(Plan, bool), Early> {
        let planned = plan_phase(&self.cx, issue, &mut self.ledger);
        // The operator's stop outranks whatever planning returned (docs/adr/0054):
        // a reaped planner wrote no plan (an `Err`), a stopped limit wait reads as
        // `StopDeadline`, and a plan that did finish must not start an executor.
        if crate::stop::requested() {
            if let Err(e) = &planned {
                info!(number = issue.number, error = %e, "planning ended by the operator's stop");
            }
            return Err(Early::Stop(self.record_stop(issue.number)));
        }
        match planned {
            Ok(PlanPhase::Planned(plan)) => Ok((plan, false)),
            Ok(PlanPhase::Executed(plan)) => Ok((plan, true)),
            Ok(PlanPhase::Infeasible { needs_split }) => {
                let status = if needs_split {
                    ResultStatus::NeedsSplit
                } else {
                    ResultStatus::Infeasible
                };
                self.worked.push(result(issue.number, status));
                Err(Early::Next)
            }
            Ok(PlanPhase::StopLimit { reset }) => Err(self.limit_stop(issue.number, reset)),
            Ok(PlanPhase::StopDeadline) => Err(Early::Stop(StopReason::Deadline)),
            Err(e) => Err(self.fail(e)),
        }
    }

    /// Execute the issue; any non-green terminal outcome stops the whole run —
    /// later issues are untouched. A plan already executed by an earlier run
    /// goes straight to the gates.
    fn execute(&mut self, issue: &Issue, plan: &Plan, executed: bool) -> Result<Usage, Early> {
        let exec = if executed {
            clear_verify_failure(self.cx.ws);
            clear_protocol_failure(self.cx.ws);
            ExecPhase::Done {
                exec_usage: Usage::default(),
            }
        } else {
            match execute_phase(&self.cx, issue, plan, &mut self.ledger) {
                Ok(exec) => exec,
                // A refused session is a fault of the machine: restore the repo
                // and end the run. Other execute errors keep their path.
                Err(e) if e.downcast_ref::<crate::HooksUnproven>().is_some() => {
                    return Err(self.fail(e));
                }
                Err(e) => return Err(e.into()),
            }
        };
        match exec {
            ExecPhase::Done { exec_usage } => Ok(exec_usage),
            ExecPhase::NonGreen {
                outcome,
                deadline_cut,
            } => Err(self.non_green(issue.number, outcome, deadline_cut)),
        }
    }

    /// Record a non-green execute and say how the run stops.
    fn non_green(&mut self, number: u64, outcome: Outcome, deadline_cut: bool) -> Early {
        // The event has to agree with the stop reason, or the live trail reports
        // a non-green halt for a run the operator stopped while the final panel
        // says something else entirely.
        if crate::stop::requested() {
            crate::emit::run_stopped(Some(number));
        } else {
            crate::emit::non_green(number, &outcome);
        }
        self.worked.push(IssueResult {
            outcome: Some(outcome.clone()),
            // Mirrors the fold's `outcome.starts_with("Blocked")` split.
            ..result(
                number,
                if matches!(outcome, Outcome::Blocked(_)) {
                    ResultStatus::Blocked
                } else {
                    ResultStatus::NonGreen
                },
            )
        });
        // The `IssueResult` above keeps the adapter's real outcome; only the
        // RUN's stop reason is overridden by the operator's stop.
        Early::Stop(non_green_stop(number, outcome, deadline_cut))
    }

    /// A stop that landed on a GREEN execute (docs/adr/0054). Without this the
    /// run would go on to the protocol lint and then sit in the verify gate —
    /// routinely minutes of test suite — after the operator asked it to stop.
    /// The issue is recorded as worked-but-not-delivered and left OPEN: its
    /// commits are on the branch, but nothing verified them, so closing it
    /// here would be the run vouching for work it never checked.
    fn stop_if_requested(&mut self, number: u64) -> Result<(), Early> {
        if crate::stop::requested() {
            return Err(Early::Stop(self.record_stop(number)));
        }
        Ok(())
    }

    /// Structurally lint the finished plan, with one bounce back to the
    /// executor on a violation (ADR-0015).
    fn protocol(&mut self, issue: &Issue, plan: &Plan) -> Result<Settled, Early> {
        match protocol_gate(&self.cx, issue, plan, &mut self.ledger)? {
            ProtocolGate::Settled {
                lint,
                plan_md,
                protocol_usage,
            } => Ok(Settled {
                lint,
                plan_md,
                protocol_usage,
            }),
            ProtocolGate::StopLimit { reset } => Err(self.limit_stop(issue.number, reset)),
        }
    }

    /// Re-run the plan's `## Verify` commands over the committed state before
    /// trusting the self-report (ADR-0011/0015).
    fn verify(&mut self, issue: &Issue, plan: &Plan, plan_md: &str) -> Result<Usage, Early> {
        let number = issue.number;
        match verify_gate(&self.cx, issue, plan, plan_md, &mut self.ledger)? {
            VerifyGate::Green { repair_usage } => Ok(repair_usage),
            VerifyGate::StopLimit { reset } => Err(self.limit_stop(number, reset)),
            VerifyGate::Failed { summary } => {
                // A verify failure no longer halts the queue: the repair budget is
                // spent, so leave THIS issue open (its commits stay on the branch
                // for a human to pick up — see the artifact comment) and march on
                // to the next issue. The issue is reported skipped-on-verify so the
                // miss is visible, never a silent close.
                crate::emit::verify_gate_failed(number, &summary);
                self.worked.push(IssueResult {
                    skip: Some(SkipReason::VerifyFailed),
                    ..result(number, ResultStatus::Skipped)
                });
                Err(Early::Next)
            }
            VerifyGate::NeedsHuman => {
                self.park_for_human(number);
                Err(Early::Next)
            }
        }
    }

    /// ADR-0015: the one hole where a false self-report closed an issue
    /// unchecked is now a human gate. Label + comment are best-effort — the
    /// issue staying OPEN is the guarantee, and a failed label must not abort
    /// the rest of the queue.
    fn park_for_human(&mut self, number: u64) {
        let cx = &self.cx;
        if let Err(e) = cx.tracker.add_label(number, HUMAN_GATE_LABELS[0]) {
            warn!(number, error = %e, "applying ready-for-human label failed");
        }
        if let Err(e) = cx
            .tracker
            .comment(number, &no_gate_comment(&cx.cfg.stamp, cx.branch))
        {
            warn!(number, error = %e, "posting the no-gate comment failed");
        }
        // consumed by the telegram notifier / presenter — keep stable
        info!(
            number,
            "no verify gate — issue left open for a human, run continues"
        );
        self.worked.push(IssueResult {
            outcome: Some(Outcome::Done),
            ..result(number, ResultStatus::Done)
        });
    }
}
