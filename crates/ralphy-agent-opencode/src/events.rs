//! Parsing OpenCode's `--format json` line-delimited event stream: extracting
//! assistant text, detecting error/limit events under the several observed
//! envelope shapes, and the auth-error detector (ADR-0005 D2/D6/D9).

use regex::Regex;

/// A model-agnostic matcher for the *class* of usage-limit signals a provider
/// message or log line can carry. opencode fronts many providers (Anthropic, Z.ai,
/// Kimi, OpenAI-compatible, …) whose quota wording differs, so keying on any one
/// vendor's exact phrase misses the next one — this matches the shared shape
/// instead (D9). Anchored on limit-specific combos (`usage limit`, `rate limit`,
/// `limit reached/exceeded/exhausted`, `too many requests`, `quota
/// exceeded/exhausted`), not a bare `"limit"`, so an ordinary error line (`limit not
/// found`, a transient backend blip) is not misread as a quota cap. `exhausted` sits
/// in the limit-verb group so Z.ai's weekly/monthly wording (`Limit Exhausted`, which
/// carries no `quota` prefix — FinCal #77, 2026-07-13) is caught. Compiled once per
/// scan (or once per early-kill hook build in the execute path —
/// [`OpenCodeAgent::execute`]).
pub(crate) fn usage_limit_regex() -> Regex {
    Regex::new(
        r"(?i)usage limit|rate[ _]?limit|limit (?:reached|exceeded|exhausted)|too many requests|quota (?:exceeded|exhausted)",
    )
    .expect("valid usage-limit regex")
}

/// The actionable message shown when `is_opencode_auth_error` fires — tells the
/// operator exactly what to do to recover (run `opencode auth login`).
pub(crate) const OPENCODE_AUTH_ERROR_MSG: &str =
    "OpenCode is not authenticated (ProviderAuthError) — run `opencode auth login` and retry";

/// Return `true` when `text` (the combined stdout+stderr log) shows an OpenCode
/// authentication failure. A signed-out `opencode run` emits a `ProviderAuthError`
/// SDK error event (ADR-0005 D6). Keying on the case-insensitive substring
/// `providerautherror` is specific enough to avoid false positives from our own
/// prompt text mentioning `opencode auth login`.
pub(crate) fn is_opencode_auth_error(text: &str) -> bool {
    ralphy_adapter_support::auth_error(text, &[&["providerautherror"]])
}

/// The headless **degraded-line** matcher, handed to the shared runner's
/// `degraded_line` seam. A degraded line is a *retryable* error event — a
/// transient backend blip the SDK retries internally (`{type:"error"}` under any
/// observed envelope) that is NOT a terminal usage limit or auth failure (those
/// have their own handling: the early-kill switch and the auth bail). Keeping the
/// terminal signals out of the degraded set is what stops a real quota block from
/// being papered over as "degraded, retrying"; a false negative here is safe (the
/// line just counts as ordinary progress).
pub(crate) fn is_opencode_api_degraded(line: &str) -> bool {
    let trimmed = line.trim();
    let Ok(val) = serde_json::from_str::<serde_json::Value>(trimmed) else {
        return false; // logfmt/plain lines are not degraded events
    };
    if !is_error_event(&val) {
        return false;
    }
    // A terminal limit or auth error is NOT "degraded" — those are terminal.
    !is_opencode_auth_error(trimmed) && parse_opencode_limit(trimmed).is_none()
}

/// The payload object a single event's fields live in. opencode 1.16.2 wraps
/// every event in an envelope `{type, timestamp, sessionID, part:{…}}` and puts
/// the real fields (`text`, `tool`, `reason`, …) under `part`; the older/SDK
/// shape this adapter was first written against is flat (the fields sit at the
/// top level). Returning `part` when present and the value itself otherwise lets
/// every parser read fields from one place and stay correct under both shapes
/// (ADR-0005 D2 — the exact event JSON, observed live against opencode 1.16.2).
fn event_payload(val: &serde_json::Value) -> &serde_json::Value {
    val.get("part").unwrap_or(val)
}

/// Whether an event (envelope `val`) is an error event under any observed shape:
/// the flat `type:"error"`, a `part:{type:"error"}`, or the opencode 1.16.2
/// envelope that carries a top-level `error` object (`{type:"error", error:{…}}`).
fn is_error_event(val: &serde_json::Value) -> bool {
    let top = val.get("type").and_then(|v| v.as_str()).unwrap_or("");
    let inner = event_payload(val)
        .get("type")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    top == "error" || inner == "error" || val.get("error").is_some()
}

/// The object an error event's `name`/`statusCode`/`message`/`retryAfter` fields
/// live in. opencode 1.16.2 nests them as `error.data` under a top-level `error`
/// object (`{type:"error", error:{name, data:{message, …}}}`, captured live); the
/// flat/SDK shape puts them directly on the payload. Returns the most specific
/// object available so the limit matcher and reset parser read fields from one
/// place under either shape (ADR-0005 D6/D9 — exact error JSON, observed live).
fn error_detail(val: &serde_json::Value) -> &serde_json::Value {
    if let Some(err) = val.get("error") {
        return err.get("data").unwrap_or(err);
    }
    event_payload(val)
}

/// The error event's `name`, reading `error.name` (opencode 1.16.2) before the
/// flat `name` on the payload.
fn error_name(val: &serde_json::Value) -> &str {
    val.get("error")
        .and_then(|e| e.get("name"))
        .or_else(|| event_payload(val).get("name"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
}

/// Parse OpenCode's `--format json` line-delimited event stream: concatenate the
/// assistant `text` parts into the returned string (the source the sentinel scan
/// reads) and set the bool when any line is an `error` event. Tolerant of
/// unparseable lines (skipped). Reads the text from the event's `part` payload
/// (opencode 1.16.2) and falls back to the top level (flat shape), so the
/// sentinel scan sees the assistant's real output under both envelopes.
pub(crate) fn parse_opencode_events(stdout: &str) -> (String, bool) {
    let mut text = String::new();
    let mut saw_error = false;
    for line in stdout.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(val) = serde_json::from_str::<serde_json::Value>(trimmed) else {
            continue; // tolerate non-JSON noise on the stream
        };
        if is_error_event(&val) {
            saw_error = true;
        }
        let payload = event_payload(&val);
        let is_text = val.get("type").and_then(|v| v.as_str()) == Some("text")
            || payload.get("type").and_then(|v| v.as_str()) == Some("text");
        if is_text {
            if let Some(t) = payload.get("text").and_then(|v| v.as_str()) {
                text.push_str(t);
                text.push('\n');
            }
        }
    }
    (text, saw_error)
}

/// Extract a reset hint from an OpenCode error event or message text (best-effort).
/// Looks for a `retryAfter` field value, or a `Retry-After` / "try again" substring
/// in the message. Returns `None` when absent (D9: reset hint is not guaranteed).
fn parse_opencode_reset_hint(event: &serde_json::Value) -> Option<String> {
    // retryAfter field on the event object.
    if let Some(v) = event.get("retryAfter") {
        let s = match v {
            serde_json::Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        if !s.is_empty() && s != "null" {
            return Some(s);
        }
    }
    // "try again" or "Retry-After" in the message text.
    if let Some(msg) = event.get("message").and_then(|v| v.as_str()) {
        let lower = msg.to_ascii_lowercase();
        // "retry-after: <value>"
        if let Some(pos) = lower.find("retry-after:") {
            let rest = msg[pos + "retry-after:".len()..].trim();
            let hint = rest
                .split_whitespace()
                .next()
                .unwrap_or("")
                .trim_end_matches(',');
            if !hint.is_empty() {
                return Some(hint.to_string());
            }
        }
        // "try again at <value>" or "try again in <value>"
        for prefix in &["try again at ", "try again in "] {
            if let Some(pos) = lower.find(prefix) {
                let rest = msg[pos + prefix.len()..].trim();
                let hint: String = rest
                    .chars()
                    .take_while(|c| *c != '.' && *c != '\n')
                    .collect();
                let hint = hint.trim().to_string();
                if !hint.is_empty() {
                    return Some(hint);
                }
            }
        }
    }
    None
}

/// Scan the line-delimited JSON event stream for a usage-limit signal (ADR-0005 D9).
///
/// Returns:
/// - `Some(Some(hint))` — a limit event was seen and carries a reset hint.
/// - `Some(None)` — a limit event was seen but no reset hint was found.
/// - `None` — no limit event was seen.
///
/// Detects three documented shapes:
/// 1. `name:"APIError"` + `statusCode:429` (the SDK's rate-limit error).
/// 2. Literal rate-limit strings from OpenCode's `retryable()` function
///    (`retry.ts`): "rate_limit_error", "rate limit exceeded", "too many requests",
///    "quota exceeded".
/// 3. Zen provider `*UsageLimitError` name suffix.
pub(crate) fn parse_opencode_limit(stdout: &str) -> Option<Option<String>> {
    // Shares the line-delimited-JSON scan scaffold with the Claude adapter's
    // transcript limit scan (`scan_json_lines`); the error-shape decoding and the
    // limit predicate below stay OpenCode-specific.
    ralphy_adapter_support::scan_json_lines(stdout, |val| {
        if !is_error_event(val) {
            return None;
        }
        // Read the error fields from wherever this shape carries them: `error.data`
        // (opencode 1.16.2), `error`, or the flat payload (`part`/top level).
        let name = error_name(val);
        let detail = error_detail(val);
        let status = detail
            .get("statusCode")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let msg = detail
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_ascii_lowercase();

        // A structural 429/`*UsageLimitError` is a limit outright; otherwise fall to
        // the model-agnostic message matcher. This keeps Kimi's billing-cycle 403
        // (statusCode 403, wording "usage limit for this billing cycle") a limit to
        // wait out, while an ordinary 403 (no model access, "forbidden: …") stays an
        // error — the matcher requires limit-specific wording, not a bare status.
        let is_limit = (name == "APIError" && status == 429)
            || name.ends_with("UsageLimitError")
            || usage_limit_regex().is_match(&msg);

        is_limit.then(|| parse_opencode_reset_hint(detail))
    })
}

/// Scan opencode's raw combined log (stdout+stderr) for a usage-limit signal in
/// the logfmt lines `--print-logs` prints to stderr — the path a JSON-event scan
/// ([`parse_opencode_limit`], which reads the `--format json` stream) structurally
/// cannot see. Some providers never surface a quota block as a `{type:"error"}`
/// JSON event: Z.ai's `zai-coding-plan` (GLM) treats `AI_APICallError: Usage limit
/// reached` as a retryable stream error and loops on backoff, logging it only here
/// (observed live 2026-07-11, FinCal #71, glm-5.2). Uses the same model-agnostic
/// [`usage_limit_regex`] as the JSON path so a new provider's wording is caught
/// without a per-model change. Same contract as [`parse_opencode_limit`]:
/// `Some(Some(hint))` when a limit is seen with a reset hint, `Some(None)` when
/// seen without one, `None` otherwise (ADR-0005 D9).
pub(crate) fn parse_opencode_log_limit(log: &str) -> Option<Option<String>> {
    let re = usage_limit_regex();
    log.lines()
        .find(|line| re.is_match(line))
        .map(parse_reset_hint_from_text)
}

/// Best-effort reset-time extraction from a raw log line (the logfmt path, where
/// the field lives inside a quoted `error.error="…"` value rather than a JSON
/// field). Anchored on a reset phrase (`reset[s]/reset at/reset in/try again
/// at|in`) so the datetime that follows it — and not the logfmt line's own leading
/// `timestamp=…` field, nor any other instant on the line — is what gets captured.
/// The captured shape is model-agnostic: an absolute ISO date-time with or without
/// a `T`/zone (`2026-07-11 22:14:08`, `…T…Z`, `…+02:00`) or a bare clock time
/// (`22:14`, `6:13 PM`); the downstream clock ([`RunClock::wait_for_reset`])
/// understands each. Returns `None` when no usable datetime follows a reset phrase
/// — the caller then treats the limit as `Limit(None)` and retries on the
/// synthetic-reset cadence rather than parking on an unusable hint.
fn parse_reset_hint_from_text(line: &str) -> Option<String> {
    let re = Regex::new(
        r"(?i)(?:reset(?:s)?(?:\s+(?:at|in))?|try\s+again\s+(?:at|in))\s*[:=]?\s*(\d{4}-\d{2}-\d{2}[ T]\d{2}:\d{2}(?::\d{2})?(?:z|[+-]\d{2}:?\d{2})?|\d{1,2}:\d{2}(?:\s*[ap]m)?)",
    )
    .expect("valid reset-hint regex");
    re.captures(line)
        .and_then(|caps| caps.get(1))
        .map(|m| m.as_str().trim().to_string())
        .filter(|hint| !hint.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── is_opencode_auth_error ──────────────────────────────────────────────

    #[test]
    fn is_opencode_auth_error_matches_captured_provider_auth_error() {
        // Representative captured log from a signed-out `opencode run`: the SDK
        // emits a `ProviderAuthError` event name in the JSON error event and may
        // also print it to stderr. Either occurrence triggers the detector.
        let json_event =
            r#"{"type":"error","name":"ProviderAuthError","message":"Not authenticated"}"#;
        assert!(
            is_opencode_auth_error(json_event),
            "must match a ProviderAuthError JSON event"
        );

        // Mixed log with stderr text (case-insensitive check).
        let mixed_log = "some init output\nError: ProviderAuthError: not signed in\n";
        assert!(
            is_opencode_auth_error(mixed_log),
            "must match ProviderAuthError in stderr text"
        );

        // Upper-cased variant — to_ascii_lowercase makes it case-insensitive.
        assert!(
            is_opencode_auth_error("PROVIDERAUTHERROR"),
            "must be case-insensitive"
        );
    }

    #[test]
    fn is_opencode_auth_error_ignores_unrelated_text() {
        assert!(
            !is_opencode_auth_error("all steps green\nRALPHY_DONE_EXIT\n"),
            "must not match a clean DONE sentinel"
        );
        assert!(
            !is_opencode_auth_error("timeout waiting for response"),
            "must not match an unrelated error"
        );
        assert!(!is_opencode_auth_error(""), "must not match empty text");
    }

    #[test]
    fn is_opencode_auth_error_takes_precedence_over_done_sentinel() {
        // A log that carries both a ProviderAuthError and a RALPHY_DONE_EXIT
        // sentinel must still be detected as an auth error — the auth signal wins.
        let log = "some work\n\
                   {\"type\":\"error\",\"name\":\"ProviderAuthError\",\"message\":\"signed out\"}\n\
                   RALPHY_DONE_EXIT\n";
        assert!(
            is_opencode_auth_error(log),
            "auth error must win over a co-present DONE sentinel"
        );
    }

    // ── is_opencode_api_degraded ─────────────────────────────────────────────

    #[test]
    fn degraded_matches_retryable_error_event() {
        // A 500 backend blip: a retryable error event, not a limit or auth — the
        // degraded clock should track it.
        let line = r#"{"type":"error","name":"APIError","statusCode":500,"message":"internal server error"}"#;
        assert!(is_opencode_api_degraded(line));
    }

    #[test]
    fn degraded_ignores_healthy_and_terminal_lines() {
        // Healthy JSON event lines and plain assistant text are not degraded.
        assert!(!is_opencode_api_degraded(
            r#"{"type":"text","text":"working on it"}"#
        ));
        assert!(!is_opencode_api_degraded("just some assistant prose"));
        // A terminal usage limit (429) is NOT degraded — it is handled as a limit.
        assert!(!is_opencode_api_degraded(
            r#"{"type":"error","name":"APIError","statusCode":429,"message":"rate limited"}"#
        ));
        // A terminal auth error is NOT degraded either.
        assert!(!is_opencode_api_degraded(
            r#"{"type":"error","name":"ProviderAuthError","message":"Not authenticated"}"#
        ));
    }

    // ── parse_opencode_limit ─────────────────────────────────────────────────

    #[test]
    fn parse_limit_apierror_429_with_reset_hint() {
        // Representative captured JSON: APIError + statusCode:429 + retryAfter field.
        let stream = r#"{"type":"text","text":"working"}
{"type":"error","name":"APIError","statusCode":429,"message":"rate limited","retryAfter":"2026-06-10T18:00:00Z"}
"#;
        assert_eq!(
            parse_opencode_limit(stream),
            Some(Some("2026-06-10T18:00:00Z".into()))
        );
    }

    #[test]
    fn parse_limit_apierror_429_without_reset_hint() {
        // APIError + 429 but no reset hint → Some(None).
        let stream = r#"{"type":"error","name":"APIError","statusCode":429,"message":"too many requests"}
"#;
        assert_eq!(parse_opencode_limit(stream), Some(None));
    }

    #[test]
    fn parse_limit_retryable_literal_string() {
        // Documented retryable() literal: "rate limit exceeded".
        let stream = r#"{"type":"error","name":"APIError","statusCode":429,"message":"Rate limit exceeded. Try again at 2026-06-10T19:00:00Z"}
"#;
        // Should detect as limit and extract a reset hint from the message.
        let result = parse_opencode_limit(stream);
        assert!(result.is_some(), "must detect as limit: {result:?}");
        // The reset hint is extracted from "try again at <value>".
        assert_eq!(result, Some(Some("2026-06-10T19:00:00Z".into())));
    }

    #[test]
    fn parse_limit_zen_usage_limit_error() {
        // Zen provider emits a *UsageLimitError name.
        let stream = r#"{"type":"error","name":"KimiUsageLimitError","message":"usage limit reached"}
"#;
        assert!(
            parse_opencode_limit(stream).is_some(),
            "must detect Zen *UsageLimitError"
        );
    }

    #[test]
    fn parse_limit_ignores_real_unknown_error_envelope() {
        // The exact error event captured live from opencode 1.16.2: a transient
        // backend failure, NOT a usage limit. It must not be misread as a limit.
        let stream = r#"{"type":"error","timestamp":1781088576836,"sessionID":"ses_x","error":{"name":"UnknownError","data":{"message":"Unexpected server error. Check server logs for details.","ref":"err_7391de1e"}}}"#;
        assert_eq!(
            parse_opencode_limit(stream),
            None,
            "an UnknownError backend blip is not a usage limit"
        );
        // But it IS an error event (downgrades a Done claim to Stuck on execute).
        let (_t, saw_error) = parse_opencode_events(stream);
        assert!(saw_error, "the real error envelope must flag saw_error");
    }

    #[test]
    fn parse_limit_detects_429_in_real_error_data_envelope() {
        // A 429 carried in the opencode 1.16.2 envelope: name + statusCode +
        // retryAfter live under `error.data`, not at the top level.
        let stream = r#"{"type":"error","sessionID":"s","error":{"name":"APIError","data":{"statusCode":429,"message":"rate limit exceeded","retryAfter":"2026-06-10T20:00:00Z"}}}"#;
        assert_eq!(
            parse_opencode_limit(stream),
            Some(Some("2026-06-10T20:00:00Z".into())),
            "must read name/statusCode/retryAfter from error.data"
        );
    }

    #[test]
    fn parse_limit_detects_kimi_403_billing_cycle_block() {
        // Kimi's billing-cycle quota block through OpenCode: an `APIError` with
        // statusCode 403 (not 429) and no reset hint — must map to Limit(None), the
        // exact live event shape that previously fell through to Stuck.
        let stream = r#"{"type":"error","error":{"name":"APIError","data":{"message":"You've reached your usage limit for this billing cycle. Your quota will be refreshed in the next cycle.","statusCode":403,"isRetryable":false}}}"#;
        assert_eq!(
            parse_opencode_limit(stream),
            Some(None),
            "Kimi's 403 billing-cycle block is a usage limit with no reset hint"
        );
    }

    #[test]
    fn parse_limit_plain_403_is_not_a_limit() {
        // A generic 403 (e.g. no access to a model) is a permission error, not a
        // usage limit — it must stay unrecognised so it fails as Stuck, not sleep.
        let stream = r#"{"type":"error","error":{"name":"APIError","data":{"message":"forbidden: model not available","statusCode":403}}}"#;
        assert_eq!(parse_opencode_limit(stream), None);
    }

    #[test]
    fn parse_limit_non_limit_status_500() {
        // A 500 error must not be classified as a limit.
        let stream = r#"{"type":"error","name":"APIError","statusCode":500,"message":"internal server error"}
"#;
        assert_eq!(parse_opencode_limit(stream), None);
    }

    #[test]
    fn parse_limit_clean_stream_no_limit() {
        // A clean stream with no error events yields None.
        let stream = r#"{"type":"text","text":"working on it"}
{"type":"text","text":"RALPHY_DONE_EXIT"}
{"type":"step_finish","reason":"stop"}
"#;
        assert_eq!(parse_opencode_limit(stream), None);
    }

    // ── parse_opencode_log_limit ─────────────────────────────────────────────

    #[test]
    fn log_limit_detects_zai_5h_cap_with_reset() {
        // The exact logfmt line opencode prints to stderr under `--print-logs` when
        // Z.ai's `zai-coding-plan` (GLM) hits its 5-hour cap — captured live
        // 2026-07-11 (FinCal #71). No `{type:"error"}` JSON event accompanies it, so
        // only the log-scan (not `parse_opencode_limit`) can catch it. The reset
        // value carries a space and must survive intact.
        let log = concat!(
            "{\"type\":\"step_finish\",\"reason\":\"stop\"}\n",
            "timestamp=2026-07-11T09:48:22.735Z level=ERROR run=d9ec1918 ",
            "message=\"stream error\" providerID=zai-coding-plan modelID=glm-5.2 ",
            "session.id=ses_x error.error=\"AI_APICallError: Usage limit reached for ",
            "5 hour. Your limit will reset at 2026-07-11 22:14:08\"",
        );
        assert_eq!(
            parse_opencode_log_limit(log),
            Some(Some("2026-07-11 22:14:08".into())),
        );
    }

    #[test]
    fn log_limit_detects_zai_weekly_limit_exhausted_wording() {
        // Z.ai's weekly/monthly block reads `Limit Exhausted`, not the 5h cap's
        // `Usage limit reached` — captured live 2026-07-13 (FinCal #77). The verb
        // `exhausted` sits directly after `Limit` with no `quota` prefix, so the
        // matcher must carry `limit exhausted` or this slips through and the child
        // burns the full wall budget in silent backoff instead of early-killing.
        let log = concat!(
            "timestamp=2026-07-13T17:06:04.998Z level=ERROR run=d9a3c1f7 ",
            "message=\"stream error\" providerID=zai-coding-plan modelID=glm-5.2 ",
            "session.id=ses_x error.error=\"AI_APICallError: Weekly/Monthly Limit ",
            "Exhausted. Your limit will reset at 2026-07-18 11:26:07\"",
        );
        assert_eq!(
            parse_opencode_log_limit(log),
            Some(Some("2026-07-18 11:26:07".into())),
        );
    }

    #[test]
    fn log_limit_detects_kimi_billing_cycle_without_reset() {
        // Kimi's billing-cycle block as it reads in the logfmt log: a usage limit
        // with no reset timestamp → `Some(None)`.
        let log = concat!(
            "timestamp=2026-07-09T23:19:01.732Z level=ERROR message=\"stream error\" ",
            "providerID=kimi-for-coding error.error=\"AI_APICallError: You've reached ",
            "your usage limit for this billing cycle. Your quota will be refreshed in ",
            "the next cycle.\"",
        );
        assert_eq!(parse_opencode_log_limit(log), Some(None));
    }

    #[test]
    fn log_limit_is_model_agnostic_and_extracts_iso_reset() {
        // A provider whose wording differs from Z.ai/Kimi ("rate limit" + "try again
        // at" + an ISO-Z reset): the model-agnostic class matcher catches it with no
        // per-model phrase, and the reset that follows the phrase is captured — not
        // the logfmt line's own leading `timestamp=` instant.
        let log = concat!(
            "timestamp=2026-08-01T09:00:00.000Z level=ERROR message=\"stream error\" ",
            "error.error=\"Rate limit exceeded. Please try again at 2026-08-01T10:30:00Z\"",
        );
        assert_eq!(
            parse_opencode_log_limit(log),
            Some(Some("2026-08-01T10:30:00Z".into())),
        );
    }

    #[test]
    fn log_limit_seen_without_usable_datetime_is_some_none() {
        // A usage limit whose only hint is relative ("in 30 minutes", no datetime):
        // the limit is detected but there is no schedulable reset, so it maps to
        // `Some(None)` — the caller then retries on the synthetic cadence rather than
        // parking on an unparseable string.
        let log = "level=ERROR error.error=\"Usage limit reached. Try again in 30 minutes.\"";
        assert_eq!(parse_opencode_log_limit(log), Some(None));
    }

    #[test]
    fn log_limit_ignores_ordinary_and_non_limit_error_lines() {
        // An INFO runtime line and a non-limit ERROR (transient backend blip) must
        // not be misread as a usage limit.
        let log = concat!(
            "{\"type\":\"text\",\"text\":\"working\"}\n",
            "timestamp=2026-07-11T09:47:41.590Z level=INFO message=\"llm runtime ",
            "selected\" llm.provider=zai-coding-plan llm.model=glm-5.2\n",
            "timestamp=2026-07-11T09:48:22.735Z level=ERROR message=\"stream error\" ",
            "error.error=\"AI_APICallError: Unexpected server error\"",
        );
        assert_eq!(parse_opencode_log_limit(log), None);
    }

    // ── parse_opencode_events ────────────────────────────────────────────────

    #[test]
    fn parse_extracts_text_parts() {
        let stream = "{\"type\":\"step_start\",\"snapshot\":\"abc\"}\n\
                      {\"type\":\"text\",\"text\":\"working on it\"}\n\
                      {\"type\":\"text\",\"text\":\"RALPHY_DONE_EXIT\"}\n\
                      {\"type\":\"step_finish\",\"reason\":\"stop\"}\n";
        let (text, saw_error) = parse_opencode_events(stream);
        assert!(text.contains("working on it"), "text: {text:?}");
        assert!(text.contains("RALPHY_DONE_EXIT"), "text: {text:?}");
        assert!(!saw_error);
    }

    #[test]
    fn parse_flags_error_event() {
        let stream = "{\"type\":\"text\",\"text\":\"trying\"}\n\
                      {\"type\":\"error\",\"name\":\"APIError\",\"statusCode\":500}\n";
        let (text, saw_error) = parse_opencode_events(stream);
        assert!(text.contains("trying"));
        assert!(saw_error, "an error event must set the flag");
    }

    #[test]
    fn parse_extracts_text_from_nested_part_envelope() {
        // The real opencode 1.16.2 `--format json` shape, captured live: every
        // event is wrapped `{type, timestamp, sessionID, part:{…}}` and the text
        // lives under `part.text`. The sentinel scan must see it through the
        // envelope, or every execute run misclassifies as Stuck.
        let stream = concat!(
            r#"{"type":"step_start","sessionID":"s","part":{"type":"step-start","snapshot":"abc"}}"#,
            "\n",
            r#"{"type":"tool_use","sessionID":"s","part":{"type":"tool","tool":"read","callID":"c1"}}"#,
            "\n",
            r#"{"type":"text","sessionID":"s","part":{"type":"text","text":"did the work\nRALPHY_DONE_EXIT"}}"#,
            "\n",
            r#"{"type":"step_finish","sessionID":"s","part":{"type":"step-finish","reason":"stop"}}"#,
            "\n",
        );
        let (text, saw_error) = parse_opencode_events(stream);
        assert!(
            text.contains("RALPHY_DONE_EXIT"),
            "must extract the sentinel from part.text: {text:?}"
        );
        assert!(
            ralphy_adapter_support::done_sentinel(&text),
            "done_sentinel must fire on the extracted text"
        );
        // A `tool` part must not be mistaken for text or an error.
        assert!(!saw_error, "a tool_use envelope is not an error");
    }

    #[test]
    fn parse_flags_error_event_in_nested_part() {
        // An error carried inside the `part` envelope must still set saw_error.
        let stream = r#"{"type":"error","sessionID":"s","part":{"type":"error","name":"APIError","statusCode":500}}"#;
        let (_text, saw_error) = parse_opencode_events(stream);
        assert!(saw_error, "a nested error part must flag saw_error");
    }

    #[test]
    fn parse_limit_detects_429_in_nested_part() {
        // The limit scan must read name/statusCode/retryAfter from `part` too.
        let stream = concat!(
            r#"{"type":"error","sessionID":"s","part":{"type":"error","name":"APIError","statusCode":429,"message":"rate limited","retryAfter":"2026-06-10T18:00:00Z"}}"#,
            "\n",
        );
        assert_eq!(
            parse_opencode_limit(stream),
            Some(Some("2026-06-10T18:00:00Z".into())),
            "must detect a 429 nested under part and extract the reset"
        );
    }

    #[test]
    fn parse_tolerates_unparseable_lines() {
        let stream = "not json at all\n\
                      {\"type\":\"text\",\"text\":\"kept\"}\n\
                      \n";
        let (text, saw_error) = parse_opencode_events(stream);
        assert_eq!(text.trim(), "kept");
        assert!(!saw_error);
    }
}
