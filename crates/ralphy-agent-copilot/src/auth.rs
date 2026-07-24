//! Copilot authentication and usage-limit detection: the two signals recovered
//! from the CLI's output that the process exit code alone can't distinguish from
//! a generic failure (ADR-0041 D3/D11).

/// The actionable message surfaced when a run hits a Copilot authentication
/// failure (no OAuth session and no token). Stops a logged-out infinite
/// plan-retry.
pub(crate) const COPILOT_AUTH_ERROR_MSG: &str =
    "Copilot is not authenticated (no authentication information found) — run `copilot login` and retry";

/// Return `true` when `text` shows a Copilot authentication failure. A logged-out
/// `copilot` exits 1 having printed `Error: No authentication information found.`
/// to **stderr** (spike §5); without this the failure masquerades as a generic
/// "no plan" (planning) or `Outcome::Stuck` (execution).
pub(crate) fn is_copilot_auth_error(text: &str) -> bool {
    ralphy_adapter_support::auth_error(text, &[&["no authentication information found"]])
}

/// Return `true` when `text` shows a Copilot usage-limit failure.
///
/// `(indicative — refine against a captured limit, ADR-0041 D11)`: no exhausted
/// limit was ever induced in the spike (§7 is entirely unobserved), and Copilot
/// bills in **AI credits** rather than tokens, so the exact wording is unknown.
/// The predicate therefore matches a limit *class* (ADR-0040 C7) rather than one
/// provider's phrasing, and is trusted only on a NON-CLEAN exit — a clean exit is
/// itself proof the phrase was merely echoed by the agent's own prose.
///
/// Every alternative is ERROR-SHAPED (`… exceeded`, `… reached`, `429 …`) rather
/// than a bare topic word. The scan runs over the whole captured log, which carries
/// the echoed charter, the issue body and every tool's output — a bare `rate limit`
/// would fire on an issue that merely *discusses* rate limiting and park the queue
/// for hours on the ADR-0030 cadence.
pub(crate) fn is_copilot_limit_text(text: &str) -> bool {
    let l = text.to_ascii_lowercase();
    l.contains("rate limit exceeded")
        || l.contains("rate limit reached")
        || l.contains("quota exceeded")
        || l.contains("429 too many requests")
        || l.contains("usage limit for")
        || l.contains("usage limit reached")
        || l.contains("out of ai credits")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The literal logged-out stderr block from the spike (§5).
    const LOGGED_OUT: &str = "Error: No authentication information found.\n\
        Copilot can be authenticated with GitHub using an OAuth Token or a Fine-Grained\n\
        Personal Access Token.\n\
        To authenticate, you can use any of the following methods:\n\
          • Start 'copilot' and run the '/login' command\n\
          • Set the COPILOT_GITHUB_TOKEN, GH_TOKEN, or GITHUB_TOKEN environment variable\n\
          • Run 'gh auth login' to authenticate with the GitHub CLI";

    #[test]
    fn is_copilot_auth_error_matches_the_logged_out_block() {
        assert!(is_copilot_auth_error(LOGGED_OUT));
        // Case-insensitive.
        assert!(is_copilot_auth_error("no authentication information found"));
        // Auth is not a limit: the two predicates must not overlap, or a
        // logged-out run would be retried forever as a "wait for reset".
        assert!(!is_copilot_limit_text(LOGGED_OUT));
    }

    #[test]
    fn a_clean_run_matches_neither_predicate() {
        assert!(!is_copilot_auth_error("all green\nRALPHY_DONE_EXIT"));
        assert!(!is_copilot_limit_text("all green\nRALPHY_DONE_EXIT"));
    }

    #[test]
    fn is_copilot_limit_text_matches_the_class() {
        for phrase in [
            "API rate limit exceeded for this token",
            "Your quota exceeded for the current period",
            "429 Too Many Requests",
            "You've reached your usage limit for this billing cycle",
            "You have run out of AI credits",
        ] {
            assert!(is_copilot_limit_text(phrase), "should match: {phrase}");
            assert!(
                !is_copilot_auth_error(phrase),
                "not an auth error: {phrase}"
            );
        }
    }

    /// The predicate scans the WHOLE captured log, which echoes the charter, the
    /// issue body and every tool's output. Prose that merely names the topic must
    /// not fire — a false positive parks the queue on the ADR-0030 cadence.
    #[test]
    fn is_copilot_limit_text_ignores_prose_about_limits() {
        for prose in [
            "the issue asks us to handle a rate limit gracefully",
            "TODO: retry when the API returns too many requests",
            "docs: document the usage limit behaviour",
            "fn ai_credits_remaining() -> u32",
        ] {
            assert!(!is_copilot_limit_text(prose), "should NOT match: {prose}");
        }
    }
}
