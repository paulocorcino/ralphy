use super::super::fixtures::{fold_within, InteractiveRow, LedgerRow, OPUS};
use super::super::Window;

/// The rule CONTEXT.md fixes: a delivery is the ISSUE, joined across every
/// run that touched it — a failed attempt cost real money and stays in the
/// issue's cost. Two `execute` lines on one issue are one row of two
/// attempts, not two rows.
#[test]
fn a_delivery_sums_every_run_that_touched_it_including_failed_attempts() {
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
    let summary = fold_within(&rows, &[], Window::All, None);

    assert_eq!(summary.deliveries.len(), 1, "one issue, one row");
    assert_eq!(summary.deliveries[0].issue, 251);
    assert_eq!(summary.deliveries[0].usd, Some(45.0));
    assert_eq!(summary.deliveries[0].total, "$45.00");
    assert_eq!(
        summary.deliveries[0].attempts, 2,
        "each `execute` line is one attempt, failed ones included"
    );
    assert_eq!(summary.deliveries[0].tokens, 3_000_000);
    assert_eq!(summary.deliveries[0].share_label, "100.0%");
    assert_eq!(summary.deliveries_truncated, 0);
}

/// The grid answers "where did the money go", so it opens with the most
/// expensive issue.
#[test]
fn deliveries_are_ordered_by_cost_descending() {
    let rows = [
        LedgerRow {
            issue: 12,
            input: 200_000,
            ..Default::default()
        }
        .json(),
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
    ];
    let summary = fold_within(&rows, &[], Window::All, None);

    assert_eq!(
        summary
            .deliveries
            .iter()
            .map(|d| d.issue)
            .collect::<Vec<_>>(),
        [251, 300, 12],
        "costliest first: 45.0 / 15.0 / 3.0"
    );
    assert_eq!(
        summary
            .deliveries
            .iter()
            .map(|d| d.total.as_str())
            .collect::<Vec<_>>(),
        ["$45.00", "$15.00", "$3.00"]
    );
}

/// `issue: 0` is the run-level `consolidate` line — real spend that bought
/// no issue. Folding it into the grid would invent an issue #0; dropping it
/// would break the identity. It is overhead.
#[test]
fn issue_zero_is_consolidation_overhead_not_a_phantom_issue() {
    let rows = [
        LedgerRow {
            issue: 0,
            phase: "consolidate",
            outcome: "ok",
            input: 1_000_000,
            ..Default::default()
        }
        .json(),
        LedgerRow {
            issue: 251,
            input: 1_000_000,
            ..Default::default()
        }
        .json(),
    ];
    let summary = fold_within(&rows, &[], Window::All, None);

    assert_eq!(summary.overhead.consolidation_usd, Some(15.0));
    assert_eq!(summary.overhead.consolidation_total, "$15.00");
    assert!(
        !summary.deliveries.iter().any(|d| d.issue == 0),
        "issue 0 is not a delivery: {:?}",
        summary.deliveries
    );
    assert_eq!(summary.deliveries.len(), 1);
    assert_eq!(summary.overhead.deliveries_usd, Some(15.0));
}

/// An interactive session bought something, just not a delivery — so it is
/// project overhead beside the column, never a row inside it and never in
/// the column's total.
#[test]
fn interactive_usage_is_overhead_not_a_delivery() {
    let interactive = [InteractiveRow {
        tokens: Some((1_000_000, 0)),
        ..Default::default()
    }
    .json()];
    let summary = fold_within(&[], &interactive, Window::All, None);

    assert!(summary.deliveries.is_empty());
    assert_eq!(summary.overhead.interactive_usd, Some(15.0));
    assert_eq!(summary.overhead.interactive_sessions, 1);
    assert_eq!(
        summary.overhead.deliveries_usd, None,
        "an interactive session must never reach the delivery column"
    );
    // `$0.00`, not `~$?`: there were no deliveries at all, which is a
    // measured zero — not volume nobody could price.
    assert_eq!(summary.overhead.deliveries_total, "$0.00");
    assert_eq!(summary.usd, Some(15.0), "it IS in the project total");
}

/// The visible list is bounded even when the project is not — but NOTHING is
/// dropped from a figure. 61 issues yield 60 rows, a truncation count of 1,
/// and a delivery tile that still says 61.
#[test]
fn the_grid_is_capped_but_no_figure_loses_the_rows_it_omits() {
    // Descending cost, so the omitted row is the cheapest — and its $1 is
    // still inside the column total below.
    let rows = (1..=61)
        .map(|issue| {
            LedgerRow {
                issue,
                input: issue * 100_000,
                ..Default::default()
            }
            .json()
        })
        .collect::<Vec<_>>();
    let summary = fold_within(&rows, &[], Window::All, None);

    assert_eq!(summary.deliveries.len(), 60, "the LIST is capped");
    assert_eq!(summary.deliveries_truncated, 1);
    assert_eq!(summary.kpis.deliveries, 61, "the FIGURE counts them all");
    assert_eq!(
        summary.deliveries[0].issue, 61,
        "the cap keeps the costliest, not the first seen"
    );
    assert!(
        !summary.deliveries.iter().any(|d| d.issue == 1),
        "the cheapest row is the one omitted"
    );
    // 100k … 6.1M input at $15/1M = $15 * (0.1 + … + 6.1) = $15 * 189.1.
    let expected = 15.0 * (1..=61).map(|i| i as f64 * 0.1).sum::<f64>();
    assert!(
        (summary.overhead.deliveries_usd.unwrap() - expected).abs() < 1e-9,
        "the omitted row's cost is still in the column: {:?} vs {expected}",
        summary.overhead.deliveries_usd
    );
}

/// The identity PRD #355 states: `Σ deliveries + interactive + consolidation`
/// is the project total. One line of each, and the three must add up.
#[test]
fn the_overhead_lines_sum_to_the_project_total() {
    let rows = [
        LedgerRow {
            issue: 251,
            input: 1_000_000,
            ..Default::default()
        }
        .json(),
        LedgerRow {
            issue: 0,
            phase: "consolidate",
            outcome: "ok",
            input: 2_000_000,
            ..Default::default()
        }
        .json(),
    ];
    let interactive = [InteractiveRow {
        model: OPUS,
        tokens: Some((3_000_000, 0)),
        ..Default::default()
    }
    .json()];
    let summary = fold_within(&rows, &interactive, Window::All, None);

    let parts = summary.overhead.deliveries_usd.unwrap_or(0.0)
        + summary.overhead.interactive_usd.unwrap_or(0.0)
        + summary.overhead.consolidation_usd.unwrap_or(0.0);
    assert!(
        (summary.usd.unwrap() - parts).abs() < 1e-9,
        "the three lines must sum to the total: {:?} vs {parts}",
        summary.usd
    );
    assert_eq!(summary.overhead.deliveries_usd, Some(15.0));
    assert_eq!(summary.overhead.consolidation_usd, Some(30.0));
    assert_eq!(summary.overhead.interactive_usd, Some(45.0));
}
