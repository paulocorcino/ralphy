//! The end of an `agy` run read from its `stream-json` capture (ADR-0086 D3):
//! the fold of the stream, the completion signals the shared ladder orders
//! (ADR-0023), and the machine fault of D9.

use std::path::Path;

use ralphy_adapter_support::{blocked_reason, done_sentinel, CompletionSignals, HeadlessRun};
use serde::Deserialize;
use serde_json::Value;

/// The `result.status` of a turn that ended well.
const SUCCESS: &str = "SUCCESS";

/// The `finished_reason` of a turn cut by `--print-timeout`.
const PRINT_TIMEOUT: &str = "print_timeout";

/// The prefix of the one structured error line `agy` writes to stderr.
const AGY_ERROR: &str = "AGY_ERROR:";

/// What one capture of stdout reduces to.
#[derive(Debug, Default)]
pub(crate) struct Fold {
    /// `init.conversation_id`: the id a resume passes back (D18).
    pub(crate) conversation_id: Option<String>,
    /// The body of every `step_update` record, in order; usage is summed from
    /// them (D5).
    pub(crate) steps: Vec<Value>,
    /// The LAST `result` envelope: a session with a proof turn writes one per
    /// turn (D9). `None` when the capture has none, or the last one does not
    /// parse: the capture was cut.
    pub(crate) result: Option<TurnResult>,
}

/// The fields of a `result` envelope the outcome reads.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct TurnResult {
    pub(crate) status: String,
    pub(crate) response: String,
    pub(crate) error: Option<String>,
    pub(crate) finished_reason: Option<String>,
    pub(crate) denied_actions: Vec<Value>,
}

impl TurnResult {
    /// The envelope says the turn failed: a status other than success, or a
    /// tool `agy` denied on its own.
    fn failed(&self) -> bool {
        self.status != SUCCESS || !self.denied_actions.is_empty()
    }
}

/// How the child ended, as the runner saw it.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct RunEnd {
    /// The exit code; `None` when the child was killed.
    pub(crate) code: Option<i32>,
    /// Ralphy's own deadline or idle watch ended the child.
    pub(crate) timed_out: bool,
    /// A new commit landed during the call.
    pub(crate) committed: bool,
}

/// Fold a capture of stdout. A line of plain text is skipped. A result counts
/// only when the stream ends with it: a step after it starts the next turn,
/// and a record cut mid-JSON may be the final result. Either way the result is
/// unknown, and never the proof turn's.
pub(crate) fn fold(stdout: &str) -> Fold {
    let mut fold = Fold::default();
    for line in stdout.lines().map(str::trim).filter(|l| !l.is_empty()) {
        match serde_json::from_str::<Value>(line) {
            Ok(record) => fold.take(record),
            Err(_) if line.starts_with('{') => fold.result = None,
            Err(_) => {}
        }
    }
    fold
}

impl Fold {
    fn take(&mut self, mut record: Value) {
        match record.get("event").and_then(Value::as_str) {
            Some("init") => {
                self.conversation_id = record
                    .get("conversation_id")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
            }
            Some("step_update") => {
                self.result = None;
                if let Some(step) = record.get_mut("step_update") {
                    self.steps.push(step.take());
                }
            }
            Some("result") => {
                let body = record.get_mut("result").map(Value::take);
                self.result = body.and_then(|body| serde_json::from_value(body).ok());
            }
            _ => {}
        }
    }
}

/// The signals of one run (D3). `stderr` is the child's stderr alone: the
/// stream on stdout holds what the agent read, which may name a limit.
pub(crate) fn completion_signals(fold: &Fold, stderr: &str, end: RunEnd) -> CompletionSignals {
    let result = fold.result.as_ref();
    let response = result.map_or("", |r| r.response.as_str());
    let finished = result.and_then(|r| r.finished_reason.as_deref());
    let exited_ok =
        end.code == Some(0) && result.is_some_and(|r| r.status == SUCCESS) && finished.is_none();
    let errored = result.is_none_or(TurnResult::failed) || has_agy_error(stderr);
    CompletionSignals {
        done: last_line(response).is_some_and(done_sentinel),
        blocked: blocked_reason(response),
        // A free-text detector judges only a run that failed: the CLI retries a
        // per-minute 429 itself and may still end well.
        limit: ((errored || !exited_ok) && crate::auth::is_limit(stderr)).then_some(None),
        committed: end.committed,
        timed_out: end.timed_out || finished == Some(PRINT_TIMEOUT),
        exited_ok,
        errored,
    }
}

/// The child's stderr alone: the run's log is its stdout, then its stderr.
pub(crate) fn stderr_of(run: &HeadlessRun) -> &str {
    run.log
        .strip_prefix(run.stdout.as_str())
        .unwrap_or(&run.log)
}

/// The last line of `text` that is not blank.
fn last_line(text: &str) -> Option<&str> {
    text.lines().rev().find(|line| !line.trim().is_empty())
}

fn has_agy_error(stderr: &str) -> bool {
    stderr
        .lines()
        .any(|line| line.trim_start().starts_with(AGY_ERROR))
}

/// The fault of this machine, not of the issue: `agy` ends the whole run with
/// exit 3 and `invalid UTF-8` when a hook prints text that is not UTF-8 (`cmd`
/// in a non-English Windows, spike H1). `None` for any other end.
pub(crate) fn machine_fault(code: Option<i32>, stderr: &str, hooks_file: &Path) -> Option<String> {
    (code == Some(3) && stderr.contains("invalid UTF-8")).then(|| {
        format!(
            "agy stopped the run because a hook printed text that is not UTF-8 (exit 3). \
             This is a problem of this machine, not of the issue: check the hook commands \
             in {}",
            hooks_file.display()
        )
    })
}

#[cfg(test)]
mod tests;
