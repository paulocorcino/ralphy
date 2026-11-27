//! The scripted agent and the recording tracker of the queue tests that run on
//! the fake repository: the agent writes files but never runs git.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::fs;

use anyhow::Result;

use crate::{Agent, Execution, Issue, IssueTracker, Outcome, Plan, Usage, Workspace};

/// Plans a feasible step for every issue and returns a scripted sequence of
/// execution outcomes (one per `execute` call), recording in order the issue
/// numbers it planned and executed. Each execute writes a unique marker file in
/// the workspace, so a verify command can see the work of a repair.
#[derive(Default)]
pub(super) struct ScriptedAgent {
    outcomes: RefCell<VecDeque<Outcome>>,
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
    /// Script a sequence of outcomes; once exhausted, every execute is `Done`.
    pub(super) fn new(outcomes: Vec<Outcome>) -> Self {
        Self {
            outcomes: RefCell::new(outcomes.into()),
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

    fn plan_body(&self, number: u64) -> String {
        let extra = self
            .extra_by_issue
            .iter()
            .find(|(n, _)| *n == number)
            .map(|(_, e)| e.as_str())
            .or(self.extra.as_deref());
        let extra_section = extra.map_or_default(|e| format!("\n{e}\n"));
        let step_line = if self.lint_dirty {
            "- [ ] do a thing\n"
        } else {
            "- [x] do a thing\n"
        };
        let mut body = format!("# Plan for #{number}\n\n## Steps\n{step_line}{extra_section}");
        if !self.lint_dirty {
            body.push_str(
                "\n## Acceptance ledger\n\n- [verified] scripted AC \u{2014} evidence: \
                scripted run\n",
            );
            body.push_str("\n## Handoff\n\n- **Delivered**: scripted work\n");
            body.push_str("\n## Plan friction\n\n- none\n");
        }
        body
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
        self.planned.borrow_mut().push(issue.number);
        fs::create_dir_all(ws.ralphy_dir())?;
        let path = ws.plan_path();
        fs::write(&path, self.plan_body(issue.number))?;
        Ok(Plan {
            open_steps: usize::from(!self.already_executed),
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
        let outcome = self
            .outcomes
            .borrow_mut()
            .pop_front()
            .unwrap_or(Outcome::Done);
        fs::write(
            ws.repo_root().join(format!("issue-{number}-{n}.txt")),
            "done\n",
        )?;
        Ok(Execution {
            outcome,
            usage: Usage::default(),
            session_id: self.sessions.borrow_mut().pop_front().flatten(),
        })
    }
}

/// Records every close, comment and label. Every issue reads as open.
#[derive(Default)]
pub(super) struct RecordingTracker {
    pub(super) closes: RefCell<Vec<(u64, String)>>,
    pub(super) comments: RefCell<Vec<(u64, String)>>,
    pub(super) labels: RefCell<Vec<(u64, String)>>,
}

impl IssueTracker for RecordingTracker {
    fn close(&self, number: u64, comment: &str) -> Result<()> {
        self.closes.borrow_mut().push((number, comment.to_owned()));
        Ok(())
    }

    fn is_closed(&self, _number: u64) -> Result<bool> {
        Ok(false)
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
