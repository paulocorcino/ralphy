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

/// Run a two-attempt resume loop whose attempts report `sessions`, and return the
/// execute line the ledger recorded.
fn execute_line_after_resume(stamp: &str, sessions: [Option<String>; 2]) -> String {
    let repo = init_repo("session-ledger");
    let usage_dir = ensure_usage_dir();
    let agent = SessionedAgent {
        inner: ScriptedAgent::new(vec![Outcome::Limit(Some("09:00".into())), Outcome::Done]),
        sessions: RefCell::new(VecDeque::from(sessions)),
    };

    run_queue(
        &cfg(&repo, stamp, false),
        &[issue(78)],
        &agent,
        &RecordingTracker::default(),
        &ScriptedClock::never(),
    )
    .expect("run_queue failed");

    let slug = ralphy_core::git::project_slug(&repo);
    let file = usage_dir.join(format!("{}.jsonl", slug.replace('/', "-")));
    let content = std::fs::read_to_string(&file)
        .unwrap_or_else(|e| panic!("ledger file {} unreadable: {e}", file.display()));
    let line = content
        .lines()
        .find(|l| l.contains("\"phase\":\"execute\""))
        .unwrap_or_else(|| panic!("no execute line: {content}"))
        .to_string();

    remove_scratch(&repo);
    line
}

#[test]
fn the_last_non_empty_session_of_the_resume_loop_reaches_the_ledger() {
    // The first attempt hits a limit and names its session; the resumed one
    // reports none.
    let line = execute_line_after_resume(
        "stamp-session-ledger",
        [Some("sess-first".to_string()), None],
    );
    assert!(
        line.contains("\"session_id\":\"sess-first\""),
        "the execute line keeps the earlier session: {line}"
    );
}

#[test]
fn an_empty_session_of_the_resume_loop_does_not_replace_the_earlier_one() {
    let line = execute_line_after_resume(
        "stamp-session-ledger-empty",
        [Some("sess-first".to_string()), Some(String::new())],
    );
    assert!(
        line.contains("\"session_id\":\"sess-first\""),
        "the execute line keeps the earlier session: {line}"
    );
}
