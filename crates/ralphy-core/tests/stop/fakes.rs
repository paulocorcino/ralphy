//! The scripted agent, tracker and clock of the stop tests.

use super::*;

/// Plans a feasible, lint-clean step and executes it green. `stop_at` names the
/// issue whose `execute` raises the stop flag from INSIDE the call — which is
/// what a real stop looks like: the setter is another thread, and the run
/// discovers it partway through its work.
pub(crate) struct StoppingAgent {
    pub(crate) planned: RefCell<Vec<u64>>,
    pub(crate) executed: RefCell<Vec<u64>>,
    pub(crate) stop_at: Option<u64>,
    /// A file the executor writes into the working tree and never commits — the
    /// witness for "a stop does not destroy uncommitted work".
    pub(crate) litter: Option<String>,
    /// The issue whose `plan` raises the stop from inside the call, and whether
    /// that planner is then reaped without a plan (`Err`) or finishes one.
    pub(crate) stop_in_plan: Option<(u64, PlannerEnd)>,
    /// When set, the first `execute` ends on a usage limit with a scheduled reset.
    pub(crate) limit_first: bool,
}

#[derive(Clone, Copy)]
pub(crate) enum PlannerEnd {
    Reaped,
    Finished,
}

impl StoppingAgent {
    pub(crate) fn new(stop_at: Option<u64>) -> Self {
        Self {
            planned: RefCell::new(Vec::new()),
            executed: RefCell::new(Vec::new()),
            stop_at,
            litter: None,
            stop_in_plan: None,
            limit_first: false,
        }
    }

    pub(crate) fn stopping_in_plan(mut self, number: u64, end: PlannerEnd) -> Self {
        self.stop_in_plan = Some((number, end));
        self
    }

    pub(crate) fn limiting_first(mut self) -> Self {
        self.limit_first = true;
        self
    }

    pub(crate) fn littering(mut self, name: &str) -> Self {
        self.litter = Some(name.to_owned());
        self
    }
}

impl Agent for StoppingAgent {
    fn name(&self) -> &'static str {
        "stopping"
    }

    fn plan(&self, issue: &Issue, ws: &Workspace) -> anyhow::Result<Plan> {
        self.planned.borrow_mut().push(issue.number);
        if let Some((number, end)) = self.stop_in_plan {
            if number == issue.number {
                ralphy_core::stop::request();
                if matches!(end, PlannerEnd::Reaped) {
                    anyhow::bail!("scripted planner produced no plan");
                }
            }
        }
        fs::create_dir_all(ws.ralphy_dir())?;
        let path = ws.plan_path();
        fs::write(
            &path,
            format!(
                "# Plan for #{}\n\n## Steps\n- [x] do a thing\n\
                 \n## Acceptance ledger\n\n- [verified] scripted AC \
                 \u{2014} evidence: scripted run\n\
                 \n## Handoff\n\n- **Delivered**: scripted work\n\
                 \n## Plan friction\n\n- none\n",
                issue.number
            ),
        )?;
        Ok(Plan {
            open_steps: 1,
            recommended_model: None,
            path,
            usage: Usage::default(),
            session_id: None,
        })
    }

    fn execute(&self, _plan: &Plan, ws: &Workspace) -> anyhow::Result<Execution> {
        let number = *self
            .planned
            .borrow()
            .last()
            .expect("no plan before execute");
        self.executed.borrow_mut().push(number);
        // One real commit per issue, so the run branch is non-empty and the
        // teardown has something to hand back.
        let file = ws.repo_root().join(format!("work-{number}.txt"));
        fs::write(&file, "work\n")?;
        git(ws.repo_root(), &["add", "."]);
        git(
            ws.repo_root(),
            &["commit", "-q", "-m", &format!("work #{number}")],
        );
        // …and, when asked, one file left DIRTY in the tree. Written after the
        // commit so nothing sweeps it up.
        if let Some(name) = &self.litter {
            fs::write(ws.repo_root().join(name), "uncommitted\n")?;
        }
        // The stop lands mid-issue, exactly as the delivery worker would raise it.
        if self.stop_at == Some(number) {
            ralphy_core::stop::request();
        }
        let outcome = if self.limit_first && self.executed.borrow().len() == 1 {
            Outcome::Limit(Some(ResetAt {
                text: "15:00".into(),
                at: None,
            }))
        } else {
            Outcome::Done
        };
        Ok(Execution {
            outcome,
            usage: Usage::default(),
            session_id: None,
        })
    }
}

pub(crate) struct SilentTracker;

impl IssueTracker for SilentTracker {
    fn close(&self, _number: u64, _comment: &str) -> anyhow::Result<()> {
        Ok(())
    }
    fn is_closed(&self, _number: u64) -> anyhow::Result<bool> {
        Ok(true)
    }
    fn create_issue(&self, _title: &str, _body: &str, _labels: &[String]) -> anyhow::Result<u64> {
        Ok(0)
    }
}

/// Never expires, never sleeps — so nothing in these tests can stop a run except
/// the flag under test.
pub(crate) struct FreeClock;

impl RunClock for FreeClock {
    fn deadline_passed(&self) -> bool {
        false
    }
    fn wait_for_reset(&self, _reset: &ResetAt) -> WaitOutcome {
        WaitOutcome::Resumed
    }
}
