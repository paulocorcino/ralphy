//! The delivery column: what each issue cost, and — beside it, never inside it —
//! the spend that bought no issue at all.
//!
//! A **delivery** is one issue joined by `issue` across every run that touched
//! it (CONTEXT.md → *Delivery*), failed attempts included: an issue that took
//! three runs cost what all three cost. Two kinds of spend are deliberately NOT
//! deliveries — the run-level `consolidate` line (`issue: 0`, real spend, no
//! issue) and an interactive session (project overhead) — and both are carried
//! as [`Overhead`] rather than dropped, so the three lines still sum to the
//! project total (PRD #355).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::format::{fmt_share, fmt_tokens, fmt_total, share_of};
use super::rows::{Classified, Priced, Source};

/// The visible grid is bounded even when the project is not: every row's cost
/// still counts in every figure, and [`Folded::truncated`] says how many the
/// list omitted.
const MAX_ROWS: usize = 60;

/// One issue's spend, already rendered.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DeliveryRow {
    pub issue: u64,
    /// The priced portion, or `None` when nothing about this issue could be
    /// priced. Never `Some(0.0)` standing in for "unknown".
    pub usd: Option<f64>,
    /// The cost, rendered: `$45.00`, `$45.00+` or `~$?`.
    pub total: String,
    /// `true` when this row omits volume it could not price — the row is a lower
    /// bound, and no row may read as exact while hiding unpriced tokens.
    pub floor: bool,
    /// How many `execute` phase lines touched the issue — the runner writes
    /// exactly one per issue per run, so this counts the runs that executed it.
    /// A delivery that was only planned reads `0`, which is true.
    pub attempts: u64,
    pub tokens: u64,
    pub tokens_label: String,
    /// This issue's share of the delivery column's priced spend, `0.0..=1.0`.
    pub share: f64,
    pub share_label: String,
}

/// The three lines PRD #355 sums into the project total. Deliveries are here as
/// a TOTAL as well as a grid, so the identity can be read on screen.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Overhead {
    /// Every delivery's spend, summed — the column's own total.
    pub deliveries_usd: Option<f64>,
    pub deliveries_total: String,
    pub deliveries_floor: bool,
    /// Interactive sessions: real spend that bought no delivery, but bought
    /// something (CONTEXT.md → *Retry burn*, `_Avoid_`).
    pub interactive_usd: Option<f64>,
    pub interactive_total: String,
    pub interactive_floor: bool,
    pub interactive_sessions: u64,
    /// The run-level `consolidate` line (`issue: 0`): the run's own overhead.
    pub consolidation_usd: Option<f64>,
    pub consolidation_total: String,
    pub consolidation_floor: bool,
}

/// The five figures the tile strip carries — the executive read (PRD #355).
/// Every one of them is derived from the SAME classified rows the grid below is,
/// so a tile and a row can never disagree about a line.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Kpis {
    /// Distinct non-zero issues the window's spend touched — "touched", not
    /// "closed" (CONTEXT.md → *Delivery*). It is the denominator below.
    pub deliveries: u64,
    /// The TYPICAL delivery's cost, and the average beside it: a long tail
    /// pulls the mean far from the issue an operator actually expects to buy.
    pub cost_per_delivery_median: Option<f64>,
    pub cost_per_delivery_median_label: String,
    pub cost_per_delivery_mean: Option<f64>,
    pub cost_per_delivery_mean_label: String,
    /// `true` when any delivery in the set hides unpriced volume — the pair is
    /// then a lower bound. Deliveries that priced to nothing are NOT dropped
    /// from the set: dropping them would inflate the typical cost.
    pub cost_per_delivery_floor: bool,
    /// The spend on phase lines that did not succeed — CONTEXT.md → *Retry
    /// burn*. Ledger only: an interactive session bought something, just not a
    /// delivery, so it would dilute the diagnosis.
    pub retry_burn_usd: Option<f64>,
    pub retry_burn_share: Option<f64>,
    pub retry_burn_label: String,
    pub retry_burn_floor: bool,
    /// `cache_read / (input + cache_read + cache_creation)` — the share of
    /// prompt-side tokens served from cache, comparable with the percentages
    /// beside it and needing no legend.
    pub cache_hit_share: Option<f64>,
    pub cache_hit_label: String,
}

/// The outcomes that count as success, in ONE place. Enumerated from the
/// runner's producers: `ok` (plan phase, and the run-level consolidate line) and
/// `done` (`outcome_label`, the protocol-repair pass, the repair pass). Every
/// other value — `blocked`, `timeout`, `stuck`, `limit`, `protocol-failed`,
/// `verify-failed`, and any future one this fold has never seen — is retry burn.
/// Fail-visible beats silently forgiven.
const SUCCESS: [&str; 2] = ["ok", "done"];

/// What one pass over the classified rows produced.
pub(crate) struct Folded {
    pub rows: Vec<DeliveryRow>,
    pub truncated: u64,
    pub overhead: Overhead,
    pub kpis: Kpis,
}

/// One issue's running spend.
#[derive(Default)]
struct Accum {
    usd: f64,
    priced: bool,
    floor: bool,
    tokens: u64,
    attempts: u64,
    /// Whether ANY row landed here at all. This is not the same question as
    /// [`Self::priced`], and conflating them is how a category with nothing in
    /// it comes to claim unpriceable spend — see [`Self::total`].
    seen: bool,
}

impl Accum {
    fn add(&mut self, row: &Priced) {
        self.seen = true;
        self.tokens += row.tokens.total();
        // A row with no cost carries no volume and no verdict of its own.
        if let Some(cost) = row.usd {
            self.usd += cost;
            self.priced = true;
        }
        // Unpriceable volume OR a vendor that counted only part of it: either
        // makes this figure a lower bound, however well the rest priced.
        self.floor |= row.floors();
    }

    fn usd(&self) -> Option<f64> {
        self.priced.then_some(self.usd)
    }

    /// The rendered figure. `~$?` is reserved for volume that EXISTS and could
    /// not be priced (ADR-0034 D3, ADR-0053): a category with no rows at all has
    /// no volume and no gap, so it is exactly `$0.00`. Rendering it `~$?` would
    /// tell the operator "we could not price your interactive spend" when the
    /// truth is "there was none" — on the one surface built to prevent exactly
    /// that misread.
    fn total(&self) -> String {
        if !self.seen {
            return fmt_total(Some(0.0), false);
        }
        fmt_total(self.usd(), self.floor)
    }
}

/// Group the classified rows into the delivery column and the overhead lines.
pub(crate) fn fold(classified: &Classified) -> Folded {
    let mut deliveries: BTreeMap<u64, Accum> = BTreeMap::new();
    let mut consolidation = Accum::default();
    let mut interactive = Accum::default();
    // A session the vendor never counted is still a session, and its spend is
    // real but unmeasurable — so it counts here AND makes the line a floor.
    let mut interactive_sessions = classified.unmetered_sessions;
    if classified.unmetered_sessions > 0 {
        interactive.seen = true;
        interactive.floor = true;
    }

    for row in &classified.rows {
        match row.issue() {
            Some(issue) => {
                let accum = deliveries.entry(issue).or_default();
                accum.add(row);
                // One `execute` line per issue per run (`runner/phases.rs:577`).
                if row.phase == "execute" {
                    accum.attempts += 1;
                }
            }
            None if matches!(row.source, Source::Interactive) => {
                interactive_sessions += 1;
                interactive.add(row);
            }
            None => consolidation.add(row),
        }
    }

    let kpis = kpis(classified, &deliveries);
    let column_usd: f64 = deliveries.values().filter_map(Accum::usd).sum();
    let column_priced = deliveries.values().any(|a| a.priced);
    // The column is a floor when ANY delivery in it hides unpriced volume — a
    // per-row `+` that never reached the column total would let the sum read as
    // exact while its parts do not.
    let column_floor = deliveries.values().any(|a| a.floor);
    // "No deliveries at all" is `$0.00`, not `~$?` — see `Accum::total`.
    let column_seen = deliveries.values().any(|a| a.seen);
    let mut ordered = deliveries.into_iter().collect::<Vec<_>>();
    // Costliest first; an unpriceable row (`None`) sorts last on cost, then by
    // volume, so the biggest unknown is still the first unknown a reader meets.
    ordered.sort_by(|(a_issue, a), (b_issue, b)| {
        b.usd()
            .unwrap_or(f64::NEG_INFINITY)
            .total_cmp(&a.usd().unwrap_or(f64::NEG_INFINITY))
            .then(b.tokens.cmp(&a.tokens))
            .then(a_issue.cmp(b_issue))
    });
    let truncated = ordered.len().saturating_sub(MAX_ROWS) as u64;
    let rows = ordered
        .into_iter()
        .take(MAX_ROWS)
        .map(|(issue, accum)| {
            let usd = accum.usd();
            let share = match (usd, column_usd) {
                (Some(value), total) if total > 0.0 => value / total,
                _ => 0.0,
            };
            DeliveryRow {
                issue,
                usd,
                total: fmt_total(usd, accum.floor),
                floor: accum.floor,
                attempts: accum.attempts,
                tokens: accum.tokens,
                tokens_label: fmt_tokens(accum.tokens),
                share,
                share_label: fmt_share(share),
            }
        })
        .collect();

    let column = column_priced.then_some(column_usd);
    Folded {
        rows,
        truncated,
        kpis,
        overhead: Overhead {
            deliveries_usd: column,
            deliveries_total: if column_seen {
                fmt_total(column, column_floor)
            } else {
                fmt_total(Some(0.0), false)
            },
            deliveries_floor: column_floor,
            interactive_usd: interactive.usd(),
            interactive_total: interactive.total(),
            interactive_floor: interactive.floor,
            interactive_sessions,
            consolidation_usd: consolidation.usd(),
            consolidation_total: consolidation.total(),
            consolidation_floor: consolidation.floor,
        },
    }
}

/// The tile strip's five figures, over the same rows and the same grouping the
/// grid uses.
fn kpis(classified: &Classified, deliveries: &BTreeMap<u64, Accum>) -> Kpis {
    // EVERY delivery counts, including one whose whole volume was unpriceable:
    // dropping it would inflate the typical cost and hide the gap.
    let mut costs = deliveries
        .values()
        .map(|a| a.usd().unwrap_or(0.0))
        .collect::<Vec<_>>();
    costs.sort_by(f64::total_cmp);
    let cost_floor = deliveries.values().any(|a| a.floor);
    let median = median(&costs);
    let mean = (!costs.is_empty()).then(|| costs.iter().sum::<f64>() / costs.len() as f64);

    // Ledger only, both sides of the ratio: an interactive session is not a
    // failed attempt, and putting it in the denominator would dilute the share.
    let ledger = classified
        .rows
        .iter()
        .filter(|row| matches!(row.source, Source::Ledger { .. }));
    let mut spent = 0.0;
    let mut burned = 0.0;
    let mut any_ledger_priced = false;
    let mut retry_burn_floor = false;
    for row in ledger {
        match row.usd {
            Some(cost) => {
                spent += cost;
                any_ledger_priced = true;
                if !SUCCESS.contains(&row.outcome) {
                    burned += cost;
                }
            }
            None => retry_burn_floor |= row.cause.is_some(),
        }
    }
    let retry_burn_share = (any_ledger_priced && spent > 0.0).then(|| burned / spent);

    let prompt = classified
        .rows
        .iter()
        .map(|row| row.tokens.input + row.tokens.cache_read + row.tokens.cache_creation)
        .sum::<u64>();
    let cached = classified
        .rows
        .iter()
        .map(|row| row.tokens.cache_read)
        .sum::<u64>();
    let cache_hit_share = (prompt > 0).then(|| share_of(cached, prompt));

    Kpis {
        deliveries: deliveries.len() as u64,
        cost_per_delivery_median: median,
        cost_per_delivery_median_label: fmt_total(median, cost_floor),
        cost_per_delivery_mean: mean,
        cost_per_delivery_mean_label: fmt_total(mean, cost_floor),
        cost_per_delivery_floor: cost_floor,
        retry_burn_usd: any_ledger_priced.then_some(burned),
        retry_burn_share,
        retry_burn_label: fmt_pct(retry_burn_share),
        retry_burn_floor,
        cache_hit_share,
        cache_hit_label: fmt_pct(cache_hit_share),
    }
}

/// The middle value, or the mean of the two middle ones for an even count.
/// `costs` must already be sorted ascending.
fn median(costs: &[f64]) -> Option<f64> {
    match costs.len() {
        0 => None,
        n if n % 2 == 1 => Some(costs[n / 2]),
        n => Some((costs[n / 2 - 1] + costs[n / 2]) / 2.0),
    }
}

/// A percentage tile, or `—` when its denominator is empty — a share of nothing
/// is not `0.0%`.
fn fmt_pct(share: Option<f64>) -> String {
    share.map_or_else(|| "—".to_string(), fmt_share)
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod kpi_tests;
