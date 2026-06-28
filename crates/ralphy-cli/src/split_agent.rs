//! Composition-root wrapper that lets one run use different adapters for
//! planning and execution (`--plan-agent`, ADR-0009). It delegates `plan()` to
//! the planner and `execute()` to the executor, holding two `Box<dyn Agent>`.
//! The core's `Agent` trait and the `run_queue`/`run` signatures stay untouched —
//! the core still sees a single `Agent` and never learns it is split.

use anyhow::Result;
use ralphy_core::{Agent, Execution, Issue, Plan, Workspace};

/// Routes the two agent phases to two different adapters. Built only when the
/// planner and executor actually differ; a single-agent run uses its executor
/// box directly, so that path is byte-for-byte unchanged (no wrapper in the call
/// chain).
pub struct SplitAgent {
    pub planner: Box<dyn Agent>,
    pub executor: Box<dyn Agent>,
}

impl Agent for SplitAgent {
    /// The wrapper reports a single identity — the executor's. The runner stamps
    /// this on both ledger lines, so a split run's plan line carries the
    /// executor's name even though the planner produced those tokens (the
    /// deliberate zero-churn trade-off; see ADR-0009 Consequences).
    fn name(&self) -> &'static str {
        self.executor.name()
    }

    fn plan(&self, issue: &Issue, ws: &Workspace) -> Result<Plan> {
        self.planner.plan(issue, ws)
    }

    fn execute(&self, plan: &Plan, ws: &Workspace) -> Result<Execution> {
        self.executor.execute(plan, ws)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ralphy_core::{Outcome, Usage};
    use std::cell::Cell;
    use std::rc::Rc;

    /// Shared "which method ran" flags. The test keeps a clone so it can assert on
    /// what the boxed stub recorded without downcasting the trait object.
    #[derive(Clone, Default)]
    struct Calls {
        planned: Rc<Cell<bool>>,
        executed: Rc<Cell<bool>>,
    }

    /// A stub adapter that flips its shared flags when a phase is invoked.
    struct StubAgent {
        label: &'static str,
        calls: Calls,
    }

    impl Agent for StubAgent {
        fn name(&self) -> &'static str {
            self.label
        }

        fn plan(&self, _issue: &Issue, ws: &Workspace) -> Result<Plan> {
            self.calls.planned.set(true);
            Ok(Plan {
                open_steps: 1,
                recommended_model: None,
                path: ws.plan_path(),
                usage: Usage::default(),
            })
        }

        fn execute(&self, _plan: &Plan, _ws: &Workspace) -> Result<Execution> {
            self.calls.executed.set(true);
            Ok(Execution {
                outcome: Outcome::Done,
                usage: Usage::default(),
            })
        }
    }

    /// Build a split over two stubs, returning the wrapper plus each side's call
    /// flags for assertions.
    fn split() -> (SplitAgent, Calls, Calls) {
        let planner_calls = Calls::default();
        let executor_calls = Calls::default();
        let agent = SplitAgent {
            planner: Box::new(StubAgent {
                label: "P",
                calls: planner_calls.clone(),
            }),
            executor: Box::new(StubAgent {
                label: "X",
                calls: executor_calls.clone(),
            }),
        };
        (agent, planner_calls, executor_calls)
    }

    fn issue() -> Issue {
        Issue {
            number: 1,
            title: String::new(),
            body: String::new(),
            labels: vec![],
            comments: vec![],
        }
    }

    #[test]
    fn name_reports_the_executor() {
        let (agent, _, _) = split();
        assert_eq!(agent.name(), "X");
    }

    #[test]
    fn plan_routes_to_the_planner_only() {
        let (agent, planner, executor) = split();
        let ws = Workspace::new(std::env::temp_dir());

        agent.plan(&issue(), &ws).unwrap();

        assert!(planner.planned.get(), "planner.plan was invoked");
        assert!(!planner.executed.get(), "planner.execute was not invoked");
        assert!(!executor.planned.get(), "executor was untouched by plan()");
        assert!(!executor.executed.get(), "executor was untouched by plan()");
    }

    #[test]
    fn execute_routes_to_the_executor_only() {
        let (agent, planner, executor) = split();
        let ws = Workspace::new(std::env::temp_dir());
        let plan = Plan {
            open_steps: 1,
            recommended_model: None,
            path: ws.plan_path(),
            usage: Usage::default(),
        };

        agent.execute(&plan, &ws).unwrap();

        assert!(executor.executed.get(), "executor.execute was invoked");
        assert!(!executor.planned.get(), "executor.plan was not invoked");
        assert!(!planner.planned.get(), "planner was untouched by execute()");
        assert!(
            !planner.executed.get(),
            "planner was untouched by execute()"
        );
    }
}
