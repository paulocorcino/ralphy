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
}
