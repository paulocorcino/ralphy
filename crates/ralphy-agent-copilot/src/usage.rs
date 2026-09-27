//! Copilot token-usage capture (ADR-0041 D10): the run's own minted
//! `--session-id` selects the rows Copilot wrote into its `session-store.db`.
//!
//! Unlike OpenCode's, this correlation needs no stream parsing — Ralphy mints the
//! session id (`command::mint_session_id`) and hands it to the CLI, so the key is
//! known before the child starts. The stream's `result.usage.premiumRequests` is
//! an AI-CREDIT figure, not tokens, and is never read: the two currencies must not
//! be mixed (ADR-0041 D10). The store is the only token source.
//!
//! The WAL-safe copy and the SQL live once, in
//! `ralphy_usage_scan::copilot` — this module only resolves the path and maps
//! [`Tokens`] onto [`Usage`].

use std::path::{Path, PathBuf};

use ralphy_core::Usage;
use ralphy_usage_scan::Tokens;

/// `$COPILOT_HOME/session-store.db`, else `<home>/.copilot/session-store.db`
/// (`USERPROFILE` on Windows, `HOME` elsewhere). `None` when no home is known.
pub(crate) fn copilot_store_db() -> Option<PathBuf> {
    ralphy_adapter_support::home_scoped_path(
        std::env::var_os("COPILOT_HOME"),
        Path::new(".copilot"),
        Path::new("session-store.db"),
    )
}

/// Map a session's summed store [`Tokens`] + last-seen model onto the normalized
/// [`Usage`] (ADR-0041 D10): `input→input`, `output→output`,
/// `cache_read→cache_read`, `cache_creation→cache_creation`, `model→model`.
/// `reasoning_tokens` never reaches here — the reader does not select it.
fn usage_from(tokens: Tokens, model: Option<String>) -> Usage {
    Usage {
        input: tokens.input,
        output: tokens.output,
        cache_read: tokens.cache_read,
        cache_creation: tokens.cache_creation,
        model,
    }
}

/// The token usage of `session_id` as Copilot recorded it. Best-effort:
/// `Usage::default()` when no home resolves or the store is unavailable, so token
/// capture never fails a run.
pub(crate) fn copilot_usage(session_id: &str) -> Usage {
    let Some(db) = copilot_store_db() else {
        return Usage::default();
    };
    let (tokens, model) = ralphy_usage_scan::session_tokens(&db, session_id);
    usage_from(tokens, model)
}

/// The reasoning effort Copilot RECORDED for `session_id`, or `None` when no home
/// resolves, the store is unavailable, or the vendor wrote nothing. Best-effort by
/// the same contract as [`copilot_usage`].
pub(crate) fn copilot_recorded_effort(session_id: &str) -> Option<String> {
    ralphy_usage_scan::session_reasoning_effort(&copilot_store_db()?, session_id)
}

/// The post-hoc verification of the effort clamp (ADR-0041 D5a): the REQUEST is
/// not the truth, the vendor's own record is. `Some(message)` only when both are
/// known and they differ — an absent record proves nothing, and an equal pair is
/// the expected case. Purely a `warn!` payload: never fails a run.
pub(crate) fn effort_mismatch(requested: Option<&str>, recorded: Option<&str>) -> Option<String> {
    let (r, v) = (requested?, recorded?);
    (r != v).then(|| format!("requested effort {r}, but the vendor recorded {v}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effort_mismatch_names_both_levels() {
        assert_eq!(
            effort_mismatch(Some("high"), Some("medium")).as_deref(),
            Some("requested effort high, but the vendor recorded medium")
        );
        assert_eq!(effort_mismatch(Some("high"), Some("high")), None);
        assert_eq!(effort_mismatch(Some("high"), None), None);
        assert_eq!(effort_mismatch(None, Some("high")), None);
        assert_eq!(effort_mismatch(None, None), None);
    }
}
