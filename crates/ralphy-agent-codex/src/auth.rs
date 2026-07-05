//! Codex authentication and usage-limit text detection: signals recovered from
//! `codex exec` stdout/stderr that the process exit code alone can't distinguish.

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
