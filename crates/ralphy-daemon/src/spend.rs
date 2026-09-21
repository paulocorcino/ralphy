//! The spend fold (PRD #355, tracer bullet #358): ledger rows, interactive
//! records, the model-recovery map and the price table in — one priced summary
//! document out.
//!
//! **It is a pure function.** No filesystem, no network, no clock appears in its
//! signature or its body; the I/O stays in [`crate::usage`], which keeps
//! reparsing the ledger JSONL itself rather than depending on `ralphy-core`
//! (ADR-0032 §10). That is what makes every domain rule here unit-testable
//! without touching disk.
//!
//! **The number is honest by construction.** USD is a read-time projection and
//! is never stored (ADR-0008 D2). A model the table cannot price contributes
//! `~$?`, never `$0` (ADR-0034 D3), and never silently drops out — its tokens
//! land in [`Unpriced`], so a total carrying any of them renders as a **floor**
//! (`$2,350.59+`). The unpriced volume splits by cause, because one shrinks with
//! work and the other never will (ADR-0053 D4): a line with a `session_id` is
//! *recoverable*, a line without one is *lost*, and a real model absent from the
//! table is neither — it is one `pricing.toml` entry away.

use std::collections::BTreeMap;

use ralphy_pricing::{PriceTable, TokenCounts};
use serde::{Deserialize, Serialize};

pub mod activity;
pub mod deliveries;
#[cfg(test)]
pub(crate) mod fixtures;
pub mod format;
pub mod gap;
pub mod meter;
pub mod models;
pub mod period;
pub(crate) mod rows;

use format::fmt_total;
use gap::Gap;
use meter::Counts;

pub use activity::ActivityDay;
pub use deliveries::{DeliveryRow, Kpis, Overhead};
pub use gap::{Unpriced, UnpricedCause};
pub use meter::{MeterPart, TokenMeter};
pub use models::ModelRow;
pub use period::{Period, Window};

/// The sentinel the runner writes when a phase recorded no model attribution
/// (mirrors `ralphy_pricing`'s own constant, which is private to that crate). It
/// is never a real model id, so it is classified by recoverability rather than
/// reported as a model the operator could add to `pricing.toml`.
const UNKNOWN_MODEL: &str = "unknown";

/// Everything the fold reads. Borrowed, so the caller keeps ownership of the
/// rows it already read for `/api/usage` — the summary is a projection over that
/// same data, not a second copy of it.
pub struct SpendInput<'a> {
    /// The ledger's run records, as `/api/usage` serves them.
    pub records: &'a [serde_json::Value],
    /// The interactive records the usage scan produced (ADR-0033 §2).
    pub interactive: &'a [serde_json::Value],
    /// `session_id → model`, the persisted append-only recovery map (ADR-0053 D3).
    pub recovered: &'a BTreeMap<String, String>,
    /// The read-time price table (ADR-0034 slice A).
    pub prices: &'a PriceTable,
    /// The open project's `owner/repo` slug — the identity the board scopes on.
    pub project: &'a str,
    /// The window the figures are scoped to. Carried alongside [`Self::since`]
    /// rather than derived from it, because the activity band zero-fills a
    /// bounded window's quiet days and needs its LENGTH, not just its start.
    pub window: Window,
    /// The inclusive lower bound, RFC3339, or `None` for all time. The ROUTE
    /// derives it from the clock; the fold only compares against it, which is
    /// what keeps `summarize` clock-free and this rule unit-testable.
    pub since: Option<&'a str>,
}

/// The summary document the Spend tab renders. Small by design: opening the tab
/// must not transfer the ledger, so nothing here grows with the number of rows.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SpendSummary {
    pub project: String,
    /// The priced portion in USD, or `None` when *nothing* in the project could
    /// be priced. Never `Some(0.0)` standing in for "unknown".
    pub usd: Option<f64>,
    /// `true` when the total omits volume it could not price — so the figure is
    /// a lower bound and must be rendered as one.
    pub floor: bool,
    /// The total, already rendered: `$2,350.59+`, `$18.40`, or `~$?`. Formatted
    /// here so the money vocabulary has one implementation, not one per client.
    pub total: String,
    pub tokens: TokenMeter,
    pub unpriced: Unpriced,
    /// The window every figure above is scoped to — echoed so the client renders
    /// the label it is actually reading, never one it assumed.
    pub period: Period,
    /// One row per issue the window's spend touched, costliest first — capped,
    /// because a per-issue grid grows with the project and opening the tab must
    /// not transfer the ledger.
    pub deliveries: Vec<DeliveryRow>,
    /// How many delivery rows the cap omitted. Their cost is still inside every
    /// figure above; only the visible LIST is bounded.
    pub deliveries_truncated: u64,
    /// The spend that bought no single issue, beside the delivery column rather
    /// than inside it (PRD #355: `Σ deliveries + interactive + consolidation`).
    pub overhead: Overhead,
    /// The tile strip's figures, derived from the same rows as everything above.
    pub kpis: Kpis,
    /// One row per engine the money went to, costliest first — the unnameable
    /// volume among them as its own row, never as a hole.
    pub models: Vec<ModelRow>,
    /// Spend and deliveries on one timeline, one entry per UTC day.
    pub activity: Vec<ActivityDay>,
}

/// Fold one project's usage into its priced summary. Pure: same inputs, same
/// document, always.
pub fn summarize(input: &SpendInput) -> SpendSummary {
    let classified = rows::classify(input);

    let mut meter = Counts::default();
    let mut usd = 0.0;
    let mut any_priced = false;
    let mut unpriced = Gap {
        unmetered_sessions: classified.unmetered_sessions,
        ..Gap::default()
    };

    for row in &classified.rows {
        meter.add(&row.tokens);
        match row.usd {
            Some(cost) => {
                usd += cost;
                any_priced = true;
            }
            None => match row.cause {
                Some("no_price") => unpriced.no_price += row.tokens.total(),
                Some("recoverable") => unpriced.recoverable += row.tokens.total(),
                Some("lost") => unpriced.lost += row.tokens.total(),
                // A zero-token line: no spend, no gap, and no floor marker.
                _ => {}
            },
        }
    }

    // A session the vendor never counted, and a `lower_bound` record's partial
    // counts (ADR-0043 D10), each make the total a floor however well the rest
    // of the project priced.
    let floor = unpriced.recoverable + unpriced.no_price + unpriced.lost > 0
        || classified.unmetered_sessions > 0
        || classified.lower_bound;

    let usd = any_priced.then_some(usd);
    let deliveries = deliveries::fold(&classified);
    SpendSummary {
        project: input.project.to_string(),
        total: fmt_total(usd, floor),
        usd,
        floor,
        tokens: meter.render(),
        unpriced: unpriced.render(meter.total),
        period: input.window.render(input.since.map(str::to_string)),
        deliveries: deliveries.rows,
        deliveries_truncated: deliveries.truncated,
        overhead: deliveries.overhead,
        kpis: deliveries.kpis,
        models: models::fold(&classified),
        activity: activity::fold(&classified, input.window, input.since),
    }
}

/// Keep only the rows the given window contains — the SAME membership rule
/// [`rows::classify`] applies, so the Ledger grid and the Overview's figures are
/// folded over one population rather than two that happen to agree today.
///
/// It is deliberately a projection over rows already read, not a filter on the
/// read: `usage::run_records`'s `since` drops a row with no timestamp, while
/// [`period::in_window`] KEEPS it, and two filters that disagree about a row is
/// exactly what this function exists to prevent.
pub fn scope_to_window(
    records: &mut Vec<serde_json::Value>,
    interactive: &mut Vec<serde_json::Value>,
    since: Option<&str>,
) {
    records.retain(|row| {
        row.as_object()
            .is_some_and(|object| period::in_window(field(object, "ts"), since))
    });
    interactive.retain(|row| {
        row.as_object().is_some_and(|object| {
            // A session's window membership is its MOST RECENT activity, falling
            // back to when it started — `rows::classify`'s rule verbatim.
            let seen = field(object, "last_ts").or_else(|| field(object, "first_ts"));
            period::in_window(seen, since)
        })
    });
}

/// Annotate the RAW usage rows `/api/usage` serves with the same unpriced
/// verdict the Overview's gap is folded from, so the Ledger grid's "unpriced
/// only" filter and the Overview's unpriced split never disagree about a row.
/// The two surfaces still read different POPULATIONS in one respect the fold
/// cannot fix: `/api/usage` folds the fleet while `/api/spend` is local only
/// (PRD #355, Out of Scope) — the Ledger pane says so on screen rather than
/// leaving the operator to discover it from a count that will not add up.
///
/// A row that prices gets NO `unpriced_cause` key at all — the absence IS the
/// "this one is fine" answer, so a client that never learns the vocabulary still
/// reads the filter correctly. The values are [`gap::Gap::CAUSES`] verbatim plus
/// `unmetered`, which belongs only to a row: an interactive record with
/// `tokens: null` carries no volume for the gap to count, but it IS one of the
/// offenders the operator clicked through to see.
pub fn annotate_unpriced(
    records: &mut [serde_json::Value],
    interactive: &mut [serde_json::Value],
    recovered: &BTreeMap<String, String>,
    prices: &PriceTable,
) {
    for row in records {
        let Some(object) = row.as_object_mut() else {
            continue;
        };
        let tokens = ledger_tokens(object);
        let session = field(object, "session_id").filter(|id| !id.is_empty());
        let model = rows::recover_ledger_model(field(object, "model"), session, recovered);
        let (_, cause) = rows::price(model, session, &tokens, prices);
        if let Some(cause) = cause {
            object.insert("unpriced_cause".into(), serde_json::Value::from(cause));
        }
    }

    for row in interactive {
        let Some(object) = row.as_object_mut() else {
            continue;
        };
        let Some(tokens) = interactive_tokens(object) else {
            object.insert(
                "unpriced_cause".into(),
                serde_json::Value::from("unmetered"),
            );
            continue;
        };
        let session = field(object, "session_id").filter(|id| !id.is_empty());
        let model = rows::recover_interactive_model(field(object, "model"), session, recovered);
        let (_, cause) = rows::price(model, session, &tokens, prices);
        if let Some(cause) = cause {
            object.insert("unpriced_cause".into(), serde_json::Value::from(cause));
        }
    }
}

/// One string field of a JSON object, or `None` when absent or not a string.
fn field<'a>(object: &'a serde_json::Map<String, serde_json::Value>, key: &str) -> Option<&'a str> {
    object.get(key).and_then(serde_json::Value::as_str)
}

/// A ledger line's four token counts, read tolerantly from its `tokens` object —
/// a missing member is `0`, mirroring `ralphy_core::ledger::read_rows`'s stance
/// on a best-effort append-only file.
fn ledger_tokens(object: &serde_json::Map<String, serde_json::Value>) -> TokenCounts {
    let tokens = object.get("tokens");
    let count = |key: &str| {
        tokens
            .and_then(|t| t.get(key))
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0)
    };
    TokenCounts {
        input: count("input"),
        output: count("output"),
        cache_read: count("cache_read"),
        cache_creation: count("cache_creation"),
    }
}

/// An interactive record's counts, or `None` when its `tokens` is `null` — the
/// scan's way of saying the vendor keeps no count anywhere, which a consumer must
/// not read as zero.
fn interactive_tokens(object: &serde_json::Map<String, serde_json::Value>) -> Option<TokenCounts> {
    let tokens = object.get("tokens")?;
    if tokens.is_null() {
        return None;
    }
    let count = |key: &str| {
        tokens
            .get(key)
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0)
    };
    Some(TokenCounts {
        input: count("input"),
        output: count("output"),
        cache_read: count("cache_read"),
        cache_creation: count("cache_creation"),
    })
}

#[cfg(test)]
mod tests;
