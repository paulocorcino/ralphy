//! One row, classified once. Recovery, pricing and the unpriced-cause verdict
//! are decided HERE, and every projection the document carries — the meter, the
//! gap, the deliveries grid, the KPIs, the models grid, the activity band — reads
//! the same [`Priced`] slice.
//!
//! That single pass is the point: six folds each re-deriving "which model was
//! this, and did it price" is six chances for the delivery column and the
//! project total to disagree about the same line.

use ralphy_pricing::TokenCounts;

use super::period::in_window;
use super::{field, interactive_tokens, ledger_tokens, SpendInput, UNKNOWN_MODEL};

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
        // Recovery is applied HERE, not by the caller, so the fold is a complete
        // statement of the rule and testable on its own (ADR-0053 D2: the ledger
        // is never rewritten — the repair is a projection).
        let recorded = field(object, "model").unwrap_or(UNKNOWN_MODEL);
        let model = match (recorded, session) {
            (UNKNOWN_MODEL | "", Some(id)) => input.recovered.get(id).map(String::as_str),
            (UNKNOWN_MODEL | "", None) => None,
            (model, _) => Some(model),
        };
        // A zero-token line carries no spend and no signal — pricing it would
        // force a spurious floor marker for nothing.
        let (usd, cause) = if tokens.total() == 0 {
            (None, None)
        } else {
            match model {
                Some(model) => match input.prices.cost_usd(model, &tokens) {
                    Some(cost) => (Some(cost), None),
                    None => (None, Some("no_price")),
                },
                // Unrecovered: the line never said which engine spent these
                // tokens. With a session id a vendor store can still name it;
                // without one, no key to any store exists.
                None if session.is_some() => (None, Some("recoverable")),
                None => (None, Some("lost")),
            }
        };
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
        let model = field(object, "model")
            .filter(|m| !m.is_empty() && *m != UNKNOWN_MODEL)
            .or_else(|| {
                field(object, "session_id")
                    .and_then(|id| input.recovered.get(id).map(String::as_str))
            });
        let (usd, cause) = if tokens.total() == 0 {
            (None, None)
        } else {
            match model.and_then(|model| input.prices.cost_usd(model, &tokens)) {
                Some(cost) => (Some(cost), None),
                // An interactive record always carries its `session_id`, so an
                // unpriceable one is never *lost* — it is either a model the
                // table lacks or one recovery can still name.
                None if model.is_some() => (None, Some("no_price")),
                None => (None, Some("recoverable")),
            }
        };
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
