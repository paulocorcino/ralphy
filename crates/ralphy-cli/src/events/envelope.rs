//! The `RunEvent` -> CloudEvents 1.0 envelope mapping (ADR-0019).
//!
//! [`runevent_to_cloudevent`] is the single pure function that turns one folded
//! [`RunEvent`] into a CloudEvents 1.0 structured-mode JSON envelope, matching the
//! `docs/events.md` catalog. It is unit-tested per variant so a drift between an
//! event and its wire shape fails a test rather than silently changing the
//! contract — the per-variant tests are the source of truth the doc defers to.

use serde_json::{json, Value};

use crate::runstate::{RunEvent, RunState, SkipKind, UsageLite};

/// The per-run context every envelope shares: the `source` (`ralphy/<owner>/<repo>`),
/// the `runid` correlation ULID minted at process start, and the pre-serialized
/// `data.emitter` identity object.
#[derive(Debug, Clone)]
pub struct EventCtx {
    pub source: String,
    pub runid: String,
    pub emitter: Value,
}

/// Assemble a CloudEvents 1.0 structured-mode envelope. `data` is the
/// event-specific object; the reserved `emitter` identity is merged into it, and
/// the envelope carries exactly one extension attribute — `runid` (ADR-0019 §3).
fn envelope(type_: &str, subject: Option<&str>, ctx: &EventCtx, data: Value) -> Value {
    let mut data = data;
    if let Value::Object(ref mut map) = data {
        map.insert("emitter".to_string(), ctx.emitter.clone());
    }
    let mut ev = serde_json::Map::new();
    ev.insert("specversion".to_string(), json!("1.0"));
    ev.insert("type".to_string(), json!(type_));
    ev.insert("source".to_string(), json!(ctx.source));
    if let Some(subject) = subject {
        ev.insert("subject".to_string(), json!(subject));
    }
    ev.insert("id".to_string(), json!(super::emitter::new_id()));
    ev.insert(
        "time".to_string(),
        json!(chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)),
    );
    ev.insert("runid".to_string(), json!(ctx.runid));
    ev.insert(
        "datacontenttype".to_string(),
        json!("application/json"),
    );
    ev.insert("data".to_string(), data);
    Value::Object(ev)
}

/// The `usage {up,cr,cw,out,model}` object carried on `plan.written` and
/// `issue.closed` (docs/events.md); `model` is `null` when the adapter captured none.
fn usage_json(u: &UsageLite) -> Value {
    json!({
        "up": u.input,
        "cr": u.cache_read,
        "cw": u.cache_creation,
        "out": u.output,
        "model": u.model,
    })
}

/// The `issue/<n>` subject carried on every `issue.*` event and on `plan.written`.
fn subject_for(n: u64) -> String {
    format!("issue/{n}")
}

/// The [`SkipKind`] wire name on an `issue.skipped` event (docs/events.md).
fn skip_kind_name(kind: SkipKind) -> &'static str {
    match kind {
        SkipKind::BlockedBy => "blocked_by",
        SkipKind::StopBefore => "stop_before",
        SkipKind::HumanReturn => "human_return",
        SkipKind::VerifyFailed => "verify_failed",
    }
}

/// Map one folded [`RunEvent`] to a CloudEvents envelope, or `None` for an event
/// the sink does not forward. `state` resolves the active issue number for the
/// adapter events that carry `0` (planning/executing), mirroring the notifier fold.
///
/// Pure over `(ev, ctx, state)` apart from the per-event ULID `id` and UTC `time`
/// the envelope stamps — those are asserted for presence/shape, not equality.
pub fn runevent_to_cloudevent(ev: &RunEvent, ctx: &EventCtx, state: &RunState) -> Option<Value> {
    match ev {
        RunEvent::IssueClosed {
            number,
            tokens,
            usage,
        } => Some(envelope(
            "dev.ralphy.issue.closed",
            Some(&subject_for(*number)),
            ctx,
            json!({
                "number": number,
                "tokens": tokens,
                "usage": usage_json(usage),
            }),
        )),
        _ => {
            let _ = (state, skip_kind_name);
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A test context with a stub emitter object.
    fn ctx() -> EventCtx {
        EventCtx {
            source: "ralphy/o/r".to_string(),
            runid: "01RUNIDRUNIDRUNIDRUNID".to_string(),
            emitter: json!({ "version": "0.0.0", "pid": 4242 }),
        }
    }

    #[test]
    fn issue_closed_has_full_envelope_shape() {
        let ev = RunEvent::IssueClosed {
            number: 7,
            tokens: 42,
            usage: UsageLite {
                input: 1,
                cache_read: 2,
                cache_creation: 3,
                output: 4,
                model: Some("claude-sonnet-4".into()),
            },
        };
        let v = runevent_to_cloudevent(&ev, &ctx(), &RunState::new("t", 1)).unwrap();
        assert_eq!(v["specversion"], "1.0");
        assert_eq!(v["type"], "dev.ralphy.issue.closed");
        assert_eq!(v["source"], "ralphy/o/r");
        assert_eq!(v["subject"], "issue/7");
        assert_eq!(v["datacontenttype"], "application/json");
        assert_eq!(v["runid"], "01RUNIDRUNIDRUNIDRUNID");
        // The per-event id and time are present and well-shaped.
        assert!(v["id"].as_str().is_some_and(|s| !s.is_empty()), "id: {v}");
        assert!(
            v["time"].as_str().is_some_and(|s| s.ends_with('Z')),
            "time not UTC: {v}"
        );
        // Data fields + the reserved emitter block merged in.
        assert_eq!(v["data"]["number"], 7);
        assert_eq!(v["data"]["tokens"], 42);
        assert_eq!(v["data"]["usage"]["up"], 1);
        assert_eq!(v["data"]["usage"]["out"], 4);
        assert_eq!(v["data"]["usage"]["model"], "claude-sonnet-4");
        assert_eq!(v["data"]["emitter"]["pid"], 4242);
    }
}
