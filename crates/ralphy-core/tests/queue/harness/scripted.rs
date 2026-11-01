use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;

use ralphy_core::{
    Agent, Execution, HooksUnproven, Issue, IssueTracker, Outcome, Plan, PlanLimit, ResetAt,
    RunClock, Usage, Verdict, WaitOutcome, Workspace,
};

use super::repo::git;

/// A scripted planning result. `Limit(reset)` makes `plan()` return a
/// `PlanLimit` error (a usage limit before any plan was written); once the
/// queue drains, `plan()` succeeds normally.
pub(crate) enum PlanScript {
    Limit(Option<ResetAt>),
}

/// A limit's reset with the vendor's `text` and no instant. The scripted
/// clock records only the text, so no test here needs the instant.
pub(crate) fn reset_text(text: &str) -> Option<ResetAt> {
    Some(ResetAt {
        text: text.to_string(),
        at: None,
    })
}

/// Where a scripted agent refuses with `HooksUnproven`.
pub(crate) enum Refusal {
    Plan(String),
    Execute(String),
}

/// Plans a feasible step for every issue and returns a scripted sequence of
/// execution outcomes (one per `execute` call). It also records, in order, the
/// issue numbers it was asked to plan and to execute — so a test can assert that
/// issues past a stop were never touched. Each executed issue makes one real
/// commit, so the run branch is non-empty (and is handed back, not dropped).
pub(crate) struct ScriptedAgent {
    /// Each entry is `(outcome, commits)`: when `commits` is false the execute
    /// makes no commit, leaving `HEAD` unchanged (used to drive the
    /// progress-aware cap on limit-resumes).
    pub(crate) outcomes: RefCell<VecDeque<(Outcome, bool)>>,
    pub(crate) planned: RefCell<Vec<u64>>,
    pub(crate) executed: RefCell<Vec<u64>>,
    /// Scripted planning results consumed before a `plan` succeeds. Each entry
    /// makes one `plan` call fail with a `PlanLimit`; an empty queue plans normally.
    pub(crate) plan_scripts: RefCell<VecDeque<PlanScript>>,
    /// Every `plan` call (including the failing limit attempts), in order.
    pub(crate) plan_attempts: RefCell<usize>,
    /// Open steps written by `plan` — set to 0 to script an infeasible issue.
    pub(crate) steps: usize,
    /// If set, appended to the plan as a `## Acceptance ledger` section.
    pub(crate) ledger: Option<String>,
    /// When true, `plan` omits the default `## Acceptance ledger` section
    /// (ADR-0015) that a lint-clean plan otherwise carries — used to test the
    /// lint's ledger-presence check in isolation.
    pub(crate) omit_ledger: bool,
    /// If set, appended verbatim to the plan body (extra sections such as
    /// `## Feasible`, `## Handoff`, `## Plan friction`).
    pub(crate) extra: Option<String>,
    /// Per-issue plan extras that override `extra` for the matching issue number.
    /// Lets one queue mix, say, a failing and a passing `## Verify` gate.
    pub(crate) extra_by_issue: Vec<(u64, String)>,
    /// When true, `plan` writes a protocol-dirty plan (unticked steps, no
    /// `## Handoff` / `## Plan friction`) so the ADR-0015 lint fires. The
    /// default is a lint-clean plan, keeping the lint transparent to tests
    /// that are not about it.
    pub(crate) lint_dirty: bool,
    /// When true, an `execute` that finds `.ralphy/protocol-failure.md` (the
    /// ADR-0015 bounce brief) repairs the plan: ticks every step and appends
    /// the missing closing sections — a well-behaved executor.
    pub(crate) fix_protocol: bool,
    /// Per-attempt `Usage` to hand back from `execute`, one per call, in
    /// order; once exhausted, `Usage::default()` (no model). Lets a test
    /// script the resume loop's model-folding behavior.
    pub(crate) exec_usages: RefCell<VecDeque<Usage>>,
    /// When true, `plan` hands back a plan whose steps are all checked and
    /// reports zero open steps, as a resumed plan from a finished run does.
    pub(crate) already_executed: bool,
    /// A scripted `HooksUnproven` from `plan` or `execute`.
    pub(crate) refusal: Option<Refusal>,
}

impl ScriptedAgent {
    /// Script a sequence of outcomes where every execute makes a commit.
    pub(crate) fn new(outcomes: Vec<Outcome>) -> Self {
        Self::scripted(outcomes.into_iter().map(|o| (o, true)).collect())
    }

    /// Script a sequence of `(outcome, commits)` pairs, controlling whether each
    /// execute leaves a commit behind.
    pub(crate) fn scripted(outcomes: Vec<(Outcome, bool)>) -> Self {
        Self {
            outcomes: RefCell::new(outcomes.into()),
            planned: RefCell::new(Vec::new()),
            executed: RefCell::new(Vec::new()),
            plan_scripts: RefCell::new(VecDeque::new()),
            plan_attempts: RefCell::new(0),
            steps: 1,
            ledger: None,
            omit_ledger: false,
            extra: None,
            extra_by_issue: Vec::new(),
            lint_dirty: false,
            fix_protocol: false,
            exec_usages: RefCell::new(VecDeque::new()),
            already_executed: false,
            refusal: None,
        }
    }

    /// Write protocol-dirty plans (unticked steps, no closing sections) so the
    /// ADR-0015 lint fails on the first DONE.
    pub(crate) fn lint_dirty(mut self) -> Self {
        self.lint_dirty = true;
        self
    }

    /// Repair the plan when a bounce brief (`protocol-failure.md`) is present.
    pub(crate) fn with_protocol_fix(mut self) -> Self {
        self.fix_protocol = true;
        self
    }

    pub(crate) fn with_ledger(mut self, ledger: impl Into<String>) -> Self {
        self.ledger = Some(ledger.into());
        self
    }

    /// Omit the default `## Acceptance ledger` section from an otherwise
    /// lint-clean plan, so the ledger-presence check alone bounces it.
    pub(crate) fn without_ledger(mut self) -> Self {
        self.omit_ledger = true;
        self
    }

    /// Append extra sections verbatim to every plan this agent writes.
    pub(crate) fn with_plan_extra(mut self, extra: impl Into<String>) -> Self {
        self.extra = Some(extra.into());
        self
    }

    /// Append an extra section to ONLY the plan for `number`, overriding
    /// `with_plan_extra` for that issue. Lets a single queue carry both a failing
    /// and a passing verify gate.
    pub(crate) fn with_plan_extra_for(mut self, number: u64, extra: impl Into<String>) -> Self {
        self.extra_by_issue.push((number, extra.into()));
        self
    }

    /// Script an infeasible plan (zero open steps).
    pub(crate) fn infeasible(mut self) -> Self {
        self.steps = 0;
        self
    }

    /// Script a resumed plan that an earlier run already executed: every step
    /// checked, zero open steps.
    pub(crate) fn already_executed(mut self) -> Self {
        self.already_executed = true;
        self
    }

    /// Script planning failures: each [`PlanScript`] makes one `plan` call return
    /// a `PlanLimit`; once exhausted, `plan` succeeds.
    pub(crate) fn with_plan_scripts(self, scripts: Vec<PlanScript>) -> Self {
        *self.plan_scripts.borrow_mut() = scripts.into();
        self
    }

    /// Script a refusal: `plan` or `execute` fails with `HooksUnproven`.
    pub(crate) fn refusing(mut self, refusal: Refusal) -> Self {
        self.refusal = Some(refusal);
        self
    }

    /// Script the `Usage` (including `model`) each `execute` call hands back,
    /// one per call in order.
    pub(crate) fn with_exec_usages(self, usages: Vec<Usage>) -> Self {
        *self.exec_usages.borrow_mut() = usages.into();
        self
    }
}

impl Agent for ScriptedAgent {
    fn name(&self) -> &'static str {
        "scripted"
    }

    fn plan(&self, issue: &Issue, ws: &Workspace) -> anyhow::Result<Plan> {
        *self.plan_attempts.borrow_mut() += 1;
        if let Some(Refusal::Plan(cause)) = &self.refusal {
            return Err(HooksUnproven {
                cause: cause.clone(),
            }
            .into());
        }
        // A scripted limit fails this plan call before any artifact is written,
        // mirroring a usage limit hit mid-planning.
        if let Some(PlanScript::Limit(reset)) = self.plan_scripts.borrow_mut().pop_front() {
            return Err(PlanLimit { reset }.into());
        }
        self.planned.borrow_mut().push(issue.number);
        fs::create_dir_all(ws.ralphy_dir())?;
        let path = ws.plan_path();
        let ledger_section = self
            .ledger
            .as_deref()
            .map(|l| format!("\n## Acceptance ledger\n\n{l}"))
            .unwrap_or_default();
        let extra = self
            .extra_by_issue
            .iter()
            .find(|(n, _)| *n == issue.number)
            .map(|(_, e)| e.as_str())
            .or(self.extra.as_deref());
        let extra_section = extra.map(|e| format!("\n{e}\n")).unwrap_or_default();
        // Lint-clean by default (ADR-0015): steps ticked and the closing
        // sections present, so tests not about the protocol lint never trip it.
        // `lint_dirty` scripts the opposite; an extra already carrying one of
        // the closing headings keeps its own version.
        let step_line = if self.lint_dirty {
            "- [ ] do a thing\n"
        } else {
            "- [x] do a thing\n"
        };
        let mut body = format!(
            "# Plan for #{}\n\n## Steps\n{}{}{}",
            issue.number,
            step_line.repeat(self.steps),
            ledger_section,
            extra_section,
        );
        if !self.lint_dirty {
            if !self.omit_ledger && !body.contains("## Acceptance ledger") {
                body.push_str(
                    "\n## Acceptance ledger\n\n- [verified] scripted AC \u{2014} evidence: scripted run\n",
                );
            }
            if !body.contains("## Handoff") {
                body.push_str("\n## Handoff\n\n- **Delivered**: scripted work\n");
            }
            if !body.contains("## Plan friction") {
                body.push_str("\n## Plan friction\n\n- none\n");
            }
        }
        fs::write(&path, body)?;
        Ok(Plan {
            open_steps: if self.already_executed { 0 } else { self.steps },
            recommended_model: None,
            path,
            usage: Usage::default(),
            session_id: None,
        })
    }

    fn execute(&self, _plan: &Plan, ws: &Workspace) -> anyhow::Result<Execution> {
        if let Some(Refusal::Execute(cause)) = &self.refusal {
            return Err(HooksUnproven {
                cause: cause.clone(),
            }
            .into());
        }
        // The most recently planned issue is the one being executed.
        let number = *self
            .planned
            .borrow()
            .last()
            .expect("no plan before execute");
        // A well-behaved executor's reaction to the ADR-0015 bounce brief:
        // tick every step and append the missing closing sections.
        if self.fix_protocol && ws.ralphy_dir().join("protocol-failure.md").exists() {
            let plan_md = fs::read_to_string(ws.plan_path())?;
            let fixed = plan_md.replace("- [ ]", "- [x]")
                + "\n## Acceptance ledger\n\n- [verified] scripted AC \u{2014} evidence: scripted run\n\n\
                   ## Handoff\n\n- **Delivered**: repaired\n\n## Plan friction\n\n- none\n";
            fs::write(ws.plan_path(), fixed)?;
        }
        let n = self.executed.borrow().len();
        self.executed.borrow_mut().push(number);
        let (outcome, commits) = self
            .outcomes
            .borrow_mut()
            .pop_front()
            .unwrap_or((Outcome::Done, true));
        // Make a real commit so the run branch carries work and is handed back —
        // unless this call is scripted to make no progress (HEAD unchanged).
        if commits {
            let repo = ws.repo_root();
            // A unique path per execute so a resumed retry still produces a new
            // commit when scripted to.
            let marker = repo.join(format!("issue-{number}-{n}.txt"));
            fs::write(&marker, "done\n").expect("write the scripted marker file");
            git(repo, &["add", "."]);
            git(
                repo,
                &["commit", "-q", "-m", &format!("work #{number} ({n})")],
            );
        }
        Ok(Execution {
            outcome,
            usage: self
                .exec_usages
                .borrow_mut()
                .pop_front()
                .unwrap_or_default(),
            session_id: None,
        })
    }
}

/// Records every `(number, comment)` close and every `write_evidence` call so
/// tests can assert exactly which issues were closed and what evidence was
/// written. Never mutates labels — there is no label API here, which is the point.
///
/// `closed_issues` is a set of issue numbers to report as closed when `is_closed`
/// is called — numbers absent from the set are reported as open.
#[derive(Default)]
pub(crate) struct RecordingTracker {
    pub(crate) closes: RefCell<Vec<(u64, String)>>,
    pub(crate) evidence_writes: RefCell<Vec<(u64, Vec<Verdict>)>>,
    pub(crate) closed_issues: HashSet<u64>,
    /// Every `comment` call (handoff at close, infeasible-skip reasoning).
    pub(crate) comments: RefCell<Vec<(u64, String)>>,
    /// Every `add_label` call (`needs-split` on a bundle verdict,
    /// `needs-human-review` on a close carrying review-only ledger lines).
    pub(crate) labels: RefCell<Vec<(u64, String)>>,
    /// Every `remove_label` call (the label swaps `ralphy triage` performs).
    pub(crate) removed_labels: RefCell<Vec<(u64, String)>>,
    /// Scripted handoff comments returned by `handoff_comment`, by issue number.
    pub(crate) handoffs: HashMap<u64, String>,
    /// Scripted comment threads returned by `issue_comments`, by issue number —
    /// the discussion the runner attaches to an issue before planning.
    pub(crate) comment_threads: HashMap<u64, Vec<String>>,
    /// Scripted open children returned by `open_children`, by parent number —
    /// the open issues whose `## Parent` references a retired bundle.
    pub(crate) children: HashMap<u64, Vec<u64>>,
    /// Scripted labels returned by `issue_labels`, by issue number — lets a test
    /// mark an open blocker as a human gate (`ready-for-human`/`HITL`, ADR-0014).
    pub(crate) issue_labels: HashMap<u64, Vec<String>>,
}

impl IssueTracker for RecordingTracker {
    fn close(&self, number: u64, comment: &str) -> anyhow::Result<()> {
        self.closes.borrow_mut().push((number, comment.to_string()));
        Ok(())
    }

    fn write_evidence(&self, number: u64, _body: &str, verdicts: &[Verdict]) -> anyhow::Result<()> {
        self.evidence_writes
            .borrow_mut()
            .push((number, verdicts.to_vec()));
        Ok(())
    }

    fn is_closed(&self, number: u64) -> anyhow::Result<bool> {
        Ok(self.closed_issues.contains(&number))
    }

    fn create_issue(&self, _title: &str, _body: &str, _labels: &[String]) -> anyhow::Result<u64> {
        Ok(0)
    }

    fn comment(&self, number: u64, body: &str) -> anyhow::Result<()> {
        self.comments.borrow_mut().push((number, body.to_string()));
        Ok(())
    }

    fn add_label(&self, number: u64, label: &str) -> anyhow::Result<()> {
        self.labels.borrow_mut().push((number, label.to_string()));
        Ok(())
    }

    fn remove_label(&self, number: u64, label: &str) -> anyhow::Result<()> {
        self.removed_labels
            .borrow_mut()
            .push((number, label.to_string()));
        Ok(())
    }

    fn handoff_comment(&self, number: u64) -> anyhow::Result<Option<String>> {
        Ok(self.handoffs.get(&number).cloned())
    }

    fn issue_comments(&self, number: u64) -> anyhow::Result<Vec<String>> {
        Ok(self
            .comment_threads
            .get(&number)
            .cloned()
            .unwrap_or_default())
    }

    fn open_children(&self, number: u64) -> anyhow::Result<Vec<u64>> {
        Ok(self.children.get(&number).cloned().unwrap_or_default())
    }

    fn issue_labels(&self, number: u64) -> anyhow::Result<Vec<String>> {
        Ok(self.issue_labels.get(&number).cloned().unwrap_or_default())
    }
}

/// A clock that reports the deadline passed once it has been polled `after`
/// times — letting a test fast-forward the budget deterministically. It also
/// records every `wait_for_reset` call (the reset string) and returns a scripted
/// [`WaitOutcome`] (default [`WaitOutcome::Resumed`]) without sleeping.
pub(crate) struct ScriptedClock {
    pub(crate) polls: RefCell<usize>,
    pub(crate) after: usize,
    /// The reset string passed to each `wait_for_reset`, in call order.
    pub(crate) waited_for: RefCell<Vec<String>>,
    /// What each `wait_for_reset` returns (defaults to `Resumed`).
    pub(crate) wait_result: WaitOutcome,
}

impl ScriptedClock {
    /// Never expires.
    pub(crate) fn never() -> Self {
        Self {
            polls: RefCell::new(0),
            after: usize::MAX,
            waited_for: RefCell::new(Vec::new()),
            wait_result: WaitOutcome::Resumed,
        }
    }

    /// Reports the deadline passed starting from the `after`-th poll (0-based).
    pub(crate) fn passes_after(after: usize) -> Self {
        Self {
            polls: RefCell::new(0),
            after,
            waited_for: RefCell::new(Vec::new()),
            wait_result: WaitOutcome::Resumed,
        }
    }

    /// A never-expiring clock whose `wait_for_reset` reports the deadline passed
    /// (the reset lands beyond the run deadline).
    pub(crate) fn deadline_on_wait() -> Self {
        Self {
            polls: RefCell::new(0),
            after: usize::MAX,
            waited_for: RefCell::new(Vec::new()),
            wait_result: WaitOutcome::DeadlinePassed,
        }
    }
}

impl RunClock for ScriptedClock {
    fn deadline_passed(&self) -> bool {
        let mut p = self.polls.borrow_mut();
        let passed = *p >= self.after;
        *p += 1;
        passed
    }

    fn wait_for_reset(&self, reset: &ResetAt) -> WaitOutcome {
        self.waited_for.borrow_mut().push(reset.text.clone());
        self.wait_result
    }
}
