//! The execute ledger line carries the last non-empty vendor session of the
//! resume loop.

use std::cell::RefCell;
use std::collections::VecDeque;

use ralphy_core::{Agent, Execution, Plan, Workspace};

use super::*;

/// A scripted agent whose `execute` reports the next session id of a script,
/// whatever outcome the inner agent scripted.
struct SessionedAgent {
    inner: ScriptedAgent,
    sessions: RefCell<VecDeque<Option<String>>>,
}

impl Agent for SessionedAgent {
    fn name(&self) -> &'static str {
        self.inner.name()
    }

    fn plan(&self, issue: &Issue, ws: &Workspace) -> anyhow::Result<Plan> {
        self.inner.plan(issue, ws)
    }

    fn execute(&self, plan: &Plan, ws: &Workspace) -> anyhow::Result<Execution> {
        let mut execution = self.inner.execute(plan, ws)?;
        execution.session_id = self.sessions.borrow_mut().pop_front().flatten();
        Ok(execution)
    }
}

#[test]
fn the_last_non_empty_session_of_the_resume_loop_reaches_the_ledger() {
    let repo = init_repo("session-ledger");
    let usage_dir = ensure_usage_dir();
    // The first attempt hits a limit and names its session; the resumed one
    // reports none.
    let agent = SessionedAgent {
        inner: ScriptedAgent::new(vec![Outcome::Limit(Some("09:00".into())), Outcome::Done]),
        sessions: RefCell::new(VecDeque::from([Some("sess-first".to_string()), None])),
    };

    run_queue(
        &cfg(&repo, "stamp-session-ledger", false),
        &[issue(78)],
        &agent,
        &RecordingTracker::default(),
        &ScriptedClock::never(),
    )
    .unwrap();

    let slug = ralphy_core::git::project_slug(&repo);
    let file = usage_dir.join(format!("{}.jsonl", slug.replace('/', "-")));
    let content = std::fs::read_to_string(&file)
        .unwrap_or_else(|e| panic!("ledger file {} unreadable: {e}", file.display()));
    let line = content
        .lines()
        .find(|l| l.contains("\"phase\":\"execute\""))
        .unwrap_or_else(|| panic!("no execute line: {content}"));
    assert!(
        line.contains("\"session_id\":\"sess-first\""),
        "the execute line keeps the earlier session: {line}"
    );

    fs::remove_dir_all(&repo).ok();
}
