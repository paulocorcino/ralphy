//! Parsing `OpenCode`'s `--format json` line-delimited event stream: extracting
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
/// carries no `quota` prefix — `FinCal` #77, 2026-07-13) is caught. Compiled once per
/// scan (or once per early-kill hook build in the execute path —
/// [`OpenCodeAgent::execute`]).
pub(crate) fn usage_limit_regex() -> Regex {
    Regex::new(concat!(
        r"(?i)usage limit|rate[ _]?limit|limit (?:reached|exceeded|exhausted)",
        r"|too many requests|quota (?:exceeded|exhausted)",
    ))
    .expect("valid usage-limit regex")
}

/// The actionable message shown when `is_opencode_auth_error` fires — tells the
/// operator exactly what to do to recover (run `opencode auth login`).
pub(crate) const OPENCODE_AUTH_ERROR_MSG: &str =
    "OpenCode is not authenticated (ProviderAuthError) — run `opencode auth login` and retry";

/// Return `true` when `text` (the combined stdout+stderr log) shows an `OpenCode`
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

/// Parse `OpenCode`'s `--format json` line-delimited event stream: concatenate the
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
        if let Some(t) = text_part(&val) {
            text.push_str(t);
            text.push('\n');
        }
    }
    (text, saw_error)
}

/// The assistant text one event carries when the event, or its `part`
/// payload, is typed `text`. The text is read from the payload.
fn text_part(val: &serde_json::Value) -> Option<&str> {
    let payload = event_payload(val);
    let is_text = val.get("type").and_then(|v| v.as_str()) == Some("text")
        || payload.get("type").and_then(|v| v.as_str()) == Some("text");
    if !is_text {
        return None;
    }
    payload.get("text").and_then(|v| v.as_str())
}

/// Extract a reset hint from an `OpenCode` error event or message text (best-effort).
/// Looks for a `retryAfter` field value, or a `Retry-After` / "try again" substring
/// in the message. Returns `None` when absent (D9: reset hint is not guaranteed).
fn parse_opencode_reset_hint(event: &serde_json::Value) -> Option<String> {
    if let Some(hint) = retry_after_field(event) {
        return Some(hint);
    }
    // "try again" or "Retry-After" in the message text.
    let msg = event.get("message").and_then(|v| v.as_str())?;
    let lower = msg.to_ascii_lowercase();
    retry_after_header(msg, &lower).or_else(|| try_again_phrase(msg, &lower))
}

/// The `retryAfter` field on the event object, as text.
fn retry_after_field(event: &serde_json::Value) -> Option<String> {
    let s = match event.get("retryAfter")? {
        serde_json::Value::String(s) => s.clone(),
        other @ (serde_json::Value::Null
        | serde_json::Value::Bool(_)
        | serde_json::Value::Number(_)
        | serde_json::Value::Array(_)
        | serde_json::Value::Object(_)) => other.to_string(),
    };
    (!s.is_empty() && s != "null").then_some(s)
}

/// The first word after `retry-after:` in the message, without a trailing comma.
/// `lower` is the ASCII-lowercase of `msg`, so byte positions agree.
fn retry_after_header(msg: &str, lower: &str) -> Option<String> {
    let pos = lower.find("retry-after:")?;
    let rest = msg[pos + "retry-after:".len()..].trim();
    let hint = rest
        .split_whitespace()
        .next()
        .unwrap_or("")
        .trim_end_matches(',');
    (!hint.is_empty()).then(|| hint.to_string())
}

/// The text after "try again at " or "try again in ", up to the first `.` or
/// newline. `lower` is the ASCII-lowercase of `msg`, so byte positions agree.
fn try_again_phrase(msg: &str, lower: &str) -> Option<String> {
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
    None
}

// Scan the line-delimited JSON event stream for a usage-limit signal (ADR-0005 D9).
///
/// Returns:
/// - `Some(Some(hint))` — a limit event was seen and carries a reset hint.
/// - `Some(None)` — a limit event was seen but no reset hint was found.
/// - `None` — no limit event was seen.
///
/// Detects three documented shapes:
/// 1. `name:"APIError"` + `statusCode:429` (the SDK's rate-limit error).
/// 2. Literal rate-limit strings from `OpenCode`'s `retryable()` function
///    (`retry.ts`): "`rate_limit_error`", "rate limit exceeded", "too many requests",
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
            .and_then(serde_json::Value::as_u64)
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
/// (observed live 2026-07-11, `FinCal` #71, glm-5.2). Uses the same model-agnostic
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
    let re = Regex::new(concat!(
        r"(?i)(?:reset(?:s)?(?:\s+(?:at|in))?|try\s+again\s+(?:at|in))",
        r"\s*[:=]?\s*",
        r"(\d{4}-\d{2}-\d{2}[ T]\d{2}:\d{2}(?::\d{2})?(?:z|[+-]\d{2}:?\d{2})?",
        r"|\d{1,2}:\d{2}(?:\s*[ap]m)?)",
    ))
    .expect("valid reset-hint regex");
    re.captures(line)
        .and_then(|caps| caps.get(1))
        .map(|m| m.as_str().trim().to_string())
        .filter(|hint| !hint.is_empty())
}

#[cfg(test)]
mod tests;
