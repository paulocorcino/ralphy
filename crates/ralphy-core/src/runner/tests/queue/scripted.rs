//! The scripted agent, the recording tracker and the recording clock of the queue
//! tests that run on the fake repository: the agent writes files but never runs git.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::rc::Rc;

use anyhow::Result;

use crate::{
    Agent, Execution, Issue, IssueTracker, Outcome, Plan, PlanLimit, ResetAt, RunClock, Usage,
    Verdict, WaitOutcome, Workspace,
};

/// A scripted planning result: `plan()` fails with a `PlanLimit` carrying the reset.
pub(super) enum PlanScript {
    Limit(Option<ResetAt>),
}

/// Plans a feasible step for every issue and returns a scripted sequence of
/// execution outcomes (one per `execute` call), recording in order the issue
/// numbers it planned and executed. Each execute writes a unique marker file in
/// the workspace, so a verify command can see the work of a repair. A commit is
/// an advance of `head`, the counter the fake repository reads its `HEAD` from.
#[derive(Default)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "each bool scripts one independent behavior of the fake agent, not a state"
)]
pub(super) struct ScriptedAgent {
    /// Each entry is `(outcome, commits)`; an execute with no entry left is `Done`.
    outcomes: RefCell<VecDeque<(Outcome, bool)>>,
    pub(super) head: Rc<Cell<u32>>,
    plan_scripts: RefCell<VecDeque<PlanScript>>,
    /// Every `plan` call, including the ones that fail with a limit.
    pub(super) plan_attempts: Cell<usize>,
    /// Open steps `plan` writes; 0 scripts an infeasible issue.
    steps: usize,
    /// Appended to the plan as an `## Acceptance ledger` section.
    ledger: Option<String>,
    /// Leave the default `## Acceptance ledger` out of a lint-clean plan.
    omit_ledger: bool,
    /// The usage each `execute` hands back, one per call, in order.
    exec_usages: RefCell<VecDeque<Usage>>,
    pub(super) planned: RefCell<Vec<u64>>,
    pub(super) executed: RefCell<Vec<u64>>,
    /// Appended verbatim to every plan body.
    extra: Option<String>,
    /// Per-issue plan extras that override `extra` for the matching issue.
    extra_by_issue: Vec<(u64, String)>,
    /// Write a protocol-dirty plan (unticked step, no closing sections).
    lint_dirty: bool,
    /// Repair the plan when the bounce brief `protocol-failure.md` is present.
    fix_protocol: bool,
    /// Hand back a plan whose steps are all checked, with zero open steps.
    already_executed: bool,
    /// The session id each `execute` reports, one per call, in order.
    sessions: RefCell<VecDeque<Option<String>>>,
}

impl ScriptedAgent {
    /// Script a sequence of outcomes, each with a commit; once exhausted, every
    /// execute is `Done` with a commit.
    pub(super) fn new(outcomes: Vec<Outcome>) -> Self {
        Self::scripted(outcomes.into_iter().map(|o| (o, true)).collect())
    }

    /// Script `(outcome, commits)` pairs: `commits` false leaves `HEAD` unchanged.
    pub(super) fn scripted(outcomes: Vec<(Outcome, bool)>) -> Self {
        Self {
            outcomes: RefCell::new(outcomes.into()),
            steps: 1,
            ..Self::default()
        }
    }

    pub(super) fn lint_dirty(mut self) -> Self {
        self.lint_dirty = true;
        self
    }

    pub(super) fn with_protocol_fix(mut self) -> Self {
        self.fix_protocol = true;
        self
    }

    pub(super) fn with_plan_extra(mut self, extra: impl Into<String>) -> Self {
        self.extra = Some(extra.into());
        self
    }

    pub(super) fn with_plan_extra_for(mut self, number: u64, extra: impl Into<String>) -> Self {
        self.extra_by_issue.push((number, extra.into()));
        self
    }

    pub(super) fn already_executed(mut self) -> Self {
        self.already_executed = true;
        self
    }

    pub(super) fn with_sessions(self, sessions: Vec<Option<String>>) -> Self {
        *self.sessions.borrow_mut() = sessions.into();
        self
    }

    pub(super) fn with_ledger(mut self, ledger: impl Into<String>) -> Self {
        self.ledger = Some(ledger.into());
        self
    }

    pub(super) fn without_ledger(mut self) -> Self {
        self.omit_ledger = true;
        self
    }

    pub(super) fn infeasible(mut self) -> Self {
        self.steps = 0;
        self
    }

    pub(super) fn with_plan_scripts(self, scripts: Vec<PlanScript>) -> Self {
        *self.plan_scripts.borrow_mut() = scripts.into();
        self
    }

    pub(super) fn with_exec_usages(self, usages: Vec<Usage>) -> Self {
        *self.exec_usages.borrow_mut() = usages.into();
        self
    }

    fn plan_body(&self, number: u64) -> String {
        let extra = self
            .extra_by_issue
            .iter()
            .find(|(n, _)| *n == number)
            .map(|(_, e)| e.as_str())
            .or(self.extra.as_deref());
        let extra_section = extra.map_or_default(|e| format!("\n{e}\n"));
        let ledger_section = self
            .ledger
            .as_deref()
            .map_or_default(|l| format!("\n## Acceptance ledger\n\n{l}"));
        let step_line = if self.lint_dirty {
            "- [ ] do a thing\n"
        } else {
            "- [x] do a thing\n"
        };
        let mut body = format!(
            "# Plan for #{number}\n\n## Steps\n{}{ledger_section}{extra_section}",
            step_line.repeat(self.steps)
        );
        if !self.lint_dirty {
            self.append_closing_sections(&mut body);
        }
        body
    }

    /// The sections a lint-clean plan carries; one the plan already has stays as it is.
    fn append_closing_sections(&self, body: &mut String) {
        if !self.omit_ledger && !body.contains("## Acceptance ledger") {
            body.push_str(
                "\n## Acceptance ledger\n\n- [verified] scripted AC \u{2014} evidence: \
                scripted run\n",
            );
        }
        if !body.contains("## Handoff") {
            body.push_str("\n## Handoff\n\n- **Delivered**: scripted work\n");
        }
        if !body.contains("## Plan friction") {
            body.push_str("\n## Plan friction\n\n- none\n");
        }
    }

    fn repair_protocol(&self, ws: &Workspace) -> Result<()> {
        if self.fix_protocol && ws.ralphy_dir().join("protocol-failure.md").exists() {
            let plan_md = fs::read_to_string(ws.plan_path())?;
            let fixed = plan_md.replace("- [ ]", "- [x]")
                + "\n## Acceptance ledger\n\n- [verified] scripted AC \u{2014} evidence: scripted \
                run\n\n\
                   ## Handoff\n\n- **Delivered**: repaired\n\n## Plan friction\n\n- none\n";
            fs::write(ws.plan_path(), fixed)?;
        }
        Ok(())
    }
}

impl Agent for ScriptedAgent {
    fn name(&self) -> &'static str {
        "scripted"
    }

    fn plan(&self, issue: &Issue, ws: &Workspace) -> Result<Plan> {
        self.plan_attempts.set(self.plan_attempts.get() + 1);
        if let Some(PlanScript::Limit(reset)) = self.plan_scripts.borrow_mut().pop_front() {
            return Err(PlanLimit { reset }.into());
        }
        self.planned.borrow_mut().push(issue.number);
        fs::create_dir_all(ws.ralphy_dir())?;
        let path = ws.plan_path();
        fs::write(&path, self.plan_body(issue.number))?;
        Ok(Plan {
            open_steps: if self.already_executed { 0 } else { self.steps },
            recommended_model: None,
            path,
            usage: Usage::default(),
            session_id: None,
        })
    }

    fn execute(&self, _plan: &Plan, ws: &Workspace) -> Result<Execution> {
        let number = *self
            .planned
            .borrow()
            .last()
            .expect("the runner plans an issue before it executes it");
        self.repair_protocol(ws)?;
        let n = self.executed.borrow().len();
        self.executed.borrow_mut().push(number);
        let (outcome, commits) = self
            .outcomes
            .borrow_mut()
            .pop_front()
            .unwrap_or((Outcome::Done, true));
        if commits {
            self.head.set(self.head.get() + 1);
        }
        fs::write(
            ws.repo_root().join(format!("issue-{number}-{n}.txt")),
            "done\n",
        )?;
        Ok(Execution {
            outcome,
            usage: self
                .exec_usages
                .borrow_mut()
                .pop_front()
                .unwrap_or_default(),
            session_id: self.sessions.borrow_mut().pop_front().flatten(),
        })
    }
}

/// Records every close, comment, label and evidence write. An issue reads as open
/// unless it is in `closed_issues`; the other maps script what the tracker returns.
#[derive(Default)]
pub(super) struct RecordingTracker {
    pub(super) closes: RefCell<Vec<(u64, String)>>,
    pub(super) comments: RefCell<Vec<(u64, String)>>,
    pub(super) labels: RefCell<Vec<(u64, String)>>,
    pub(super) evidence_writes: RefCell<Vec<(u64, Vec<Verdict>)>>,
    pub(super) closed_issues: HashSet<u64>,
    /// Handoff comments by issue number.
    pub(super) handoffs: HashMap<u64, String>,
    /// Comment threads by issue number.
    pub(super) comment_threads: HashMap<u64, Vec<String>>,
    /// Labels by issue number.
    pub(super) issue_labels: HashMap<u64, Vec<String>>,
}

impl IssueTracker for RecordingTracker {
    fn close(&self, number: u64, comment: &str) -> Result<()> {
        self.closes.borrow_mut().push((number, comment.to_owned()));
        Ok(())
    }

    fn write_evidence(&self, number: u64, _body: &str, verdicts: &[Verdict]) -> Result<()> {
        self.evidence_writes
            .borrow_mut()
            .push((number, verdicts.to_vec()));
        Ok(())
    }

    fn is_closed(&self, number: u64) -> Result<bool> {
        Ok(self.closed_issues.contains(&number))
    }

    fn handoff_comment(&self, number: u64) -> Result<Option<String>> {
        Ok(self.handoffs.get(&number).cloned())
    }

    fn issue_comments(&self, number: u64) -> Result<Vec<String>> {
        Ok(self
            .comment_threads
            .get(&number)
            .cloned()
            .unwrap_or_default())
    }

    fn issue_labels(&self, number: u64) -> Result<Vec<String>> {
        Ok(self.issue_labels.get(&number).cloned().unwrap_or_default())
    }

    fn create_issue(&self, _title: &str, _body: &str, _labels: &[String]) -> Result<u64> {
        Ok(0)
    }

    fn comment(&self, number: u64, body: &str) -> Result<()> {
        self.comments.borrow_mut().push((number, body.to_owned()));
        Ok(())
    }

    fn add_label(&self, number: u64, label: &str) -> Result<()> {
        self.labels.borrow_mut().push((number, label.to_owned()));
        Ok(())
    }
}

/// Never expires; records every reset it is asked to wait for and answers each
/// wait with `wait_result` without sleeping.
pub(super) struct RecordingClock {
    pub(super) waited_for: RefCell<Vec<String>>,
    wait_result: WaitOutcome,
}

impl RecordingClock {
    pub(super) fn never() -> Self {
        Self {
            waited_for: RefCell::new(Vec::new()),
            wait_result: WaitOutcome::Resumed,
        }
    }

    /// A clock whose every wait reports the deadline passed.
    pub(super) fn deadline_on_wait() -> Self {
        Self {
            wait_result: WaitOutcome::DeadlinePassed,
            ..Self::never()
        }
    }
}

impl RunClock for RecordingClock {
    fn deadline_passed(&self) -> bool {
        false
    }

    fn wait_for_reset(&self, reset: &ResetAt) -> WaitOutcome {
        self.waited_for.borrow_mut().push(reset.text.clone());
        self.wait_result
    }
}
