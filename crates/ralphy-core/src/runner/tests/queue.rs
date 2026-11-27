//! Queue-loop tests that assert no git state, run on the fake repository with a
//! plain temp dir as the workspace. The tests that assert git stay in
//! `tests/queue/`, over a real repository.

use super::*;
use crate::ledger::record_line;

mod blockers;
mod capture;
mod close_artifacts;
mod exec_usage;
mod limits;
mod scripted;
mod selection;
mod session_ledger;
mod verify;

use scripted::{PlanScript, RecordingClock, RecordingTracker, ScriptedAgent};

/// Work `queue` on the fake repository and an in-memory ledger.
fn run_on_fake(
    cfg: &QueueConfig,
    queue: &[Issue],
    agent: &ScriptedAgent,
    tracker: &RecordingTracker,
) -> (QueueReport, FakeLedger) {
    run_on_fake_with(cfg, queue, agent, tracker, &FakeClock)
}

/// [`run_on_fake`] with the clock of the test; the repository reads the commits
/// the agent scripts.
fn run_on_fake_with(
    cfg: &QueueConfig,
    queue: &[Issue],
    agent: &ScriptedAgent,
    tracker: &RecordingTracker,
    clock: &dyn RunClock,
) -> (QueueReport, FakeLedger) {
    let sink = FakeLedger::default();
    let report = run_queue_with(
        cfg,
        queue,
        agent,
        tracker,
        clock,
        &FakeRepo::with_head(Rc::clone(&agent.head)),
        &sink,
    )
    .expect("the queue run succeeds");
    (report, sink)
}

/// A limit's reset with the vendor's `text` and no instant.
fn reset_text(text: &str) -> ResetAt {
    ResetAt {
        text: text.to_owned(),
        at: None,
    }
}

/// A test issue with `body`.
fn issue_with_body(number: u64, body: &str) -> Issue {
    Issue {
        body: body.to_owned(),
        ..test_issue(number)
    }
}

/// The ledger lines `sink` received, as the file ledger writes them.
fn ledger_lines(sink: &FakeLedger) -> Vec<String> {
    sink.records
        .borrow()
        .iter()
        .map(|rec| record_line(rec).expect("a ledger record serializes"))
        .collect()
}

/// The issue numbers `tracker` closed, in order.
fn closed(tracker: &RecordingTracker) -> Vec<u64> {
    tracker.closes.borrow().iter().map(|(n, _)| *n).collect()
}

/// A `## Verify` line that passes only once the first repair wrote its marker.
fn verify_pass_after_repair_line() -> &'static str {
    if cfg!(windows) {
        "cmd /c \"if exist issue-1-1.txt (exit 0) else (exit 3)\""
    } else {
        "sh -c \"test -f issue-1-1.txt\""
    }
}
