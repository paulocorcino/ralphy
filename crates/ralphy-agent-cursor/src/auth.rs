//! Cursor authentication detection (ADR-0042 D8), in two tiers: a free,
//! machine-readable preflight, and an in-flight stderr matcher for a token that
//! expires mid-run.

use std::time::Duration;

use serde_json::Value;

/// The actionable message surfaced when a run hits a Cursor authentication
/// failure. It quotes the login command **verbatim** — `agent login` is what the
/// CLI itself prints, regardless of which of its two binary names was invoked.
pub const CURSOR_AUTH_ERROR_MSG: &str =
    "Cursor is not authenticated — run `agent login` (or `cursor-agent login`) and retry";

/// Return `true` when `text` shows a Cursor authentication failure.
///
/// Three distinct vendor strings exist and a naive matcher misses one (D8): the
/// listing path and the execution path both say `Authentication required`, but the
/// invalid-key warning — `⚠ Warning: The provided API key is invalid.` — shares no
/// words with them. Matching only the first phrase would let an invalid key
/// masquerade as a generic "no plan".
pub(crate) fn is_cursor_auth_error(text: &str) -> bool {
    ralphy_adapter_support::auth_error(
        text,
        &[
            &["authentication required"],
            &["the provided api key is invalid"],
        ],
    )
}

/// Parse `agent status --format json` into "is the operator logged in".
///
/// **The exit code is not a parameter, deliberately: `status` exits 0 while logged
/// out** (D8), so a caller that gated on it would report every logged-out operator
/// as authenticated. The verdict comes from `isAuthenticated` alone; an
/// unparsable or absent answer is `false`, which fails toward telling the operator
/// to log in rather than toward a spawn that cannot work.
pub fn cursor_status_verdict(stdout: &str) -> bool {
    stdout
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter_map(|v| v.get("isAuthenticated").and_then(Value::as_bool))
        .next_back()
        .unwrap_or(false)
}

/// Ask the CLI itself whether the operator is logged in — the ADR-0013 preflight,
/// and what `ralphy init`'s gate reports. Behavioural detection: the vendor's own
/// answer, never inspection of its credential file.
///
/// A missing binary, a wedged probe or a timeout all read as "not logged in": the
/// gate's job is to tell the operator what to fix, and `agent login` is the right
/// advice in every one of those states.
pub fn probe_cursor_login() -> bool {
    let mut cmd = std::process::Command::new(crate::command::resolve_cursor_program());
    cmd.arg("status")
        .arg("--format")
        .arg("json")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    match ralphy_adapter_support::run_headless(cmd, "", Duration::from_secs(30)) {
        Ok(out) if !out.timed_out => cursor_status_verdict(&out.stdout),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The three literal strings the spike captured. The third is the reason the
    /// predicate is a disjunction rather than one phrase.
    const LISTING_PATH: &str = "Error: Authentication required. Run 'agent login', pass --api-key/--auth-token, or set CURSOR_API_KEY/CURSOR_AUTH_TOKEN.";
    const EXECUTION_PATH: &str = "Error: Authentication required. Please run 'agent login' first, or set CURSOR_API_KEY environment variable.";
    const INVALID_KEY: &str = "⚠ Warning: The provided API key is invalid.";

    #[test]
    fn all_three_vendor_strings_classify_as_auth() {
        for s in [LISTING_PATH, EXECUTION_PATH, INVALID_KEY] {
            assert!(is_cursor_auth_error(s), "should classify as auth: {s}");
        }
        assert!(!is_cursor_auth_error("everything is fine"));
    }

    /// The one that a two-string matcher would miss: it contains neither
    /// `Authentication` nor `required`.
    #[test]
    fn the_invalid_key_warning_shares_no_words_with_the_other_two() {
        let lower = INVALID_KEY.to_ascii_lowercase();
        assert!(!lower.contains("authentication required"));
        assert!(is_cursor_auth_error(INVALID_KEY));
    }

    /// The whole point of D8's tier 1: `status` exits **0** while logged out, so a
    /// verdict that consulted the exit code would be wrong in exactly the case it
    /// exists to catch. The exit code is passed here only to show it is ignored.
    #[test]
    fn status_json_verdict_ignores_the_exit_code() {
        const LOGGED_OUT: &str = r#"{"status":"unauthenticated","isAuthenticated":false,"hasAccessToken":false,"hasRefreshToken":false,"message":"Not logged in"}"#;
        let exit_code = 0;
        assert_eq!(exit_code, 0, "the vendor exits 0 while logged out");
        assert!(
            !cursor_status_verdict(LOGGED_OUT),
            "exit 0 + isAuthenticated:false is NOT logged in"
        );

        const LOGGED_IN: &str = r#"{"status":"authenticated","isAuthenticated":true,"hasAccessToken":true,"hasRefreshToken":true,"message":"Logged in"}"#;
        assert!(cursor_status_verdict(LOGGED_IN));

        // Junk, an empty answer and a missing key all fail toward "log in".
        assert!(!cursor_status_verdict(""));
        assert!(!cursor_status_verdict("not json"));
        assert!(!cursor_status_verdict(r#"{"status":"authenticated"}"#));

        assert!(
            CURSOR_AUTH_ERROR_MSG.contains("agent login"),
            "the message must quote the login command verbatim: {CURSOR_AUTH_ERROR_MSG}"
        );
    }

    /// The verdict must never be derived from an exit status — pin the source, since
    /// no test here spawns a real child. Fragments assembled with `concat!` so the
    /// assertion cannot match itself.
    #[test]
    fn the_verdict_never_reads_an_exit_status() {
        let production = include_str!("auth.rs")
            .split("#[cfg(test)]")
            .next()
            .unwrap();
        for banned in [
            concat!("status", "().success()"),
            concat!("exit", "_code"),
            concat!("exit", "_status"),
        ] {
            assert!(
                !production.contains(banned),
                "`status` exits 0 while logged out (D8); found {banned}"
            );
        }
    }
}
