//! The vendor's model-id grammar: which ids an operator may pin, which billing
//! key each one costs out under, and the 404 the CLI answers an id it does not
//! serve with (ADR-0043 D8).
//!
//! The id set is enumerated from `packages/core/src/config/models.ts` @ v0.51.0
//! as read by `docs/research/gemini-cli-adapter-spike.md` §4 — the six `-m`
//! routing aliases plus the concrete turn-driving ids. Two exclusions are
//! deliberate: `gemini-3-pro-preview` is RETIRED (the CLI still ships the
//! constant but the backend answers 404, spike Trap 1), and
//! `gemini-embedding-001` can never be a phase model.

/// The ids `ralphy config set gemini.plan_model|gemini.exec_model` accepts.
///
/// Deliberately NOT applied to `--plan-model`/`--exec-model` at run time: the
/// vendor's `resolveModel()` passes unknown strings through and the constant set
/// is mutable by server-side experiment flags, so a stale local list must never
/// block an id the vendor started serving (ADR-0043 D8). The run-time signal is
/// [`unknown_model_stop`]'s 404.
pub const PINNABLE_MODELS: &[&str] = &[
    "auto",
    "pro",
    "flash",
    "flash-lite",
    "gemini-3.1-pro-preview",
    "gemini-3.1-pro-preview-customtools",
    "gemini-3.5-flash",
    "gemini-3-flash",
    "gemini-3-flash-preview",
    "gemini-3.1-flash-lite",
    "gemini-2.5-pro",
    "gemini-2.5-flash",
    "gemma-4-31b-it",
    "gemma-4-26b-a4b-it",
];

/// The routing aliases: ids that name a ROUTER, not an engine. What the router
/// picked is not knowable from the id, so they all fold onto one sentinel.
const ROUTING_ALIASES: &[&str] = &[
    "auto",
    "pro",
    "flash",
    "flash-lite",
    // Deprecated spellings the CLI still accepts but no longer offers.
    "auto-gemini-3",
    "auto-gemini-2.5",
];

/// The price key a routed run costs out under. Deliberately ABSENT from
/// `PriceTable::defaults`, so a routed run reports an unpriced model rather than
/// borrowing the rates of an engine it may never have used. `auto` is already a
/// Cursor row (grok-4.5 rates), which is exactly the misattribution this avoids.
const ROUTED_KEY: &str = "gemini-routed";

/// `true` when `id` is one of the ids this CLI version still serves.
pub fn is_pinnable_model(id: &str) -> bool {
    PINNABLE_MODELS.contains(&id.trim())
}

/// The price-table key a pinned id bills under — the seam
/// `ralphy_agent_cursor::model_family` occupies for its own vendor.
///
/// Two ids are renamed because the CLI's constant does not name the engine that
/// serves it: `gemini-3-flash` maps to the 3.5 backend
/// (`SECONDARY_GEMINI_3_5_FLASH_MODEL`, spike §4) — 3× the price of the
/// same-named *preview* Flash another vendor's catalogue carries — and the
/// retired `gemini-3-pro-preview` costs out as its successor so a historical run
/// record still prices.
pub fn price_key(model: &str) -> String {
    let id = model.trim();
    match id {
        _ if ROUTING_ALIASES.contains(&id) => ROUTED_KEY.to_string(),
        "gemini-3-flash" => "gemini-3.5-flash".to_string(),
        "gemini-3-pro-preview" => "gemini-3.1-pro-preview".to_string(),
        other => other.to_string(),
    }
}

/// The vendor's error class for an id it does not serve.
const NOT_FOUND: &str = "ModelNotFoundError";

/// `Some(err)` when `log` carries the vendor's model-not-found refusal: the run
/// did not fail, it was REFUSED, and the operator can fix it by editing one flag.
///
/// `log` is stdout+stderr COMBINED, and on a working run stdout carries the whole
/// transcript — so the class is only recognized at the START of a line: stdout is
/// stream-json, where every line begins with `{`, while the vendor writes the
/// error bare on stderr. A transcript quoting the phrase can never trip this.
pub(crate) fn unknown_model_stop(log: &str, requested: Option<&str>) -> Option<anyhow::Error> {
    let pinned = requested.unwrap_or(crate::DEFAULT_MODEL);
    let line = log
        .lines()
        .map(str::trim)
        .find(|l| l.starts_with(NOT_FOUND))?;
    Some(anyhow::anyhow!(
        "gemini refused the model `{pinned}`: {line}"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn price_key_renames_the_two_ids_whose_constant_lies() {
        // The CLI's `gemini-3-flash` is served by the 3.5 backend; the identically
        // named Cursor row is Google's *preview* Flash at a third of the price.
        assert_eq!(price_key("gemini-3-flash"), "gemini-3.5-flash");
        // Retired for pinning, still priced — as its successor (spike Trap 1).
        assert_eq!(price_key("gemini-3-pro-preview"), "gemini-3.1-pro-preview");
    }

    #[test]
    fn every_routing_alias_folds_onto_the_unpriced_sentinel() {
        for alias in [
            "auto",
            "pro",
            "flash",
            "flash-lite",
            "auto-gemini-3",
            "auto-gemini-2.5",
        ] {
            assert_eq!(price_key(alias), "gemini-routed", "{alias} must not price");
        }
        assert_eq!(price_key("  auto  "), "gemini-routed", "trimmed first");
    }

    #[test]
    fn a_concrete_id_passes_through_verbatim() {
        assert_eq!(
            price_key("gemini-3.1-pro-preview-customtools"),
            "gemini-3.1-pro-preview-customtools"
        );
        assert_eq!(price_key("gemini-2.5-pro"), "gemini-2.5-pro");
    }

    /// The retired id is priceable but not choosable.
    #[test]
    fn the_retired_pro_preview_is_not_offered() {
        assert!(!PINNABLE_MODELS.contains(&"gemini-3-pro-preview"));
        assert!(!is_pinnable_model("gemini-3-pro-preview"));
        assert!(is_pinnable_model("gemini-3.1-pro-preview"));
        assert!(is_pinnable_model(" gemini-3.5-flash "));
        assert!(!is_pinnable_model("gemini-embedding-001"));
    }

    #[test]
    fn a_bare_404_line_names_the_requested_model() {
        const ERR: &str = "ModelNotFoundError: models/no-such-model is not found for API version \
                           v1beta, or is not supported for generateContent. { code: 404 }";
        let err = unknown_model_stop(ERR, Some("no-such-model")).expect("a 404 is a named stop");
        let text = err.to_string();
        assert!(text.contains("no-such-model"), "{text}");
        assert!(text.contains("ModelNotFoundError"), "{text}");
    }

    #[test]
    fn a_transcript_quoting_the_error_is_not_a_stop() {
        let quoted = r#"{"type":"assistant","text":"a ModelNotFoundError means the id is wrong"}"#;
        assert!(unknown_model_stop(quoted, Some("gemini-2.5-pro")).is_none());
        assert!(unknown_model_stop(r#"{"type":"result"}"#, None).is_none());
    }

    #[test]
    fn an_unpinned_run_is_named_by_the_routed_default() {
        let err = unknown_model_stop("ModelNotFoundError: nope { code: 404 }", None)
            .expect("a 404 is a named stop");
        assert!(err.to_string().contains("`auto`"), "{err}");
    }
}
