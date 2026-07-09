//! Kimi authentication detection: the one signal recovered from `kimi --print`
//! output that the process exit code alone can't distinguish from a generic
//! failure — a logged-out session prints `LLM not set` (ADR-0028 D6).

/// The actionable message surfaced when a run hits a Kimi authentication failure
/// (no active OAuth session). Stops a logged-out infinite plan-retry.
pub(crate) const KIMI_AUTH_ERROR_MSG: &str =
    "Kimi is not authenticated (LLM not set) — run `kimi login` and retry";

/// Return `true` when `text` shows a Kimi authentication failure. A logged-out
/// `kimi --print` prints `LLM not set` to stdout; without this the failure
/// masquerades as a generic "no plan" (planning) or `Outcome::Stuck` (execution).
pub(crate) fn is_kimi_auth_error(text: &str) -> bool {
    ralphy_adapter_support::auth_error(text, &[&["llm not set"]])
}

/// Return `true` when `text` shows a Kimi API-level usage-limit failure. When the
/// billing-cycle quota is exhausted, `kimi --print` gets an HTTP 403 whose body
/// carries `access_terminated_error`; the CLI writes that line to the log and exits
/// non-zero *without* the exit-75 chat-level sentinel (ADR-0028 D9) and without a
/// `RALPHY_DONE_EXIT`, so — absent this marker — a genuine limit is misread as
/// `Outcome::Stuck`. The marker is deliberately the distinctive error *type*, not
/// the prose "usage limit", to avoid matching a task that merely echoed the phrase.
pub(crate) fn is_kimi_limit_text(text: &str) -> bool {
    text.to_ascii_lowercase()
        .contains("access_terminated_error")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_kimi_auth_error_matches_llm_not_set() {
        assert!(is_kimi_auth_error("Error: LLM not set"));
        // Case-insensitive.
        assert!(is_kimi_auth_error("llm not set"));
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
}
