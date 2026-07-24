//! Kimi authentication detection: the signals recovered from headless `kimi`
//! output that the process exit code alone can't distinguish from a generic
//! failure. A logged-out 0.28 session has **two** shapes (ADR-0028 D6): an
//! expired/invalid token with the config still intact prints `auth.login_required`;
//! a full `kimi logout` strips the login-populated model catalog from
//! `config.toml`, so the pinned `-m kimi-code/k3` then fails `config.invalid …
//! is not configured`, and a bare invocation fails `No model configured … /login`.

/// The actionable message surfaced when a run hits a Kimi authentication failure
/// (no active OAuth session). Stops a logged-out infinite plan-retry.
pub(crate) const KIMI_AUTH_ERROR_MSG: &str =
    "Kimi is not authenticated (no active login) — run `kimi login` and retry";

/// Return `true` when `text` shows a Kimi authentication failure. A logged-out
/// `kimi` fails in one of three shapes, all handled here (ADR-0028 D6):
///
/// - `auth.login_required:` — an expired/invalid token with the model catalog
///   still intact; a second line names the OAuth provider, so matching the
///   error-type token alone survives the provider name and the line wrap.
/// - `No model configured … /login` — a full `kimi logout` (catalog stripped)
///   invoked without `-m`; the CLI itself points at `/login`, so this is an
///   unambiguous logged-out signal.
/// - `config.invalid … is not configured` — the same full-logout state on the
///   adapter's real path, where `-m kimi-code/k3` is pinned (`command.rs`).
///
/// The last group carries a small conflation risk: a genuine operator
/// model-config typo would also be reported as "run `kimi login`". Accepted on
/// purpose — login populates the catalog and the adapter always pins the managed
/// `kimi-code/k3`, so "model not configured" almost always *means* logged-out,
/// and re-login is the right first action either way (ADR-0028 D6). Without any
/// of these the failure masquerades as a generic "no plan" (planning) or
/// `Outcome::Stuck` (execution).
pub(crate) fn is_kimi_auth_error(text: &str) -> bool {
    ralphy_adapter_support::auth_error(
        text,
        &[
            &["auth.login_required"],
            &["no model configured", "login"],
            &["config.invalid", "is not configured"],
        ],
    )
}

/// Return `true` when `text` shows a Kimi API-level usage-limit failure. When the
/// billing-cycle quota is exhausted, headless `kimi` gets an HTTP 403 and exits
/// non-zero *without* the exit-75 chat-level sentinel (ADR-0028 D9) and without a
/// `RALPHY_DONE_EXIT`, so — absent this marker — a genuine limit is misread as
/// `Outcome::Stuck`.
///
/// Two 403 body shapes are matched, both observed live:
/// - the older JSON error *type* `access_terminated_error` (kept for back-compat);
/// - the `kimi-code` 0.28 shape `provider.api_error: 403 … usage limit for this
///   billing cycle …` (captured live in the #274 capstone) — this one carries **no**
///   `access_terminated_error` token, so the old matcher alone silently misses every
///   real 0.28 ceiling.
///
/// Matching the 0.28 prose "usage limit for this billing cycle" is safe from a task
/// merely echoing the phrase: `detect_limit` trusts this scan only on a **non-clean
/// exit**, and a task that quotes the words does not fail the process (ADR-0028 D9,
/// mirroring Codex's non-clean-exit guard).
pub(crate) fn is_kimi_limit_text(text: &str) -> bool {
    // The Kimi CLI hard-wraps its 403 body to the terminal width, so in the captured
    // log a marker token can be split mid-word by a newline (observed live:
    // `access_\nterminated_error`). Strip line breaks before matching so the wrap
    // position can't hide the signal.
    let unwrapped: String = text.chars().filter(|c| *c != '\n' && *c != '\r').collect();
    let lower = unwrapped.to_ascii_lowercase();
    lower.contains("access_terminated_error")
        || lower.contains("usage limit for this billing cycle")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_kimi_auth_error_matches_login_required() {
        // The verbatim 0.28 logged-out message, captured live on this host with
        // KIMI_CODE_HOME pointed at a temp dir holding config.toml but no
        // `credentials` (ADR-0028 D6): an expired/invalid token, catalog intact.
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
    fn is_kimi_auth_error_matches_full_logout_signatures() {
        // A full `kimi logout` strips the login-populated model catalog from
        // config.toml, so the two logged-out shapes below carry NO
        // `auth.login_required` token. Both captured live (kimi-code 0.28.0,
        // Windows) in the #274 capstone, Phase 0 (issue #281, ADR-0028 D6).

        // The adapter's real path: `-m kimi-code/k3` is pinned (command.rs), so a
        // stripped catalog fails config.invalid / is not configured.
        let with_model = "error: failed to run prompt: config.invalid: Model \
             \"kimi-code/k3\" is not configured in config.toml. Add a \
             [models.\"kimi-code/k3\"] entry with max_context_size.";
        assert!(is_kimi_auth_error(with_model));

        // A bare invocation (no `-m`): the CLI itself points at /login.
        let no_model = "error: failed to run prompt: No model configured. Run `kimi` \
             and use /login to sign in, then retry; or set default_model in config.toml.";
        assert!(is_kimi_auth_error(no_model));

        // Neither is confused with a usage limit or a clean run.
        assert!(!is_kimi_limit_text(with_model));
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
    fn is_kimi_limit_text_matches_0_28_provider_api_error() {
        // The verbatim kimi-code 0.28 billing-cycle 403, captured live in the #274
        // capstone. It carries NO `access_terminated_error` token — the old matcher
        // alone silently misses it, misclassifying a real ceiling as Stuck / "no plan".
        let live = "error: failed to run prompt: provider.api_error: 403 You've reached \
            your usage limit for this billing cycle. Your quota will be refreshed in the \
            next cycle. To continue now, purchase extra usage or upgrade your plan: \
            https://www.kimi.com/code/#pricing";
        assert!(is_kimi_limit_text(live));
        // Still not confused with an auth failure or a clean run.
        assert!(!is_kimi_auth_error(live));
        assert!(!is_kimi_limit_text("all green\nRALPHY_DONE_EXIT\n"));
    }

    #[test]
    fn plan_time_detector_maps_real_0_28_ceiling_to_reset_none_limit() {
        // The plan-path closure in `lib.rs` composes exactly this: the (now
        // 0.28-correct) matcher through `detect_limit` with no reset hint. A
        // billing-cycle ceiling during planning writes no plan, so this is where it
        // bites (#282 defect #3) — it must classify as a limit carrying `None`
        // (drives the ADR-0030 synthetic cadence), never "kimi produced no plan".
        let plan_log = "planning cmd=kimi model=kimi-code/k3\n\
            error: failed to run prompt: provider.api_error: 403 You've reached your \
            usage limit for this billing cycle. Your quota will be refreshed in the \
            next cycle. To continue now, purchase extra usage or upgrade your plan: \
            https://www.kimi.com/code/#pricing";
        assert_eq!(
            ralphy_adapter_support::detect_limit(plan_log, is_kimi_limit_text, |_| None),
            Some(None),
            "the real 0.28 plan-time ceiling must be a reset-less limit, not a no-plan"
        );
        // A clean planning log with no 403 is not a limit (guards against a false
        // positive that would route every plan through the cadence).
        assert_eq!(
            ralphy_adapter_support::detect_limit(
                "planning cmd=kimi model=kimi-code/k3\nall green\nRALPHY_DONE_EXIT",
                is_kimi_limit_text,
                |_| None
            ),
            None
        );
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
