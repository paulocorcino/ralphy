//! Token usage of a run: the sum of the per-call records of the stream
//! (ADR-0086 D5).
#![cfg_attr(
    not(test),
    expect(dead_code, reason = "used once the Agent impl lands")
)]

use ralphy_core::Usage;
use serde_json::Value;

use crate::outcome::Fold;

/// The `step_type` of a model call.
const AGENT_RESPONSE: &str = "agent_response";

/// The `state` of a step that finished.
const DONE: &str = "DONE";

/// The usage of a run: the sum of every finished model call in the stream.
///
/// `result.usage` is never read: on a resumed conversation it holds the whole
/// conversation, not this run. `output_tokens` already includes thinking, and
/// `agy` keeps no cache-creation count, so that field stays 0. `model` is the
/// `--model` id of the call; with none passed it stays `None` and is recovered
/// at read time from the store.
pub(crate) fn usage(fold: &Fold, model: Option<&str>) -> Usage {
    let mut total = Usage {
        model: model.map(str::to_owned),
        ..Usage::default()
    };
    for call in fold.steps.iter().filter_map(finished_call) {
        total.input += count(call, "input_tokens");
        total.cache_read += count(call, "cache_read_tokens");
        total.output += count(call, "output_tokens");
    }
    total
}

/// The `usage` object of a step that is a finished model call.
fn finished_call(step: &Value) -> Option<&Value> {
    let is_call = step.get("step_type").and_then(Value::as_str) == Some(AGENT_RESPONSE)
        && step.get("state").and_then(Value::as_str) == Some(DONE);
    is_call.then(|| step.get("usage")).flatten()
}

fn count(usage: &Value, field: &str) -> u64 {
    usage.get(field).and_then(Value::as_u64).unwrap_or(0)
}

#[cfg(test)]
mod tests;
