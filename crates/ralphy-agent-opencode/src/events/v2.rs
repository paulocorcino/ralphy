//! The 2.x `error` event, read by `error.type`, `error.message` and `error.status`
//! (ADR-0094 D4b-D4f). The 1.x reader of `error.name` and `error.data` does not run here.

use serde_json::Value;

use super::{usage_limit_regex, OpencodeLimit};

/// The `error.type` values of a 2.x usage limit (ADR-0094 D4e).
const LIMIT_TYPES: [&str; 7] = [
    "provider.quota",
    "provider.free-limit",
    "provider.go-limit",
    "provider.budget-limit",
    "provider.model-budget-limit",
    "provider.chatgpt-limit",
    "provider.rate-limit",
];

/// What a 2.x `error` event means, from its `error.type`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ErrorKind {
    /// `provider.auth`: the provider refused the login.
    Auth,
    /// `provider.no-route`: no login for the provider, or a wrong model id.
    NoRoute,
    /// One of [`LIMIT_TYPES`].
    Limit,
    /// Any other type: only its message can still name a limit.
    Unknown,
}

impl ErrorKind {
    fn of(error_type: &str) -> Self {
        match error_type {
            "provider.auth" => Self::Auth,
            "provider.no-route" => Self::NoRoute,
            t if LIMIT_TYPES.contains(&t) => Self::Limit,
            _ => Self::Unknown,
        }
    }
}

/// One 2.x `error` event: its kind and its message.
struct StreamError {
    kind: ErrorKind,
    message: String,
}

/// Whether an event is a 2.x `error` event. A `retry` event carries an `error`
/// object too, and is not one (ADR-0094 D4c).
pub(super) fn is_error_event(val: &Value) -> bool {
    val.get("type").and_then(Value::as_str) == Some("error")
}

fn stream_error(val: &Value) -> Option<StreamError> {
    if !is_error_event(val) {
        return None;
    }
    let field = |name: &str| {
        val.get("error")
            .and_then(|e| e.get(name))
            .and_then(Value::as_str)
            .unwrap_or("")
    };
    Some(StreamError {
        kind: ErrorKind::of(field("type")),
        message: field("message").to_owned(),
    })
}

fn json_line(line: &str) -> Option<Value> {
    serde_json::from_str(line.trim()).ok()
}

/// The last `error` event of a stream: only it and the exit decide the outcome.
fn last_error(stdout: &str) -> Option<StreamError> {
    stdout
        .lines()
        .rev()
        .filter_map(json_line)
        .find_map(|val| stream_error(&val))
}

/// The usage limit of a 2.x stream. A limit type is a limit; the wording regex
/// reads only the message of an unknown type. `retryAfterMs` is not read: an
/// `OpenCode` reset is never scheduled on (ADR-0094 D4f).
pub(super) fn parse_limit(stdout: &str) -> Option<OpencodeLimit> {
    let error = last_error(stdout)?;
    let is_limit = match error.kind {
        ErrorKind::Limit => true,
        ErrorKind::Unknown => usage_limit_regex().is_match(&error.message),
        ErrorKind::Auth | ErrorKind::NoRoute => false,
    };
    is_limit.then_some(OpencodeLimit { reset: None })
}

/// Whether any line of `text` is a 2.x `provider.auth` error event.
pub(super) fn has_auth_error(text: &str) -> bool {
    text.lines()
        .filter_map(json_line)
        .filter_map(|val| stream_error(&val))
        .any(|error| error.kind == ErrorKind::Auth)
}

/// The message of the last error when it is `provider.no-route`.
pub(super) fn no_route_message(stdout: &str) -> Option<String> {
    last_error(stdout)
        .filter(|error| error.kind == ErrorKind::NoRoute)
        .map(|error| error.message)
}

/// Whether a line is a 2.x `retry` event: one retry of a failed provider call.
pub(super) fn is_retry_line(line: &str) -> bool {
    json_line(line).is_some_and(|val| val.get("type").and_then(Value::as_str) == Some("retry"))
}

#[cfg(test)]
pub(super) mod tests;
