//! The per-issue `data.issues[]` of `dev.ralphy.run.finished` (#96, #224).

use serde_json::{json, Value};

use super::{issue_title, skip_kind_name};
use crate::runstate::{IssueEntry, RunState};

/// The per-issue rollup. PREFER the run's own rollup: the `run.finished`
/// scalars are folded from it, and the console `RunState` this sink also sees
/// is a lossy mirror — a drop-oldest eviction on the event ring can starve it,
/// leaving `data.issues` contradicting the very counts beside it. One source,
/// or none. An emitter that carried no rollup (a legacy `run finished`) falls
/// back to the fold. `title` stays fold-sourced either way: `QueueReport` has
/// no titles, and it is display sugar, not a count.
pub(super) fn finished_issues(rollup: &Value, state: &RunState) -> Vec<Value> {
    match rollup {
        Value::Array(entries) => entries.iter().map(|e| rollup_entry(e, state)).collect(),
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) | Value::Object(_) => {
            state
                .issues
                .iter()
                .filter_map(|e| fold_entry(e, state))
                .collect()
        }
    }
}

/// One entry of the run's own rollup, with its title from the fold.
fn rollup_entry(e: &Value, state: &RunState) -> Value {
    let number = e.get("number").cloned().unwrap_or(Value::Null);
    let mut obj = serde_json::Map::new();
    obj.insert("number".to_string(), number.clone());
    obj.insert(
        "title".to_string(),
        json!(issue_title(state, number.as_u64().unwrap_or(0))),
    );
    if let Some(status) = e.get("status") {
        obj.insert("status".to_string(), status.clone());
    }
    if let Some(kind) = e.get("kind") {
        obj.insert("kind".to_string(), kind.clone());
    }
    if let Some(blocked_by) = e.get("blocked_by") {
        obj.insert("blocked_by".to_string(), blocked_by.clone());
    }
    // Only the run's own rollup can supply this: `RunState` never learns the
    // ledger verdicts, so the fold fallback carries no `review_only` (#313).
    if let Some(review_only) = e.get("review_only") {
        obj.insert("review_only".to_string(), review_only.clone());
    }
    Value::Object(obj)
}

/// One entry built from the fold, or `None` for an issue still in flight.
fn fold_entry(e: &IssueEntry, state: &RunState) -> Option<Value> {
    let status = e.status.status_wire()?;
    let mut obj = serde_json::Map::new();
    obj.insert("number".to_string(), json!(e.number));
    obj.insert("title".to_string(), json!(issue_title(state, e.number)));
    obj.insert("status".to_string(), json!(status));
    if status == "skipped" {
        if let Some(kind) = e.kind {
            obj.insert("kind".to_string(), json!(skip_kind_name(kind)));
        }
        obj.insert("blocked_by".to_string(), json!(e.blocked_by));
    }
    Some(Value::Object(obj))
}
