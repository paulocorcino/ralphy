//! Kimi authentication detection: the one signal recovered from headless `kimi`
//! output that the process exit code alone can't distinguish from a generic
//! failure — a logged-out 0.28 session prints `auth.login_required` (ADR-0028 D6).

/// The actionable message surfaced when a run hits a Kimi authentication failure
/// (no active OAuth session). Stops a logged-out infinite plan-retry.
pub(crate) const KIMI_AUTH_ERROR_MSG: &str =
    "Kimi is not authenticated (auth.login_required) — run `kimi login` and retry";

/// Return `true` when `text` shows a Kimi authentication failure. A logged-out
/// `kimi -p` fails with `error: failed to run prompt: auth.login_required:` and a
/// second line naming the OAuth provider (ADR-0028 D6); matching the error-type
/// token alone survives the provider name and the line wrap. Without this the
/// failure masquerades as a generic "no plan" (planning) or `Outcome::Stuck`
/// (execution).
pub(crate) fn is_kimi_auth_error(text: &str) -> bool {
    ralphy_adapter_support::auth_error(text, &[&["auth.login_required"]])
}

/// Return `true` when `text` shows a Kimi API-level usage-limit failure. When the
/// billing-cycle quota is exhausted, headless `kimi` gets an HTTP 403 whose body
/// carries `access_terminated_error`; the CLI writes that line to the log and exits
/// non-zero *without* the exit-75 chat-level sentinel (ADR-0028 D9) and without a
/// `RALPHY_DONE_EXIT`, so — absent this marker — a genuine limit is misread as
/// `Outcome::Stuck`. The marker is deliberately the distinctive error *type*, not
/// the prose "usage limit", to avoid matching a task that merely echoed the phrase.
pub(crate) fn is_kimi_limit_text(text: &str) -> bool {
    // The Kimi CLI hard-wraps its 403 body to the terminal width, so in the captured
    // log the marker token is split mid-word by a newline (observed live:
    // `access_\nterminated_error`). Strip line breaks before matching so the wrap
    // position can't hide the signal.
    let unwrapped: String = text.chars().filter(|c| *c != '\n' && *c != '\r').collect();
    unwrapped
        .to_ascii_lowercase()
        .contains("access_terminated_error")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_kimi_auth_error_matches_login_required() {
        // The verbatim 0.28 logged-out message, captured live on this host with
        // KIMI_CODE_HOME pointed at a temp dir holding config.toml but no
        // `credentials` (ADR-0028 D6).
        let live = "error: failed to run prompt: auth.login_required: OAuth provider \
             \"managed:kimi-code\" requires login before it can be used.";
        assert!(is_kimi_auth_error(live));
        // The same signal survives the CLI's line wrap of the same message.
        assert!(is_kimi_auth_error(
            "error: failed to run prompt: auth.login_required:\n\
             OAuth provider \"managed:kimi-code\" requires login before it can be used."
        ));
        // The pre-0.28 (1.48) signal is NOT the 0.28 one.
        assert!(!is_kimi_auth_error("Error: LLM not set"));
        // A clean run is not an auth error.
        assert!(!is_kimi_auth_error("all green\nRALPHY_DONE_EXIT\n"));
    }

    #[test]
    fn is_kimi_limit_text_matches_403_access_terminated() {
        // The live 403 body from an exhausted billing-cycle quota.
        let log = "Error code: 403 - {'error': {'message': \"You've reached your \
            usage limit for this billing cycle.\", 'type': 'access_terminated_error'}}";
        assert!(is_kimi_limit_text(log));
        // A usage limit is not an auth error, and a clean run is neither.
        assert!(!is_kimi_auth_error(log));
        assert!(!is_kimi_limit_text("all green\nRALPHY_DONE_EXIT\n"));
    }

    #[test]
    fn is_kimi_limit_text_matches_terminal_wrapped_marker() {
        // The Kimi CLI hard-wraps the 403 body to terminal width, splitting the
        // marker token across a newline in the captured log — the exact live form
        // that slipped past a naive substring match.
        let wrapped = "get more: https://www.kimi.com/...quota-upgrade\", 'type': 'access_\n\
             terminated_error'}}\n\nTo resume this session: kimi -r 0f5c0ded";
        assert!(is_kimi_limit_text(wrapped));
        // Also tolerate a CRLF wrap.
        assert!(is_kimi_limit_text("'type': 'access_\r\nterminated_error'"));
    }
}
