//! The vendor's model-id grammar: normalizing a pinned id to its billing family,
//! and turning the two refusals the vendor answers a bad `--model` with into an
//! actionable stop instead of a mute failure (ADR-0042 D4/D5).
//!
//! An id the CLI accepts is a family plus optional decorations — a `-thinking`
//! marker, an effort segment, a `-fast` speed suffix, or a `[…]` override
//! expression. The vendor spells them in BOTH orders (`claude-opus-4-8-thinking-max`
//! and `claude-4.6-sonnet-medium-thinking` are both in its catalogue), so the
//! decorations are stripped in a fixpoint loop rather than a fixed sequence.

/// Decorations stripped off the tail of an id, longest-first so `-extra-high` is
/// not read as a bare `-high`. Sourced from the catalogue the vendor prints when
/// it rejects an id (`fixtures/model-invalid-2026-07-21.err`).
const DECORATIONS: &[&str] = &[
    "-extra-high",
    "-thinking",
    "-medium",
    "-xhigh",
    "-none",
    "-fast",
    "-high",
    "-low",
    "-max",
];

/// The billing family of a pinned model id: the price-table key.
///
/// `composer-2.5-fast` → `composer-2.5` (the id the vendor itself persists as
/// `modelId`), `claude-opus-4-8[context=1m,effort=high]` → `claude-opus-4-8`,
/// `auto` → `auto`.
pub fn model_family(id: &str) -> String {
    let mut s = id.split('[').next().unwrap_or(id).trim();
    loop {
        match DECORATIONS.iter().find_map(|d| s.strip_suffix(*d)) {
            // A decoration that consumed the whole id was the family's own name.
            Some(shorter) if !shorter.is_empty() => s = shorter,
            _ => return s.to_string(),
        }
    }
}

/// Cursor's own family — the one an unentitled plan may still name, which the
/// vendor's `Named models unavailable` sentence claims otherwise (ADR-0042 D4).
pub(crate) fn is_first_party(family: &str) -> bool {
    family.starts_with("composer")
}

/// The vendor's two `--model` refusals, verbatim. Both are answered on stderr
/// before any paid call, and both leave a run with zero records and exit 1 — the
/// same shape as a truncation, which is why they must be recognized by text.
const REFUSALS: &[&str] = &["Cannot use this model:", "Named models unavailable"];

/// `Some(err)` when `log` carries a `--model` refusal: the run did not fail, it was
/// REFUSED, and the operator can fix it by editing one flag.
pub(crate) fn model_refusal_stop(log: &str, requested: Option<&str>) -> Option<anyhow::Error> {
    let pinned = requested.unwrap_or(crate::command::AUTO_MODEL);
    let (line, marker) = log.lines().find_map(|l| {
        REFUSALS
            .iter()
            .find(|m| l.contains(**m))
            .map(|m| (l.trim(), *m))
    })?;
    let mut msg = format!("cursor refused the pinned model `{pinned}`: {line}");
    if marker == REFUSALS[1] {
        let family = model_family(pinned);
        msg.push_str(
            "\nnote: a Free plan CAN name the first-party `composer-*` family; \
             only third-party ids are refused",
        );
        if is_first_party(&family) {
            msg.push_str(&format!(
                " — `{family}` is first-party, so this refusal is unexpected"
            ));
        }
    }
    Some(anyhow::anyhow!(msg))
}

#[cfg(test)]
mod tests {
    use super::*;

    const INVALID: &str = include_str!("../fixtures/model-invalid-2026-07-21.err");
    const ENTITLEMENT: &str = include_str!("../fixtures/model-entitlement-2026-07-21.err");

    #[test]
    fn family_strips_effort_speed_thinking_and_bracket() {
        assert_eq!(
            model_family("claude-opus-4-8-thinking-max"),
            "claude-opus-4-8"
        );
        assert_eq!(model_family("composer-2.5-fast"), "composer-2.5");
        assert_eq!(model_family("gpt-5.6-sol-max"), "gpt-5.6-sol");
        assert_eq!(
            model_family("claude-opus-4-8[context=1m,effort=high,fast=false]"),
            "claude-opus-4-8"
        );
        assert_eq!(model_family("auto"), "auto");
        assert_eq!(model_family("gpt-5.4-nano-low"), "gpt-5.4-nano");
        // `-flash` is part of the family name, not the `-fast` speed suffix.
        assert_eq!(model_family("gemini-3-flash"), "gemini-3-flash");
        // The vendor spells the decorations in both orders.
        assert_eq!(
            model_family("claude-4.6-sonnet-medium-thinking"),
            "claude-4.6-sonnet"
        );
        assert_eq!(model_family("gpt-5.5-extra-high-fast"), "gpt-5.5");
    }

    #[test]
    fn every_catalogued_id_normalizes_to_a_family_the_catalogue_also_spells() {
        // The whole live catalogue folds onto a small set of families, and no
        // decoration eats a family name down to nothing.
        let ids: Vec<&str> = INVALID
            .split("Available models:")
            .nth(1)
            .expect("the refusal carries the catalogue")
            .split(',')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .collect();
        assert!(ids.len() > 100, "catalogue looked short: {}", ids.len());
        for id in ids {
            let family = model_family(id);
            assert!(!family.is_empty(), "{id} normalized to nothing");
            assert!(
                id.starts_with(&family),
                "{id} normalized to an unrelated {family}"
            );
        }
    }

    #[test]
    fn first_party_is_the_composer_family_only() {
        assert!(is_first_party("composer-2.5"));
        assert!(!is_first_party("cursor-grok-4.5"));
        assert!(!is_first_party("claude-opus-4-8"));
    }

    #[test]
    fn no_default_model_id_is_baked_in() {
        // The "optional value, no hardcoded default" criterion: no behavioural test
        // can catch a literal id smuggled into the production path.
        for (name, src) in [
            ("lib.rs", include_str!("lib.rs")),
            ("command.rs", include_str!("command.rs")),
        ] {
            let production = src.split("#[cfg(test)]").next().unwrap_or(src);
            assert!(
                !production.contains("composer-2.5"),
                "{name} hardcodes a model id outside its tests"
            );
        }
    }

    #[test]
    fn an_entitlement_refusal_is_an_actionable_stop() {
        let e = model_refusal_stop(ENTITLEMENT, Some("claude-opus-4-8-thinking-max"))
            .expect("the entitlement refusal must stop the run");
        let text = format!("{e}");
        assert!(text.contains("claude-opus-4-8-thinking-max"), "{text}");
        assert!(text.contains("Named models unavailable"), "{text}");
        assert!(text.contains("Free plans can only use Auto"), "{text}");
        assert!(text.contains("composer-*"), "{text}");
    }

    #[test]
    fn an_invalid_model_id_surfaces_the_whole_catalogue() {
        let e = model_refusal_stop(INVALID, Some("definitely-not-a-real-model"))
            .expect("an invalid id must stop the run");
        let text = format!("{e}");
        assert!(text.contains("definitely-not-a-real-model"), "{text}");
        assert!(text.contains("Cannot use this model:"), "{text}");
        assert!(text.contains("Available models:"), "{text}");
    }

    #[test]
    fn an_unpinned_refusal_names_auto() {
        let e = model_refusal_stop(ENTITLEMENT, None).expect("still a refusal");
        assert!(format!("{e}").contains("`auto`"));
    }

    #[test]
    fn an_ordinary_log_is_not_a_refusal() {
        assert!(model_refusal_stop("{\"type\":\"result\"}\n", Some("composer-2.5")).is_none());
    }
}
