//! Queue-loop tests that assert no git state, run on the fake repository with a
//! plain temp dir as the workspace. The tests that assert git stay in
//! `tests/queue/`, over a real repository.

use super::*;
use crate::ledger::record_line;

mod scripted;
mod session_ledger;
mod verify;

use scripted::{RecordingTracker, ScriptedAgent};

/// Work `queue` on the fake repository and an in-memory ledger.
fn run_on_fake(
    cfg: &QueueConfig,
    queue: &[Issue],
    agent: &ScriptedAgent,
    tracker: &RecordingTracker,
) -> (QueueReport, FakeLedger) {
    let sink = FakeLedger::default();
    let report = run_queue_with(
        cfg,
        queue,
        agent,
        tracker,
        &FakeClock,
        &FakeRepo::new(),
        &sink,
    )
    .expect("the queue run succeeds");
    (report, sink)
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
