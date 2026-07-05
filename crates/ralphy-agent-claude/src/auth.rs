//! Claude authentication and usage-limit detection: signals recovered from
//! `claude` stdout and session transcripts that the process exit code alone
//! can't distinguish (auth failures vs usage limits vs a genuine stall).

/// The actionable message surfaced when a run hits a Claude Code authentication
/// failure — the account is signed out or has never been logged in.
pub(crate) const CLAUDE_AUTH_ERROR_MSG: &str =
    "Claude Code is not authenticated — run `claude login` and retry";

/// Return `true` when `text` shows a Claude Code authentication failure.
/// A logged-out headless `claude -p` prints `Not logged in · Please run /login`
/// on stdout and exits with code 1 (verified against CLI v2.1.170), so without
/// this the failure masquerades as a generic "no plan" (planning) or
/// `Outcome::Stuck` (headless execution) — both of which hide the real cause.
/// The line is a `-p`-only signal: an *interactive* logged-out session instead
/// renders the onboarding/login TUI and stalls, so the live path detects auth
/// failure only when it surfaces in the transcript (mid-session revocation).
/// That gap is benign because `plan` runs headless first and bails here before
/// `execute` is ever reached.
///
/// Detection is per-line and skips `user`/`assistant` transcript records. In
/// `--output-format stream-json` (the plan path) the log carries `tool_result`
/// records whose content is *the files the agent read* — and this adapter's own
/// source documents the `Not logged in · Please run /login` string, so a naive
/// whole-text scan self-triggers the moment a "repo diagnosis" plan reads
/// `lib.rs`. The genuine signal is never a `user`/`assistant` record: it is a
/// CLI-level message (plain text in default `-p`, a `system`/`result` record in
/// stream-json) emitted before the model loop runs. Plain output has no JSON
/// envelope, so its lines are scanned as-is.
pub(crate) fn is_claude_auth_error(text: &str) -> bool {
    text.lines().any(|line| {
        let trimmed = line.trim_start();
        if trimmed.starts_with('{') {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(trimmed) {
                if matches!(
                    v.get("type").and_then(|t| t.as_str()),
                    Some("user") | Some("assistant")
                ) {
                    return false;
                }
            }
        }
        // One AND-group: the genuine CLI banner carries both substrings on the
        // same line, so an AND avoids matching prose that mentions only one.
        ralphy_adapter_support::auth_error(line, &[&["not logged in", "please run /login"]])
    })
}

/// Whether text looks like a subscription usage/rate-limit notice. Ports the ps1
/// `Test-LimitText` oracle. Used only on the bounded `claude -p` **stdout**
/// channels (plan / headless-exec), never on the live PTY transcript — see
/// [`transcript_limit`] for why a raw scan is unsafe there.
pub(crate) fn is_limit_text(text: &str) -> bool {
    use regex::Regex;
    let re = Regex::new(
        r"(?i)(rate[_ -]?limit|usage limit|session limit|reached your .* limit|limit reached|resets\s+\d)",
    )
    .expect("valid regex");
    re.is_match(text)
}

/// Detect a *genuine* usage-limit banner in a Claude session transcript JSONL,
/// returning `Some(reset_hint)` (the hint itself may be `None`) when found and
/// `None` otherwise.
///
/// This is line-oriented and **anchored on the API-error structure** — the real
/// banner is an assistant line carrying `isApiErrorMessage: true` together with
/// `error: "rate_limit"` or `apiErrorStatus: 429` (verified against a captured
/// 429), or a `rate_limit_event` whose status is `rejected`. A raw substring
/// scan ([`is_limit_text`]) over the whole transcript cannot be used here: the
/// transcript records everything the agent *read and wrote*, so it false-trips
/// the instant the agent touches source that merely mentions "usage limit" /
/// "session limit" — which is exactly what happens when ralphy runs against a
/// repo about rate limiting (its own included, where the test fixtures alone
/// carry the phrase hundreds of times). Only Claude's own injected error line is
/// a limit; prose in tool results and assistant text is not.
///
/// The reset hint is parsed from that error line's own text via
/// [`parse_reset_hhmm`] (e.g. `"You've hit your session limit · resets 8:10am"`
/// → `Some("08:10")`).
pub(crate) fn transcript_limit(jsonl: &str) -> Option<Option<String>> {
    // Shares the line-delimited-JSON scan scaffold with OpenCode's limit parser
    // (`scan_json_lines`); the rate-limit *predicate* and reset-hint *format* stay
    // Claude-specific.
    ralphy_adapter_support::scan_json_lines(jsonl, |v| {
        line_is_rate_limit_error(v)
            .then(|| limit_line_text(v).as_deref().and_then(parse_reset_hhmm))
    })
}

/// Whether a parsed transcript line is Claude's own rate-limit error — either an
/// `isApiErrorMessage` line whose `error`/`apiErrorStatus` marks a rate limit, or
/// a rejected `rate_limit_event`.
fn line_is_rate_limit_error(v: &serde_json::Value) -> bool {
    let api_rate_limited = v
        .get("isApiErrorMessage")
        .and_then(|b| b.as_bool())
        .unwrap_or(false)
        && (v.get("error").and_then(|e| e.as_str()) == Some("rate_limit")
            || v.get("apiErrorStatus").and_then(|s| s.as_u64()) == Some(429));
    let rate_limit_event = v.get("type").and_then(|t| t.as_str()) == Some("rate_limit_event")
        && v.get("rate_limit_info")
            .and_then(|i| i.get("status"))
            .and_then(|s| s.as_str())
            == Some("rejected");
    api_rate_limited || rate_limit_event
}

/// Concatenate the `text` blocks of a transcript line's `message.content`, so the
/// reset hint can be parsed from the banner Claude rendered into it. `None` when
/// no text is present.
fn limit_line_text(v: &serde_json::Value) -> Option<String> {
    let blocks = v.get("message")?.get("content")?.as_array()?;
    let text: String = blocks
        .iter()
        .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
        .collect();
    (!text.is_empty()).then_some(text)
}

/// Parse a reset time from a usage-limit transcript. Looks for a pattern like
/// "resets 3pm", "resets 3:00pm", or "resets Tue 12:30am" and converts it to 24h
/// `HH:mm` (minutes default to `00` when absent). When a day-of-week prefixes the
/// time it is captured, Title-cased, and prepended (`"Tue 00:30"`); a bare time
/// stays bare (`"15:00"`). Returns `None` when no match is found. Ports
/// `Get-ResetDateTime`; the optional weekday lets the core compute the next
/// correct occurrence rather than assuming "today".
pub(crate) fn parse_reset_hhmm(text: &str) -> Option<String> {
    use regex::Regex;
    let re = Regex::new(r"(?i)resets\s+(?:([a-z]{3})\s+)?(\d{1,2})(?::(\d{2}))?\s*([ap]m)")
        .expect("valid regex");
    let caps = re.captures(text)?;
    let hour: u32 = caps[2].parse().ok()?;
    let min: u32 = caps.get(3).map_or(Ok(0), |m| m.as_str().parse()).ok()?;
    let ampm = caps[4].to_lowercase();
    let hour24 = match ampm.as_str() {
        "am" => hour % 12,
        _ => (hour % 12) + 12,
    };
    let hhmm = format!("{:02}:{:02}", hour24, min);
    match caps.get(1) {
        Some(wd) => Some(format!("{} {}", title_case_weekday(wd.as_str()), hhmm)),
        None => Some(hhmm),
    }
}

/// Title-case a three-letter weekday abbreviation (`"tue"` → `"Tue"`).
fn title_case_weekday(wd: &str) -> String {
    let mut chars = wd.chars();
    match chars.next() {
        Some(first) => first
            .to_uppercase()
            .chain(chars.flat_map(|c| c.to_lowercase()))
            .collect(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One real transcript api-error line carrying the limit banner `text`, in the
    /// exact shape Claude Code writes (`isApiErrorMessage`+`error`+`apiErrorStatus`).
    fn limit_jsonl(text: &str) -> String {
        serde_json::json!({
            "type": "assistant",
            "isApiErrorMessage": true,
            "error": "rate_limit",
            "apiErrorStatus": 429,
            "message": { "role": "assistant", "content": [ { "type": "text", "text": text } ] }
        })
        .to_string()
    }

    #[test]
    fn transcript_limit_detects_real_429_error_line() {
        let t = limit_jsonl("You've hit your session limit · resets 8:10am (America/Bahia)");
        assert_eq!(transcript_limit(&t), Some(Some("08:10".into())));
    }

    #[test]
    fn transcript_limit_detects_rejected_rate_limit_event() {
        let t = r#"{"type":"rate_limit_event","rate_limit_info":{"status":"rejected"}}"#;
        assert_eq!(transcript_limit(t), Some(None));
    }

    #[test]
    fn limit_text_matches_claude_rate_limit_event() {
        let log = r#"{"type":"rate_limit_event","rate_limit_info":{"status":"rejected"}}"#;
        assert!(is_limit_text(log));
    }

    #[test]
    fn limit_text_matches_session_limit_message() {
        let log = "You've hit your session limit · resets 8:10am (America/Bahia)";
        assert!(is_limit_text(log));
        assert_eq!(parse_reset_hhmm(log), Some("08:10".into()));
    }

    #[test]
    fn parse_reset_hhmm_converts_pm() {
        assert_eq!(parse_reset_hhmm("resets 3:00pm"), Some("15:00".into()));
    }

    #[test]
    fn parse_reset_hhmm_midnight() {
        assert_eq!(parse_reset_hhmm("resets 12:30am"), Some("00:30".into()));
    }

    #[test]
    fn parse_reset_hhmm_without_minutes() {
        assert_eq!(parse_reset_hhmm("resets 3pm"), Some("15:00".into()));
        assert_eq!(parse_reset_hhmm("resets 12am"), Some("00:00".into()));
    }

    #[test]
    fn parse_reset_hhmm_no_match() {
        assert_eq!(parse_reset_hhmm("no time here"), None);
    }

    #[test]
    fn parse_reset_hhmm_captures_weekday() {
        // A weekday-qualified reset is captured and prefixed, Title-cased; the
        // bare-time form is unchanged.
        assert_eq!(
            parse_reset_hhmm("You've reached your usage limit; resets Tue 12:30am"),
            Some("Tue 00:30".into())
        );
        assert_eq!(parse_reset_hhmm("resets 3:00pm"), Some("15:00".into()));
    }

    // ── is_claude_auth_error ────────────────────────────────────────────────

    #[test]
    fn is_claude_auth_error_matches_logged_out_output() {
        assert!(is_claude_auth_error(
            "Not logged in \u{00b7} Please run /login"
        ));
    }

    #[test]
    fn is_claude_auth_error_matches_case_insensitive() {
        assert!(is_claude_auth_error(
            "NOT LOGGED IN \u{00b7} PLEASE RUN /LOGIN"
        ));
    }

    #[test]
    fn is_claude_auth_error_requires_both_signals() {
        assert!(!is_claude_auth_error("Not logged in"));
        assert!(!is_claude_auth_error("Please run /login"));
        assert!(!is_claude_auth_error("all steps green\nRALPHY_DONE_EXIT\n"));
    }

    #[test]
    fn is_claude_auth_error_ignores_file_content_in_tool_results() {
        // A "repo diagnosis" plan reads this adapter's own source, whose doc
        // comment quotes `Not logged in · Please run /login`. In stream-json the
        // read lands in a `type":"user"` tool_result — it must NOT be read as a
        // real auth failure (regression: run 20260625-145058).
        let line = serde_json::json!({
            "type": "user",
            "message": {"role": "user", "content": [{
                "type": "tool_result",
                "content": "//! prints `Not logged in \u{00b7} Please run /login` on stdout",
            }]},
        })
        .to_string();
        assert!(!is_claude_auth_error(&line));
    }

    #[test]
    fn is_claude_auth_error_ignores_assistant_prose() {
        // The planning agent may *describe* the auth detector in its own message;
        // an assistant record is never the genuine CLI signal.
        let line = serde_json::json!({
            "type": "assistant",
            "message": {"role": "assistant", "content": [{
                "type": "text",
                "text": "It checks for `Not logged in \u{00b7} Please run /login`.",
            }]},
        })
        .to_string();
        assert!(!is_claude_auth_error(&line));
    }

    #[test]
    fn is_claude_auth_error_detects_real_signal_amid_tool_results() {
        // The genuine CLI message (plain line in default `-p`, or a non-user
        // record in stream-json) still fires even when file-content noise
        // precedes it.
        let log = format!(
            "{}\nNot logged in \u{00b7} Please run /login\n",
            serde_json::json!({
                "type": "user",
                "message": {"content": [{"type": "tool_result", "content": "harmless file body"}]},
            })
        );
        assert!(is_claude_auth_error(&log));
    }
}
