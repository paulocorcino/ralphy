//! The stream-json channel of a headless session: the one user message that
//! carries the prompt on stdin, and the text a detector reads from the event
//! lines on stdout. The shapes are the ones measured in
//! `docs/research/claude-hook-probes-2026-10-28.md` (M5, M7).

use serde_json::Value;

/// The prompt framed as the one stream-json user message: a single JSON line,
/// ended by `\n`.
pub(crate) fn user_message(prompt: &str) -> String {
    let line = serde_json::json!({
        "type": "user",
        "message": { "role": "user", "content": prompt },
    });
    format!("{line}\n")
}

/// The text of a stream-json stdout, in order: the `text` items of each
/// `assistant` event and the `result` field of the `result` event, one per
/// line. Every other event (`system`, `user` tool results, `rate_limit_event`)
/// is dropped. A line that is not a JSON event is kept as it is, so stderr text
/// and a CLI error printed before the stream still reach a detector.
pub(crate) fn stream_text(stdout: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    for line in stdout.lines() {
        let trimmed = line.trim_start();
        let event = trimmed
            .starts_with('{')
            .then(|| serde_json::from_str::<Value>(trimmed).ok())
            .flatten();
        match event {
            Some(event) => out.extend(event_text(&event)),
            None => out.push(line.to_string()),
        }
    }
    out.join("\n")
}

/// The text one event carries: none for an event that is not `assistant` or
/// `result`.
fn event_text(event: &Value) -> Vec<String> {
    match event.get("type").and_then(Value::as_str) {
        Some("assistant") => event
            .pointer("/message/content")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|item| item.get("type").and_then(Value::as_str) == Some("text"))
            .filter_map(|item| item.get("text").and_then(Value::as_str))
            .map(str::to_string)
            .collect(),
        Some("result") => event
            .get("result")
            .and_then(Value::as_str)
            .map(str::to_string)
            .into_iter()
            .collect(),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::is_claude_auth_error;
    use crate::headless::{classify_exec_call, HeadlessReason};

    const DONE: &str = include_str!("../tests/fixtures/stream_json_done.jsonl");
    const BLOCKED: &str = include_str!("../tests/fixtures/stream_json_blocked.jsonl");
    const LOGGED_OUT: &str = include_str!("../tests/fixtures/stream_json_logged_out.jsonl");

    #[test]
    fn the_prompt_is_framed_as_one_stream_json_user_message() {
        let prompt = "say \"hi\" \\ then\nnext line é";
        let line = user_message(prompt);
        assert!(line.ends_with('\n'), "{line:?}");
        assert_eq!(line.matches('\n').count(), 1, "one line: {line:?}");
        let v: Value = serde_json::from_str(line.trim_end()).expect("the line is JSON");
        assert_eq!(v["type"], "user");
        assert_eq!(v["message"]["role"], "user");
        assert_eq!(v["message"]["content"], prompt);
    }

    #[test]
    fn the_exec_detectors_read_the_text_of_the_stream() {
        assert_eq!(
            classify_exec_call(&stream_text(DONE), true, 3),
            Some(HeadlessReason::Done)
        );
        assert_eq!(
            classify_exec_call(&stream_text(BLOCKED), true, 3),
            Some(HeadlessReason::Blocked("missing key".into()))
        );
    }

    #[test]
    fn the_stream_text_keeps_auth_and_drops_rate_limit_events() {
        let rate_limit = BLOCKED
            .lines()
            .find(|l| l.contains("\"rate_limit_event\""))
            .expect("the fixture carries a rate_limit_event line");
        assert_eq!(classify_exec_call(&stream_text(rate_limit), true, 3), None);
        // M7 measured an empty stderr for the logged-out case.
        let stderr = "";
        let log = format!("{LOGGED_OUT}{stderr}");
        assert!(is_claude_auth_error(&stream_text(&log)));
    }
}
