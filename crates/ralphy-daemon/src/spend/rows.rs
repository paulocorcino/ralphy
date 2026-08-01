//! One row, classified once. Recovery, pricing and the unpriced-cause verdict
//! are decided HERE, and every projection the document carries — the meter, the
//! gap, the deliveries grid, the KPIs, the models grid, the activity band — reads
//! the same [`Priced`] slice.
//!
//! That single pass is the point: six folds each re-deriving "which model was
//! this, and did it price" is six chances for the delivery column and the
//! project total to disagree about the same line.

use ralphy_pricing::{PriceTable, TokenCounts};

use super::period::in_window;
use super::{field, interactive_tokens, ledger_tokens, SpendInput, UNKNOWN_MODEL};

/// Which engine spent a LEDGER row's tokens, after recovery — the ONE
/// implementation of that rule, so the Overview's fold and the Ledger grid's
/// annotator can never resolve the same line to different models (ADR-0053 D2:
/// the ledger is never rewritten; the repair is a projection).
pub(crate) fn recover_ledger_model<'a>(
    recorded: Option<&'a str>,
    session: Option<&str>,
    recovered: &'a std::collections::BTreeMap<String, String>,
) -> Option<&'a str> {
    match (recorded.unwrap_or(UNKNOWN_MODEL), session) {
        (UNKNOWN_MODEL | "", Some(id)) => recovered.get(id).map(String::as_str),
        (UNKNOWN_MODEL | "", None) => None,
        (model, _) => Some(model),
    }
}

/// The same rule for an INTERACTIVE record, whose scan writes the model directly
/// and falls back to the recovery map keyed on its `session_id`.
pub(crate) fn recover_interactive_model<'a>(
    recorded: Option<&'a str>,
    session: Option<&str>,
    recovered: &'a std::collections::BTreeMap<String, String>,
) -> Option<&'a str> {
    recorded
        .filter(|m| !m.is_empty() && *m != UNKNOWN_MODEL)
        .or_else(|| session.and_then(|id| recovered.get(id).map(String::as_str)))
}

/// Did this row price, and if not, why not — the ONE implementation of the
/// verdict. Every surface that splits priced from unpriced volume (the Overview's
/// gap, the Ledger grid's `unpriced_cause` annotation) calls this, because two
/// implementations of "did this line price" is exactly how they come to disagree
/// about the same row.
///
/// `model` is the model AFTER recovery; `session` is the row's non-empty
/// `session_id`, which is what separates a gap that can still be closed from one
/// that never will (ADR-0053 D4).
pub(crate) fn price(
    model: Option<&str>,
    session: Option<&str>,
    tokens: &TokenCounts,
    prices: &PriceTable,
) -> (Option<f64>, Option<&'static str>) {
    // A zero-token line carries no spend and no signal — pricing it would force
    // a spurious floor marker for nothing.
    if tokens.total() == 0 {
        return (None, None);
    }
    match model {
        Some(model) => match prices.cost_usd(model, tokens) {
            Some(cost) => (Some(cost), None),
            None => (None, Some("no_price")),
        },
        // Unrecovered: the line never said which engine spent these tokens. With
        // a session id a vendor store can still name it; without one, no key to
        // any store exists.
        None if session.is_some() => (None, Some("recoverable")),
        None => (None, Some("lost")),
    }
}

/// Where a row's spend came from — the distinction the overhead lines rest on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Source {
    /// A ledger phase line for one issue. `0` is the run-level `consolidate`
    /// line, which is real spend but never a delivery.
    Ledger { issue: u64 },
    /// An interactive session (ADR-0033): project-level overhead, never a row in
    /// the deliveries grid.
    Interactive,
}

/// One row of usage after classification.
pub(crate) struct Priced<'a> {
    pub source: Source,
    /// The runner's phase outcome (`done`, `timeout`, …); empty for interactive.
    pub outcome: &'a str,
    /// The row's phase (`plan` | `execute` | `consolidate`); empty for
    /// interactive. An `execute` line is one attempt.
    pub phase: &'a str,
    /// The model AFTER recovery, or `None` when no engine could be named for
    /// this row's tokens.
    pub model: Option<&'a str>,
    /// The UTC civil date the row buckets into (`2026-07-30`), or empty when it
    /// carried no timestamp at all.
    pub date: &'a str,
    pub tokens: TokenCounts,
    /// The row's cost, or `None` when its volume could not be priced.
    pub usd: Option<f64>,
    /// `recoverable` | `no_price` | `lost` when [`Self::usd`] is `None` and the
    /// row carried volume; `None` when the row priced or carried nothing.
    pub cause: Option<&'static str>,
    /// This row's counts are a FLOOR, not the bill (ADR-0043 D10) — a vendor
    /// that hides part of its usage. Carried PER ROW, not only per project, so
    /// every figure the row lands in inherits the caveat: a delivery, a model
    /// row and a band day are each as much a lower bound as the total is.
    pub lower_bound: bool,
}

impl Priced<'_> {
    /// Does this row make the figure it lands in a lower bound? Either it
    /// carried volume nobody could price, or the vendor only counted part of it.
    pub(crate) fn floors(&self) -> bool {
        self.cause.is_some() || self.lower_bound
    }
}

impl Priced<'_> {
    /// Is this row a delivery's spend? `issue: 0` is the run-level consolidate
    /// line and interactive is project overhead — neither bought an issue.
    pub(crate) fn issue(&self) -> Option<u64> {
        match self.source {
            Source::Ledger { issue } if issue > 0 => Some(issue),
            _ => None,
        }
    }
}

/// What one pass over the input produced: the classified rows, plus the two
/// floor signals that belong to no row's arithmetic.
pub(crate) struct Classified<'a> {
    pub rows: Vec<Priced<'a>>,
    /// Interactive sessions whose vendor keeps no token count anywhere.
    pub unmetered_sessions: u64,
    /// A `lower_bound` record was seen: the counts are a floor, not the bill
    /// (ADR-0043 D10), however well every model priced.
    pub lower_bound: bool,
}

/// Classify every row of the open project that falls inside the window.
pub(crate) fn classify<'a>(input: &SpendInput<'a>) -> Classified<'a> {
    let mut out = Classified {
        rows: Vec::new(),
        unmetered_sessions: 0,
        lower_bound: false,
    };

    for row in input.records {
        let Some(object) = row.as_object() else {
            continue;
        };
        if field(object, "project") != Some(input.project) {
            continue;
        }
        let ts = field(object, "ts");
        if !in_window(ts, input.since) {
            continue;
        }
        let tokens = ledger_tokens(object);
        let session = field(object, "session_id").filter(|id| !id.is_empty());
        let model = recover_ledger_model(field(object, "model"), session, input.recovered);
        let (usd, cause) = price(model, session, &tokens, input.prices);
        out.rows.push(Priced {
            source: Source::Ledger {
                issue: object
                    .get("issue")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(0),
            },
            outcome: field(object, "outcome").unwrap_or(""),
            phase: field(object, "phase").unwrap_or(""),
            model,
            date: civil_date(ts),
            tokens,
            usd,
            cause,
            lower_bound: false,
        });
    }

    for row in input.interactive {
        let Some(object) = row.as_object() else {
            continue;
        };
        if field(object, "project") != Some(input.project) {
            continue;
        }
        // A session's window membership is its MOST RECENT activity, falling
        // back to when it started; a record with neither is kept, matching
        // `usage.rs`'s best-effort stance on the scan's data.
        let seen = field(object, "last_ts").or_else(|| field(object, "first_ts"));
        if !in_window(seen, input.since) {
            continue;
        }
        let Some(tokens) = interactive_tokens(object) else {
            // The vendor keeps no count at all — unavailable, never zero.
            out.unmetered_sessions += 1;
            continue;
        };
        let lower_bound = object
            .get("lower_bound")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        out.lower_bound |= lower_bound;
        // An interactive record always carries its `session_id`, so an
        // unpriceable one is never *lost* — that property falls out of
        // `session.is_some()` rather than needing a rule of its own.
        let session = field(object, "session_id").filter(|id| !id.is_empty());
        let model = recover_interactive_model(field(object, "model"), session, input.recovered);
        let (usd, cause) = price(model, session, &tokens, input.prices);
        out.rows.push(Priced {
            source: Source::Interactive,
            outcome: "",
            phase: "",
            model,
            date: civil_date(seen),
            tokens,
            usd,
            cause,
            lower_bound,
        });
    }

    out
}

/// The UTC civil date an RFC3339 timestamp falls on — its first 10 characters,
/// because the ledger writes `chrono::Utc::now().to_rfc3339()` and the scan's
/// timestamps share that shape. An absent or short value buckets nowhere.
fn civil_date(ts: Option<&str>) -> &str {
    match ts {
        Some(ts) if ts.len() >= 10 && ts.is_char_boundary(10) => &ts[..10],
        _ => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spend::fixtures::{table, OPUS};

    /// The shared verdict, arm by arm. The `None`-model pair is the one that
    /// MOVED when `classify`'s two matches collapsed into this function
    /// (interactive used to answer `recoverable` with no session at all), so
    /// both directions are pinned here rather than left to a comment.
    #[test]
    fn the_pricing_verdict_splits_on_the_session_id() {
        let prices = table();
        let volume = TokenCounts {
            input: 1_000_000,
            output: 0,
            cache_read: 0,
            cache_creation: 0,
        };

        assert_eq!(
            price(Some(OPUS), Some("s1"), &volume, &prices),
            (Some(15.0), None)
        );
        assert_eq!(
            price(Some("big-pickle"), Some("s1"), &volume, &prices).1,
            Some("no_price"),
            "a REAL model the table lacks is one pricing.toml entry away"
        );
        assert_eq!(
            price(None, Some("s1"), &volume, &prices).1,
            Some("recoverable"),
            "a session id is a key a vendor store can still be read with"
        );
        assert_eq!(
            price(None, None, &volume, &prices).1,
            Some("lost"),
            "no model AND no session: no key to any store exists"
        );
        // A zero-token line carries no spend and no gap, so it must not force a
        // floor marker — for either kind of row.
        assert_eq!(
            price(None, None, &TokenCounts::default(), &prices),
            (None, None)
        );
    }

    /// Recovery resolves identically for both callers of these helpers — the
    /// duplication the shared verdict alone did not remove.
    #[test]
    fn recovery_names_an_unknown_models_engine_from_the_map() {
        let map = std::collections::BTreeMap::from([("s1".to_string(), OPUS.to_string())]);

        assert_eq!(recover_ledger_model(Some(OPUS), None, &map), Some(OPUS));
        assert_eq!(
            recover_ledger_model(Some("unknown"), Some("s1"), &map),
            Some(OPUS)
        );
        assert_eq!(
            recover_ledger_model(Some("unknown"), Some("s9"), &map),
            None
        );
        assert_eq!(recover_ledger_model(Some(""), None, &map), None);
        assert_eq!(recover_ledger_model(None, Some("s1"), &map), Some(OPUS));

        assert_eq!(
            recover_interactive_model(Some(OPUS), None, &map),
            Some(OPUS)
        );
        assert_eq!(
            recover_interactive_model(Some("unknown"), Some("s1"), &map),
            Some(OPUS)
        );
        assert_eq!(recover_interactive_model(Some(""), Some("s9"), &map), None);
    }
}
