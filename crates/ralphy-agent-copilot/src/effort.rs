//! The reasoning-effort clamp (ADR-0041 D5/D5a).
//!
//! Copilot's effort vocabulary is PER MODEL: the catalog publishes each model's
//! own `capabilities.supports.reasoning_effort` list, and a level outside it is
//! rejected by the vendor. So an operator's request is never sent verbatim — it
//! is clamped DOWN to the greatest level the chosen model actually supports, and
//! omitted entirely when the model takes no effort argument at all.
//!
//! Scope: this is a clamp, not a vocabulary. [`EFFORT_ORDER`] stays inside this
//! adapter — promoting effort to a core Ralphy knob is #227's decision, pinned by
//! `clamp_lives_only_in_the_copilot_adapter`.

use crate::catalog::CopilotCatalog;

/// Every effort level Copilot has been observed to publish, ordered from least to
/// most reasoning. The ONLY ordering in this crate; the support table itself is
/// always the vendor's (see `no_hardcoded_effort_table`).
pub(crate) const EFFORT_ORDER: [&str; 7] =
    ["none", "minimal", "low", "medium", "high", "xhigh", "max"];

/// Position of `level` in [`EFFORT_ORDER`]; `None` for a level the ordering does
/// not know (a typo, or a future vendor level).
fn rank(level: &str) -> Option<usize> {
    EFFORT_ORDER.iter().position(|l| *l == level)
}

/// Is `level` a reasoning-effort level Copilot's vocabulary knows at all?
///
/// The validator `ralphy config set` uses, so a typo is refused at the keyboard
/// instead of persisting as a setting that silently does nothing at run time. It
/// answers membership only — whether a given MODEL accepts the level is the
/// catalog's business, and unknowable at `config set` time.
pub fn is_known_effort(level: &str) -> bool {
    rank(level).is_some()
}

/// Clamp `requested` into `supported` (ADR-0041 D5a): the greatest supported level
/// at or below the request, falling back to the lowest supported level when the
/// request sits below the model's floor.
///
/// `None` whenever no value can be sent safely: the model takes no effort argument
/// (`supported` is `None` or empty), or the request is unrankable — an unrankable
/// string cannot be clamped, and omitting the flag degrades to the model's own
/// default instead of failing the run pre-flight.
pub(crate) fn clamp_effort(requested: &str, supported: Option<&[String]>) -> Option<String> {
    let supported = supported.filter(|s| !s.is_empty())?;
    let want = rank(requested)?;
    let ranked = || supported.iter().filter_map(|s| rank(s).map(|r| (r, s)));
    ranked()
        .filter(|(r, _)| *r <= want)
        .max_by_key(|(r, _)| *r)
        .or_else(|| ranked().min_by_key(|(r, _)| *r))
        .map(|(_, s)| s.clone())
}

/// Resolve the `--effort` value for one phase, or `None` to omit the flag.
///
/// The effective model is the phase's pinned `--model`, else the account default
/// the catalog reported. Without a support list the adapter cannot know whether
/// the flag is even accepted, so an unknown model or an unavailable catalog omits
/// it — the safe direction.
pub(crate) fn resolve_effort(
    requested: Option<&str>,
    model: Option<&str>,
    catalog: Option<&CopilotCatalog>,
) -> Option<String> {
    let requested = requested?;
    let Some(catalog) = catalog else {
        tracing::warn!(
            requested,
            "no Copilot catalog: omitting --effort for this phase"
        );
        return None;
    };
    let effective = model.or(catalog.default_model.as_deref())?;
    let Some(entry) = catalog.get(effective) else {
        tracing::warn!(
            requested,
            model = effective,
            "model absent from the Copilot catalog: omitting --effort"
        );
        return None;
    };
    let supported = entry.reasoning_effort.as_deref();
    let clamped = clamp_effort(requested, supported);
    match clamped.as_deref() {
        Some(level) if level == requested => {}
        Some(level) => tracing::warn!(
            requested,
            model = effective,
            clamped = level,
            "Copilot effort clamped to what the model supports"
        ),
        // The two omission cases are distinct faults and must not share a message:
        // one is the operator's typo, the other is the model's nature.
        None if supported.is_none_or(<[String]>::is_empty) => tracing::warn!(
            requested,
            model = effective,
            "this model takes no reasoning-effort argument: omitting --effort"
        ),
        None => tracing::warn!(
            requested,
            model = effective,
            "unknown effort level: omitting --effort"
        ),
    }
    clamped
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::parse_catalog;

    fn fixture() -> CopilotCatalog {
        parse_catalog(
            include_str!("../fixtures/capi-models-2026-07-20.log"),
            "probe-1",
        )
        .expect("the fixture parses")
    }

    fn supported(cat: &CopilotCatalog, id: &str) -> Vec<String> {
        cat.get(id)
            .unwrap_or_else(|| panic!("{id} present"))
            .reasoning_effort
            .clone()
            .unwrap_or_else(|| panic!("{id} publishes an effort list"))
    }

    #[test]
    fn clamps_xhigh_to_high_on_gpt_5_mini() {
        let cat = fixture();
        let list = supported(&cat, "gpt-5-mini");
        assert_eq!(list, ["low", "medium", "high"].map(String::from));
        assert_eq!(
            clamp_effort("xhigh", Some(&list)),
            Some("high".to_string()),
            "a request above the ceiling clamps down"
        );
        assert_eq!(
            clamp_effort("minimal", Some(&list)),
            Some("low".to_string()),
            "a request below the floor takes the lowest supported level"
        );
    }

    /// The clamp walks the ORDERING down, never up: `xhigh` on a model whose list
    /// jumps straight from `high` to `max` must not buy `max`.
    #[test]
    fn sonnet_4_6_degrades_xhigh_to_high() {
        let cat = fixture();
        let list = supported(&cat, "claude-sonnet-4.6");
        assert_eq!(list, ["low", "medium", "high", "max"].map(String::from));
        let got = clamp_effort("xhigh", Some(&list));
        assert_eq!(got, Some("high".to_string()));
        assert_ne!(
            got,
            Some("max".to_string()),
            "the clamp must never escalate"
        );
    }

    /// The property the whole slice exists for: whatever the operator asks, the
    /// clamp returns a level the model publishes and never one above the request —
    /// with the single documented floor exception (D5a), which
    /// `every_effort_model_supports_low_medium_high` proves unreachable in practice.
    #[test]
    fn clamp_never_exceeds_the_request() {
        let cat = fixture();
        let mut checked = 0;
        let mut above_floor = 0;
        for model in &cat.models {
            let Some(list) = model.reasoning_effort.as_deref() else {
                continue;
            };
            if list.is_empty() {
                continue;
            }
            for level in EFFORT_ORDER {
                let got = clamp_effort(level, Some(list))
                    .unwrap_or_else(|| panic!("{} / {level}: no level chosen", model.id));
                assert!(
                    list.contains(&got),
                    "{} / {level}: {got} is not published",
                    model.id
                );
                // The floor exception applies ONLY when the request genuinely sits
                // below everything the model publishes. An unconditional
                // `got == list[0]` would make this property vacuous: an
                // implementation that always returned the lowest level would
                // satisfy it for every model × every level.
                let floor = list.iter().filter_map(|s| rank(s)).min();
                let ok = rank(&got) <= rank(level) || rank(level) < floor;
                assert!(ok, "{} / {level}: clamped UP to {got}", model.id);
                checked += 1;
                if rank(&got) > rank(&list[0]) {
                    above_floor += 1;
                }
            }
        }
        assert!(checked > 0, "the fixture published no effort list at all");
        // Non-degeneracy: an implementation that always answered the LOWEST
        // supported level would satisfy every assertion above. It would not
        // satisfy this one.
        assert!(
            above_floor > 0,
            "every answer was the model's floor — the property proves nothing"
        );
    }

    /// What makes the floor branch unreachable for every model the vendor actually
    /// publishes: `low`/`medium`/`high` are universal, so any request at or above
    /// `low` finds a supported level below it.
    #[test]
    fn every_effort_model_supports_low_medium_high() {
        let cat = fixture();
        for model in &cat.models {
            let Some(list) = model.reasoning_effort.as_deref() else {
                continue;
            };
            for level in ["low", "medium", "high"] {
                assert!(
                    list.iter().any(|s| s == level),
                    "{} omits {level}: {list:?}",
                    model.id
                );
            }
        }
    }

    /// A model that takes no effort argument never receives the flag, however
    /// loudly the operator asked.
    #[test]
    fn no_effort_model_never_gets_the_flag() {
        let cat = fixture();
        for id in [
            "kimi-k2.7-code",
            "claude-sonnet-4.5",
            "claude-haiku-4.5",
            "gemini-2.5-pro",
        ] {
            assert_eq!(
                resolve_effort(Some("high"), Some(id), Some(&cat)),
                None,
                "{id} takes no effort argument"
            );
        }
    }

    #[test]
    fn unknown_model_or_no_catalog_omits_the_flag() {
        let cat = fixture();
        assert_eq!(
            resolve_effort(Some("high"), Some("no-such-model"), Some(&cat)),
            None
        );
        assert_eq!(resolve_effort(Some("high"), Some("gpt-5-mini"), None), None);
        // No request, no flag — and no catalog is consulted.
        assert_eq!(resolve_effort(None, Some("gpt-5-mini"), Some(&cat)), None);
        // An unrankable level cannot be clamped, so it is omitted.
        assert_eq!(
            resolve_effort(Some("turbo"), Some("gpt-5-mini"), Some(&cat)),
            None
        );
    }

    /// With no pinned model the account default decides the support list.
    #[test]
    fn the_account_default_model_supplies_the_support_list() {
        let cat = fixture();
        assert_eq!(cat.default_model.as_deref(), Some("claude-sonnet-5"));
        assert_eq!(
            resolve_effort(Some("max"), None, Some(&cat)),
            Some("max".to_string())
        );
    }

    /// The support table is the vendor's: no model id may be baked into the
    /// non-test half of this file. Needles are assembled from fragments so the
    /// assertion cannot match itself.
    #[test]
    fn no_hardcoded_effort_table() {
        let src = include_str!("effort.rs");
        let head = src.split_once("mod tests").map(|(h, _)| h).unwrap_or(src);
        for needle in [
            concat!("\"", "claude-"),
            concat!("\"", "gpt-5"),
            concat!("\"", "gemini-"),
            concat!("\"", "kimi-"),
        ] {
            assert!(
                !head.contains(needle),
                "hardcoded effort table: {needle} appears outside the tests"
            );
        }
    }

    /// D5a's scope boundary against #227: the ordering is this adapter's, not
    /// Ralphy's vocabulary. Walks every crate and fails if the constant leaked.
    #[test]
    fn clamp_lives_only_in_the_copilot_adapter() {
        let crates = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("crates/ is the parent")
            .to_path_buf();
        let needle = concat!("EFFORT", "_ORDER");
        let mut stack = vec![crates.clone()];
        let mut scanned = 0;
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    if path.file_name().is_some_and(|n| n == "target") {
                        continue;
                    }
                    stack.push(path);
                } else if path.extension().is_some_and(|e| e == "rs") {
                    if path.starts_with(env!("CARGO_MANIFEST_DIR")) {
                        continue;
                    }
                    scanned += 1;
                    let Ok(text) = std::fs::read_to_string(&path) else {
                        continue;
                    };
                    assert!(
                        !text.contains(needle),
                        "{} names the clamp ordering: it stays inside the Copilot adapter (#227)",
                        path.display()
                    );
                }
            }
        }
        assert!(
            scanned > 100,
            "only {scanned} files scanned — walk is broken"
        );
    }
}
