//! The failure a moved verb's child writes as one JSON line on stderr, read
//! into the reply; its error chain goes to the daemon log (ADR-0078 D11, D12).

use ralphy_wording::{Failure, FailureSet};

use crate::{dispatch, DaemonFailure};

/// The verbs whose CLI writes its failure as JSON.
const MOVED_VERBS: &[dispatch::Verb] = &[dispatch::Verb::ChangesCommit];

/// Whether `verb` replies with a `failure`.
pub(super) fn is_moved(verb: dispatch::Verb) -> bool {
    MOVED_VERBS.contains(&verb)
}

/// `argv` with the global flag that makes the CLI write its failure as JSON.
pub(super) fn json_argv(argv: Vec<String>) -> Vec<String> {
    let mut flagged = vec!["--failure-format".to_owned(), "json".to_owned()];
    flagged.extend(argv);
    flagged
}

/// The line the CLI writes: the failure and the chain of the error.
#[derive(serde::Deserialize)]
struct FailureLine {
    failure: Failure,
    chain: String,
}

/// The error reply of a moved verb whose child exited non-zero.
pub(super) fn failed_reply(output: &dispatch::CollectedOutput) -> serde_json::Value {
    let failure = read_failure(output);
    let message = match &failure.text.remedy {
        Some(remedy) => format!("{}. {remedy}", failure.text.cause),
        None => failure.text.cause.clone(),
    };
    serde_json::json!({ "status": "error", "message": message, "failure": failure })
}

/// The failure on the last stderr line, or `ralphy-failed` when there is none.
/// A `ralphy-failed` never carries its `cause` detail: that is the chain.
fn read_failure(output: &dispatch::CollectedOutput) -> Failure {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let last = stderr.lines().rev().find(|l| !l.trim().is_empty());
    let Some(FailureLine { mut failure, chain }) =
        last.and_then(|l| serde_json::from_str::<FailureLine>(l).ok())
    else {
        let merged = String::from_utf8_lossy(&output.merged()).into_owned();
        tracing::warn!(output = %merged.trim(), "a ralphy command failed with no failure line");
        return DaemonFailure::RalphyFailed.failure(&[]);
    };
    tracing::warn!(chain = %chain, code = %failure.code, "a ralphy command failed");
    if is_ralphy_failed(&failure) {
        failure.details.remove("cause");
    }
    failure
}

fn is_ralphy_failed(failure: &Failure) -> bool {
    failure.domain == DaemonFailure::DOMAIN && failure.code == DaemonFailure::RalphyFailed.code()
}
