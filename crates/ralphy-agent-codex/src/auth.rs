//! Codex authentication and usage-limit text detection: signals recovered from
//! `codex exec` stdout/stderr that the process exit code alone can't distinguish.

use chrono::{DateTime, Local, NaiveDateTime, NaiveTime};
use ralphy_core::ResetAt;

/// The actionable message surfaced when a run hits a Codex authentication
/// failure — the account is signed out or its credentials were revoked.
pub(crate) const CODEX_AUTH_ERROR_MSG: &str =
    "Codex is not authenticated (401 Unauthorized) — run `codex login` and retry";

/// Return `true` when `text` shows a Codex authentication failure (account
/// signed out / credentials revoked). A logged-out `codex exec` prints a `401
/// Unauthorized` with `Missing bearer or basic authentication in header` and
/// writes no `-o` file, so without this the failure masquerades as a generic
/// "no plan" (planning) or `Outcome::Stuck` (execution) — both of which hide the
/// real cause. Either signal alone is auth-specific; matching either keeps the
/// detector robust to Codex reformatting one of the two lines.
pub(crate) fn is_codex_auth_error(text: &str) -> bool {
    // OR of two single-substring groups: either signal alone is auth-specific.
    ralphy_adapter_support::auth_error(
        text,
        &[
            &["401 unauthorized"],
            &["missing bearer or basic authentication"],
        ],
    )
}

/// Return `true` when `text` contains a Codex usage-limit message (case-insensitive).
pub(crate) fn is_codex_limit_text(text: &str) -> bool {
    // to_ascii_lowercase is used so byte offsets are preserved (ASCII-only pattern).
    let lower = text.to_ascii_lowercase();
    lower.contains("you've hit your usage limit")
        || lower.contains("usage limit")
        || lower.contains("rate limit reached")
}

/// Extract the reset hint from a Codex limit message: the text following
/// `try again at ` (trimmed, to end of line). Returns `None` when absent.
pub(crate) fn parse_codex_reset_hint(text: &str) -> Option<String> {
    for line in text.lines() {
        // to_ascii_lowercase preserves byte positions so the pos from find()
        // can safely index back into line (no Unicode expansion hazard).
        let lower = line.to_ascii_lowercase();
        if let Some(pos) = lower.find("try again at ") {
            // Strip trailing sentence punctuation an error line leaves on the hint
            // (e.g. "… try again at <ts>.") so the hint is clean for the parser.
            let rest = line[pos + "try again at ".len()..]
                .trim()
                .trim_end_matches('.')
                .trim();
            if !rest.is_empty() {
                return Some(rest.to_string());
            }
        }
    }
    None
}

/// The reset of a Codex limit message: the vendor's text and the instant it
/// names, or `None` when the message names no reset.
pub(crate) fn codex_reset(text: &str) -> Option<ResetAt> {
    let hint = parse_codex_reset_hint(text)?;
    let at = reset_instant(&hint, Local::now()).map(|t| t.fixed_offset());
    Some(ResetAt { text: hint, at })
}

/// The instant a Codex reset hint names, relative to `now`. The hint is one of:
/// an RFC3339 instant (`"2026-06-09T18:00:00Z"`, alone or trailed by prose), a
/// month-name date-time (`"Jun 10th, 2026 12:23 AM"`), a numeric date-time with
/// no zone (`"2026-07-12 11:11:48"`), or a bare 12-hour (`"6:13 PM"`) or 24-hour
/// (`"18:13"`) time. The date-times name their own date and are used as they are
/// (a zoneless one is read as local wall-clock time); `now` only places a bare
/// time, which is today when still ahead, else tomorrow. `None` when the hint is
/// none of these.
fn reset_instant(hint: &str, now: DateTime<Local>) -> Option<DateTime<Local>> {
    // Trailing sentence punctuation an error line may leave on the hint.
    let trimmed = hint.trim().trim_end_matches('.').trim();
    parse_rfc3339(trimmed)
        .or_else(|| parse_month_name_datetime(trimmed))
        .or_else(|| parse_numeric_local_datetime(trimmed))
        .or_else(|| parse_12h_time(trimmed).and_then(|time| next_time_of_day(time, now)))
        // Last, so a hint with an AM/PM marker is read as 12-hour time first.
        .or_else(|| {
            let time = NaiveTime::parse_from_str(trimmed, "%H:%M").ok()?;
            next_time_of_day(time, now)
        })
}

/// An RFC3339 instant in local time: the whole hint, or its leading token when
/// prose trails the datetime.
fn parse_rfc3339(trimmed: &str) -> Option<DateTime<Local>> {
    let leading = trimmed.split_whitespace().next().unwrap_or(trimmed);
    [trimmed, leading.trim_end_matches('.')]
        .into_iter()
        .find_map(|cand| DateTime::parse_from_rfc3339(cand).ok())
        .map(|dt| dt.with_timezone(&Local))
}

/// A numeric ISO-like date-time with no zone, read as local wall-clock time.
fn parse_numeric_local_datetime(trimmed: &str) -> Option<DateTime<Local>> {
    [
        "%Y-%m-%d %H:%M:%S",
        "%Y-%m-%dT%H:%M:%S",
        "%Y-%m-%d %H:%M",
        "%Y-%m-%dT%H:%M",
    ]
    .into_iter()
    .find_map(|fmt| NaiveDateTime::parse_from_str(trimmed, fmt).ok())
    .and_then(|ndt| ndt.and_local_timezone(Local).single())
}

/// A month-name date-time like `"Jun 10th, 2026 12:23 AM"` in local time.
/// Ordinal suffixes and commas are stripped first so chrono's format specifiers
/// match.
fn parse_month_name_datetime(s: &str) -> Option<DateTime<Local>> {
    let cleaned = s.replace(',', " ");
    let normalized = cleaned
        .split_whitespace()
        .map(strip_ordinal_suffix)
        .collect::<Vec<_>>()
        .join(" ");
    [
        "%b %d %Y %I:%M %p",
        "%B %d %Y %I:%M %p",
        "%b %d %Y %H:%M",
        "%B %d %Y %H:%M",
    ]
    .into_iter()
    .find_map(|fmt| NaiveDateTime::parse_from_str(&normalized, fmt).ok())
    .and_then(|ndt| ndt.and_local_timezone(Local).single())
}

/// Drop an ordinal suffix (`st`/`nd`/`rd`/`th`) from a token that is otherwise all
/// digits, so `"10th"` becomes `"10"`. Any other token is returned unchanged.
fn strip_ordinal_suffix(tok: &str) -> &str {
    for suf in ["st", "nd", "rd", "th"] {
        if let Some(stem) = tok.strip_suffix(suf) {
            if !stem.is_empty() && stem.chars().all(|c| c.is_ascii_digit()) {
                return stem;
            }
        }
    }
    tok
}

/// A bare 12-hour clock time like `"6:13 PM"`. `None` when the AM/PM marker is
/// absent.
fn parse_12h_time(s: &str) -> Option<NaiveTime> {
    let upper = s.trim().to_uppercase();
    if !upper.ends_with("AM") && !upper.ends_with("PM") {
        return None;
    }
    // chrono's numeric specifiers accept an unpadded hour, so "6:13 PM" parses.
    NaiveTime::parse_from_str(&upper, "%I:%M %p").ok()
}

/// The next local instant carrying `time`: today when still ahead, else tomorrow.
fn next_time_of_day(time: NaiveTime, now: DateTime<Local>) -> Option<DateTime<Local>> {
    let today = now.date_naive();
    let date = if now.time() < time {
        today
    } else {
        today + chrono::Duration::days(1)
    };
    date.and_time(time).and_local_timezone(Local).single()
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── is_codex_limit_text ─────────────────────────────────────────────────

    #[test]
    fn is_codex_limit_text_matches_known_phrases() {
        assert!(is_codex_limit_text(
            "Sorry, you've hit your usage limit for today."
        ));
        assert!(is_codex_limit_text("You've Hit Your Usage Limit"));
        assert!(is_codex_limit_text("usage limit exceeded"));
        assert!(is_codex_limit_text(
            "Error: Rate Limit Reached. Please try again later."
        ));
        assert!(!is_codex_limit_text("all steps green\nRALPHY_DONE_EXIT\n"));
    }

    // ── is_codex_auth_error ─────────────────────────────────────────────────

    #[test]
    fn is_codex_auth_error_matches_real_logged_out_log() {
        // The verbatim stderr a `codex exec` (v0.138.0) emitted with the account
        // signed out: a 401 with the missing-bearer body and reconnect attempts.
        let log = "ERROR codex_api::endpoint::responses_websocket: failed to connect \
                   to websocket: HTTP error: 401 Unauthorized, url: \
                   wss://api.openai.com/v1/responses\nERROR: Reconnecting... 5/5\n\
                   ERROR: unexpected status 401 Unauthorized: Missing bearer or basic \
                   authentication in header, url: https://api.openai.com/v1/responses";
        assert!(is_codex_auth_error(log));
    }

    #[test]
    fn is_codex_auth_error_matches_either_signal_alone() {
        assert!(is_codex_auth_error("HTTP error: 401 Unauthorized"));
        assert!(is_codex_auth_error(
            "Missing bearer or basic authentication in header"
        ));
        // Case-insensitive.
        assert!(is_codex_auth_error("401 UNAUTHORIZED"));
    }

    #[test]
    fn is_codex_auth_error_ignores_unrelated_and_limit_text() {
        assert!(!is_codex_auth_error("all steps green\nRALPHY_DONE_EXIT\n"));
        // A usage limit is a different failure, not an auth error.
        assert!(!is_codex_auth_error(
            "You've hit your usage limit. Try again at 2026-06-09T18:00:00Z."
        ));
    }

    /// Build a fixed `DateTime<Local>` for the deterministic parse tests.
    fn local(y: i32, mo: u32, d: u32, h: u32, mi: u32, sec: u32) -> DateTime<Local> {
        use chrono::TimeZone;
        Local
            .with_ymd_and_hms(y, mo, d, h, mi, sec)
            .single()
            .unwrap()
    }

    /// An RFC3339 text as the epoch it names, so the expectation does not
    /// depend on the local zone.
    fn epoch(rfc3339: &str) -> i64 {
        DateTime::parse_from_rfc3339(rfc3339).unwrap().timestamp()
    }

    #[test]
    fn reset_hint_parses_each_codex_form() {
        let now = local(2026, 6, 9, 15, 20, 0);
        let cases = [
            // (hint, now, expected instant)
            ("6:13 PM", now, local(2026, 6, 9, 18, 13, 0)),
            ("18:13", now, local(2026, 6, 9, 18, 13, 0)),
            (
                "18:13",
                local(2026, 6, 9, 19, 0, 0),
                local(2026, 6, 10, 18, 13, 0),
            ),
            (
                "6:13 PM",
                local(2026, 6, 9, 19, 0, 0),
                local(2026, 6, 10, 18, 13, 0),
            ),
            (
                "12:23 AM",
                local(2026, 6, 9, 23, 0, 0),
                local(2026, 6, 10, 0, 23, 0),
            ),
            ("Jun 10th, 2026 12:23 AM", now, local(2026, 6, 10, 0, 23, 0)),
            ("2026-07-12 11:11:48", now, local(2026, 7, 12, 11, 11, 48)),
            ("2026-07-12T09:00", now, local(2026, 7, 12, 9, 0, 0)),
        ];
        for (hint, now, expected) in cases {
            assert_eq!(reset_instant(hint, now), Some(expected), "hint {hint:?}");
        }
        let rfc3339_cases = [
            ("2026-06-09T18:00:00Z", "2026-06-09T18:00:00Z"),
            ("2026-06-09T18:00:00Z.", "2026-06-09T18:00:00Z"),
            ("2026-06-09T18:00:00Z (in 3 hours)", "2026-06-09T18:00:00Z"),
            ("2026-06-09T15:00:00-03:00", "2026-06-09T15:00:00-03:00"),
        ];
        for (hint, instant) in rfc3339_cases {
            let got = reset_instant(hint, now).map(|t| t.timestamp());
            assert_eq!(got, Some(epoch(instant)), "hint {hint:?}");
        }
    }

    #[test]
    fn an_unparseable_reset_hint_keeps_its_text() {
        let reset = codex_reset("You've hit your usage limit. Try again at soon-ish.");
        assert_eq!(
            reset,
            Some(ResetAt {
                text: "soon-ish".to_string(),
                at: None
            })
        );
    }

    // ── parse_codex_reset_hint ──────────────────────────────────────────────

    #[test]
    fn parse_codex_reset_hint_extracts_datetime() {
        let text = "You've hit your usage limit. Try again at 2026-06-09T18:00:00Z.";
        assert_eq!(
            parse_codex_reset_hint(text).as_deref(),
            Some("2026-06-09T18:00:00Z")
        );
    }

    #[test]
    fn parse_codex_reset_hint_returns_none_when_absent() {
        assert_eq!(
            parse_codex_reset_hint("usage limit exceeded, no reset info"),
            None
        );
    }

    #[test]
    fn detects_real_codex_plan_limit_log() {
        // The exact ERROR line a `codex exec` plan emitted on a usage limit: the
        // adapter's plan() classifies this into a PlanLimit carrying the hint.
        let log = "ERROR: You've hit your usage limit. Upgrade to Pro \
                   (https://chatgpt.com/explore/pro), visit \
                   https://chatgpt.com/codex/settings/usage to purchase more \
                   credits or try again at Jun 10th, 2026 12:23 AM.";
        assert!(is_codex_limit_text(log));
        assert_eq!(
            parse_codex_reset_hint(log).as_deref(),
            Some("Jun 10th, 2026 12:23 AM")
        );
    }
}
