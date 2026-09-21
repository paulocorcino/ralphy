use super::super::fixtures::{fold_within, InteractiveRow, LedgerRow};
use super::super::Window;

/// One issue, two runs: the first timed out, the second delivered. Two
/// thirds of what the issue cost bought nothing — that is the number the
/// tile exists to state.
#[test]
fn retry_burn_is_the_share_of_spend_that_bought_no_delivery() {
    let burned = |outcome: &str| {
        [
            LedgerRow {
                issue: 251,
                outcome: "done",
                input: 1_000_000,
                ..Default::default()
            }
            .json(),
            LedgerRow {
                issue: 251,
                outcome,
                input: 2_000_000,
                ..Default::default()
            }
            .json(),
        ]
    };

    let rows = burned("timeout");
    let summary = fold_within(&rows, &[], Window::All, None);
    assert_eq!(summary.kpis.retry_burn_usd, Some(30.0));
    assert_eq!(summary.kpis.retry_burn_label, "66.7%");
    assert!(!summary.kpis.retry_burn_floor, "everything priced");

    // Negative control: the SAME fixture with the failing row succeeding.
    // Without it, a classifier that called everything burn would also pass.
    let rows = burned("done");
    let summary = fold_within(&rows, &[], Window::All, None);
    assert_eq!(summary.kpis.retry_burn_usd, Some(0.0));
    assert_eq!(summary.kpis.retry_burn_label, "0.0%");
}

/// The success set is the RUNNER's, per value: `ok` and `done` bought a
/// delivery, every other outcome the runner writes did not — and an outcome
/// this fold has never seen counts as burn, because fail-visible beats
/// silently forgiven.
#[test]
fn outcome_vocabulary_is_the_runners() {
    let one = |outcome: &str| {
        let rows = [LedgerRow {
            issue: 251,
            outcome,
            input: 1_000_000,
            ..Default::default()
        }
        .json()];
        fold_within(&rows, &[], Window::All, None)
            .kpis
            .retry_burn_usd
    };

    for outcome in [
        "blocked",
        "timeout",
        "stuck",
        "limit",
        "protocol-failed",
        "verify-failed",
        "a-verb-nobody-has-written-yet",
    ] {
        assert_eq!(one(outcome), Some(15.0), "`{outcome}` is retry burn");
    }
    for outcome in ["ok", "done"] {
        assert_eq!(one(outcome), Some(0.0), "`{outcome}` bought a delivery");
    }
}

/// Retry burn is LEDGER-only on BOTH sides of the ratio. An interactive
/// session bought something — just not a delivery — so folding it into the
/// denominator would dilute the diagnosis (CONTEXT.md → *Retry burn*).
/// Without this, a fold that summed interactive spend into `spent` keeps
/// every other assertion in this file green.
#[test]
fn interactive_spend_is_in_neither_side_of_the_retry_burn_ratio() {
    let rows = [
        LedgerRow {
            issue: 251,
            outcome: "done",
            input: 1_000_000,
            ..Default::default()
        }
        .json(),
        LedgerRow {
            issue: 251,
            outcome: "timeout",
            input: 2_000_000,
            ..Default::default()
        }
        .json(),
    ];
    let alone = fold_within(&rows, &[], Window::All, None);

    // The same ledger, now with a large interactive session beside it. A
    // denominator that grew would drag 66.7% down toward 33.3%.
    let interactive = [InteractiveRow {
        tokens: Some((3_000_000, 0)),
        ..Default::default()
    }
    .json()];
    let beside = fold_within(&rows, &interactive, Window::All, None);

    assert_eq!(beside.kpis.retry_burn_label, "66.7%");
    assert_eq!(
        beside.kpis.retry_burn_usd, alone.kpis.retry_burn_usd,
        "an interactive session is not a failed attempt"
    );
    assert_eq!(
        beside.usd,
        Some(90.0),
        "…while still being in the project total"
    );
}

/// A `lower_bound` record's counts are a FLOOR, not the bill (ADR-0043 D10),
/// and the caveat belongs to every figure the row lands in — not only to the
/// project total. A model row or an overhead line reading as EXACT while the
/// total beside it reads as a floor is the contradiction this pins.
#[test]
fn a_lower_bound_record_floors_every_figure_it_lands_in() {
    let interactive = [InteractiveRow {
        tokens: Some((1_000_000, 0)),
        lower_bound: true,
        ..Default::default()
    }
    .json()];
    let summary = fold_within(&[], &interactive, Window::All, None);

    assert_eq!(
        summary.total, "$15.00+",
        "the project total, as #358 had it"
    );
    assert!(summary.overhead.interactive_floor);
    assert_eq!(summary.overhead.interactive_total, "$15.00+");
    assert!(summary.models[0].floor, "the engine's row is a floor too");
    assert_eq!(summary.models[0].total, "$15.00+");
    assert!(summary.activity[0].floor, "and so is the day it fell on");
    assert_eq!(summary.activity[0].usd_label, "$15.00+");
}

/// A category with NO rows is `$0.00`, never `~$?`: the marker is reserved
/// for volume that exists and could not be priced (ADR-0034 D3). "We could
/// not price your interactive spend" when there was none is exactly the
/// misread this whole surface exists to prevent.
#[test]
fn an_empty_overhead_category_is_zero_not_unpriceable() {
    let rows = [LedgerRow {
        issue: 251,
        input: 1_000_000,
        ..Default::default()
    }
    .json()];
    let summary = fold_within(&rows, &[], Window::All, None);

    assert_eq!(summary.overhead.interactive_total, "$0.00");
    assert_eq!(summary.overhead.interactive_usd, None);
    assert!(!summary.overhead.interactive_floor);
    assert_eq!(summary.overhead.consolidation_total, "$0.00");
    assert_eq!(summary.overhead.deliveries_total, "$15.00");

    // …and the converse still holds: a category WITH volume nobody could
    // price keeps its `~$?`.
    let unpriceable = [LedgerRow {
        issue: 0,
        phase: "consolidate",
        outcome: "ok",
        model: "unknown",
        session: None,
        input: 1_000_000,
        ..Default::default()
    }
    .json()];
    let summary = fold_within(&unpriceable, &[], Window::All, None);
    assert_eq!(summary.overhead.consolidation_total, "~$?");
    assert_eq!(
        summary.overhead.deliveries_total, "$0.00",
        "no deliveries at all is zero, not unpriceable"
    );
}

/// The typical delivery and the average one, side by side: three deliveries
/// at 45 / 15 / 3 have a median of 15 and a mean of 21, and the gap between
/// them is the tail an operator needs to see.
#[test]
fn cost_per_delivery_reads_as_median_and_mean() {
    let mut rows = vec![
        LedgerRow {
            issue: 251,
            input: 3_000_000,
            ..Default::default()
        }
        .json(),
        LedgerRow {
            issue: 300,
            input: 1_000_000,
            ..Default::default()
        }
        .json(),
        LedgerRow {
            issue: 12,
            input: 200_000,
            ..Default::default()
        }
        .json(),
    ];
    let summary = fold_within(&rows, &[], Window::All, None);
    assert_eq!(summary.kpis.deliveries, 3);
    assert_eq!(summary.kpis.cost_per_delivery_median_label, "$15.00");
    assert_eq!(summary.kpis.cost_per_delivery_mean_label, "$21.00");

    // An even count takes the mean of the two middle values: 3 / 9 / 15 / 45.
    rows.push(
        LedgerRow {
            issue: 400,
            input: 600_000,
            ..Default::default()
        }
        .json(),
    );
    let summary = fold_within(&rows, &[], Window::All, None);
    assert_eq!(summary.kpis.deliveries, 4);
    assert_eq!(summary.kpis.cost_per_delivery_median_label, "$12.00");
}

/// Cache reuse as a percentage of the prompt side — comparable with the
/// other tiles, and `—` rather than `0.0%` when there is no prompt side to
/// take a share of.
#[test]
fn cache_hit_is_the_share_of_prompt_tokens_served_from_cache() {
    let rows = [LedgerRow {
        issue: 251,
        input: 1_000_000,
        cache_read: 3_000_000,
        cache_creation: 0,
        ..Default::default()
    }
    .json()];
    let summary = fold_within(&rows, &[], Window::All, None);
    assert_eq!(summary.kpis.cache_hit_label, "75.0%");
    assert_eq!(summary.kpis.cache_hit_share, Some(0.75));

    // Output-only volume has no prompt side: a share of nothing is not zero.
    let rows = [LedgerRow {
        issue: 251,
        output: 1_000_000,
        ..Default::default()
    }
    .json()];
    let summary = fold_within(&rows, &[], Window::All, None);
    assert_eq!(summary.kpis.cache_hit_share, None);
    assert_eq!(summary.kpis.cache_hit_label, "—");
}

/// The slice's marquee rule, carried into every NEW figure: one unpriceable
/// line and the delivery row, the column, the cost-per-delivery pair and the
/// retry-burn tile all read as lower bounds — and its tokens are still in
/// the unpriced split, not silently dropped.
#[test]
fn an_unknown_model_makes_every_new_figure_a_floor() {
    let rows = [
        LedgerRow {
            issue: 251,
            input: 1_000_000,
            ..Default::default()
        }
        .json(),
        LedgerRow {
            issue: 251,
            model: "unknown",
            session: Some("s2"),
            input: 500_000,
            ..Default::default()
        }
        .json(),
    ];
    let summary = fold_within(&rows, &[], Window::All, None);

    assert!(summary.deliveries[0].floor);
    assert!(
        summary.deliveries[0].total.ends_with('+'),
        "the row must read as a lower bound: {}",
        summary.deliveries[0].total
    );
    assert!(summary.kpis.cost_per_delivery_floor);
    assert!(summary.kpis.retry_burn_floor);
    assert!(summary.overhead.deliveries_floor);
    assert_eq!(
        summary
            .unpriced
            .causes
            .iter()
            .map(|c| (c.key.as_str(), c.tokens))
            .collect::<Vec<_>>(),
        [("recoverable", 500_000)],
        "the unpriced tokens are still reported, not dropped"
    );
}
