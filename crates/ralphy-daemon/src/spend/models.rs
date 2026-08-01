//! The models grid: which engine the money went to. "The money has an address"
//! is the point of this surface, so the volume no model could be named for is a
//! ROW here — hiding it would contradict the unpriced panel one column over.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::format::{fmt_share, fmt_tokens, fmt_total};
use super::rows::{Classified, Priced};
use super::UNKNOWN_MODEL;

/// One engine's spend across the window — ledger and interactive alike, because
/// the question is what the model cost, not which surface invoked it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ModelRow {
    /// The model id AFTER recovery, or `unknown` for volume no engine could be
    /// named for.
    pub model: String,
    pub usd: Option<f64>,
    /// The cost, rendered: `$45.00`, `$45.00+`, or `~$?` when this row's volume
    /// could not be priced at all.
    pub total: String,
    pub floor: bool,
    /// This model's share of the project's priced spend, `0.0..=1.0`.
    pub share: f64,
    /// The same share, rendered — or `—` for a row with no cost to take a share
    /// of, which is not `0.0%`.
    pub share_label: String,
    pub tokens: u64,
    pub tokens_label: String,
    /// `false` when nothing in this row could be priced: the surface styles the
    /// row as a gap rather than as a cheap engine.
    pub priced: bool,
}

#[derive(Default)]
struct Accum {
    usd: f64,
    priced: bool,
    floor: bool,
    tokens: u64,
}

/// Fold every row carrying volume into one row per engine.
pub(crate) fn fold(classified: &Classified) -> Vec<ModelRow> {
    let mut models: BTreeMap<&str, Accum> = BTreeMap::new();
    for row in &classified.rows {
        if row.tokens.total() == 0 {
            continue;
        }
        let accum = models.entry(key(row)).or_default();
        accum.tokens += row.tokens.total();
        if let Some(cost) = row.usd {
            accum.usd += cost;
            accum.priced = true;
        }
        accum.floor |= row.floors();
    }

    let total: f64 = models
        .values()
        .filter(|a| a.priced)
        .map(|a| a.usd)
        .sum::<f64>();
    let mut rows = models
        .into_iter()
        .map(|(model, accum)| {
            let usd = accum.priced.then_some(accum.usd);
            let share = match usd {
                Some(value) if total > 0.0 => value / total,
                _ => 0.0,
            };
            ModelRow {
                model: model.to_string(),
                usd,
                total: fmt_total(usd, accum.floor),
                floor: accum.floor,
                share,
                share_label: if usd.is_some() {
                    fmt_share(share)
                } else {
                    "—".to_string()
                },
                tokens: accum.tokens,
                tokens_label: fmt_tokens(accum.tokens),
                priced: accum.priced,
            }
        })
        .collect::<Vec<_>>();
    // Priced rows by cost; the unpriced ones last, by volume — the biggest gap
    // is the first gap a reader meets, and it sits below the money it is not in.
    rows.sort_by(|a, b| {
        b.usd
            .unwrap_or(f64::NEG_INFINITY)
            .total_cmp(&a.usd.unwrap_or(f64::NEG_INFINITY))
            .then(b.tokens.cmp(&a.tokens))
            .then(a.model.cmp(&b.model))
    });
    rows
}

/// A row's grid key: the engine it named, or the `unknown` bucket. Every
/// unnameable row shares ONE key, so the gap reads as one address rather than
/// scattering across per-session rows.
fn key<'a>(row: &Priced<'a>) -> &'a str {
    row.model.unwrap_or(UNKNOWN_MODEL)
}

#[cfg(test)]
mod tests {
    use super::super::fixtures::{fold_within, InteractiveRow, LedgerRow, OPUS, SECOND};
    use super::super::Window;

    /// The grid's job: name the engine, say what it cost, and say what share of
    /// the project that was — three quarters on one engine is the read.
    #[test]
    fn the_models_grid_carries_cost_and_share_per_engine() {
        let rows = [
            LedgerRow {
                issue: 251,
                input: 3_000_000,
                ..Default::default()
            }
            .json(),
            LedgerRow {
                issue: 300,
                model: SECOND,
                input: 1_000_000,
                ..Default::default()
            }
            .json(),
        ];
        let summary = fold_within(&rows, &[], Window::All, None);

        assert_eq!(summary.models.len(), 2);
        assert_eq!(summary.models[0].model, OPUS, "costliest engine first");
        assert_eq!(summary.models[0].total, "$45.00");
        assert_eq!(summary.models[0].share_label, "75.0%");
        assert_eq!(summary.models[0].tokens, 3_000_000);
        assert!(summary.models[0].priced);
        assert_eq!(summary.models[1].model, SECOND);
        assert_eq!(summary.models[1].total, "$15.00");
        assert_eq!(summary.models[1].share_label, "25.0%");
    }

    /// The key is the ENGINE, not the surface that invoked it: an interactive
    /// session on the same model folds into the same row.
    #[test]
    fn interactive_volume_folds_into_its_engines_row() {
        let rows = [LedgerRow {
            issue: 251,
            input: 3_000_000,
            ..Default::default()
        }
        .json()];
        let interactive = [InteractiveRow {
            tokens: Some((1_000_000, 0)),
            ..Default::default()
        }
        .json()];
        let summary = fold_within(&rows, &interactive, Window::All, None);

        assert_eq!(summary.models.len(), 1, "one engine, one row");
        assert_eq!(summary.models[0].total, "$60.00");
        assert_eq!(summary.models[0].tokens, 4_000_000);
    }

    /// The gap has an address too: volume no engine could be named for is a
    /// trailing row, never absent and never `$0.00` — a grid that hid the
    /// unnamed share would contradict the unpriced panel beside it. A REAL model
    /// the table cannot price is the same shape, under its own name.
    #[test]
    fn an_unnameable_model_is_a_row_not_a_hole() {
        let rows = [
            LedgerRow {
                issue: 251,
                input: 1_000_000,
                ..Default::default()
            }
            .json(),
            LedgerRow {
                issue: 251,
                model: "big-pickle",
                session: Some("s3"),
                input: 500_000,
                ..Default::default()
            }
            .json(),
            LedgerRow {
                issue: 251,
                model: "unknown",
                session: None,
                input: 2_000_000,
                ..Default::default()
            }
            .json(),
        ];
        let summary = fold_within(&rows, &[], Window::All, None);

        assert_eq!(
            summary
                .models
                .iter()
                .map(|m| (m.model.as_str(), m.total.as_str(), m.share_label.as_str()))
                .collect::<Vec<_>>(),
            [
                (OPUS, "$15.00", "100.0%"),
                // Unpriced rows last, by volume: the biggest gap is met first.
                ("unknown", "~$?", "—"),
                ("big-pickle", "~$?", "—"),
            ],
            "never `$0.00`, never `0.0%`, and never absent"
        );
        assert!(summary.models.iter().skip(1).all(|m| !m.priced));
        assert_eq!(
            summary.models[1].tokens, 2_000_000,
            "the volume is stated, not dropped"
        );
    }
}
