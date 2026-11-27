use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;

use ralphy_core::{
    Agent, Execution, HooksUnproven, Issue, IssueTracker, Outcome, Plan, ResetAt, RunClock, Usage,
    Verdict, WaitOutcome, Workspace,
};

use super::repo::git;

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
    /// Open steps written by `plan` — set to 0 to script an infeasible issue.
    pub(crate) steps: usize,
    /// If set, appended verbatim to the plan body (extra sections such as
    /// `## Feasible`, `## Handoff`, `## Plan friction`).
    pub(crate) extra: Option<String>,
    /// Per-issue plan extras that override `extra` for the matching issue number.
    /// Lets one queue mix, say, a failing and a passing `## Verify` gate.
    pub(crate) extra_by_issue: Vec<(u64, String)>,
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
            steps: 1,
            extra: None,
            extra_by_issue: Vec::new(),
            refusal: None,
        }
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

    /// Script a refusal: `plan` or `execute` fails with `HooksUnproven`.
    pub(crate) fn refusing(mut self, refusal: Refusal) -> Self {
        self.refusal = Some(refusal);
        self
    }
}

impl Agent for ScriptedAgent {
    fn name(&self) -> &'static str {
        "scripted"
    }

    fn plan(&self, issue: &Issue, ws: &Workspace) -> anyhow::Result<Plan> {
        if let Some(Refusal::Plan(cause)) = &self.refusal {
            return Err(HooksUnproven {
                cause: cause.clone(),
            }
            .into());
        }
        self.planned.borrow_mut().push(issue.number);
        fs::create_dir_all(ws.ralphy_dir())?;
        let path = ws.plan_path();
        let extra = self
            .extra_by_issue
            .iter()
            .find(|(n, _)| *n == issue.number)
            .map(|(_, e)| e.as_str())
            .or(self.extra.as_deref());
        let extra_section = extra.map_or_default(|e| format!("\n{e}\n"));
        // Lint-clean (ADR-0015): steps ticked and the closing sections present, so
        // tests not about the protocol lint never trip it.
        let mut body = format!(
            "# Plan for #{}\n\n## Steps\n{}{}",
            issue.number,
            "- [x] do a thing\n".repeat(self.steps),
            extra_section,
        );
        for (heading, section) in [
            (
                "## Acceptance ledger",
                "\n## Acceptance ledger\n\n- [verified] scripted AC \u{2014} evidence: \
                scripted run\n",
            ),
            (
                "## Handoff",
                "\n## Handoff\n\n- **Delivered**: scripted work\n",
            ),
            ("## Plan friction", "\n## Plan friction\n\n- none\n"),
        ] {
            if !body.contains(heading) {
                body.push_str(section);
            }
        }
        fs::write(&path, body)?;
        Ok(Plan {
            open_steps: self.steps,
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
            usage: Usage::default(),
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
        self.closes.borrow_mut().push((number, comment.to_owned()));
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
        self.comments.borrow_mut().push((number, body.to_owned()));
        Ok(())
    }

    fn add_label(&self, number: u64, label: &str) -> anyhow::Result<()> {
        self.labels.borrow_mut().push((number, label.to_owned()));
        Ok(())
    }

    fn remove_label(&self, number: u64, label: &str) -> anyhow::Result<()> {
        self.removed_labels
            .borrow_mut()
            .push((number, label.to_owned()));
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
