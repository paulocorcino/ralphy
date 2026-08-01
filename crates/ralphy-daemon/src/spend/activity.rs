//! The activity band: spend and deliveries on ONE timeline, so a spike in cost
//! can be read against what it bought on the same day.
//!
//! Both series are bar heights relative to their own peak day, rendered here as
//! `0.0..=1.0`, because the two have no common unit — dollars and issues cannot
//! share an axis, but each can share a baseline.

use std::collections::{BTreeMap, BTreeSet};

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

use super::format::{fmt_total, share_of};
use super::period::Window;
use super::rows::Classified;

/// One day of the band.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ActivityDay {
    /// The UTC civil date, `2026-07-30`.
    pub date: String,
    pub usd: f64,
    /// The day's spend, rendered — `$45.00`, `$45.00+`, or `~$?` when the day's
    /// whole volume was unpriceable.
    pub usd_label: String,
    /// The day's spend as a fraction of the PEAK day's, for a bar height.
    pub usd_share: f64,
    /// Distinct non-zero issues touched that day.
    pub deliveries: u64,
    pub deliveries_share: f64,
    /// `true` when the day's spend omits volume it could not price.
    pub floor: bool,
}

#[derive(Default)]
struct Day {
    usd: f64,
    priced: bool,
    floor: bool,
    issues: BTreeSet<u64>,
}

/// Bucket the classified rows by UTC date. A bounded window emits EVERY one of
/// its days, zero-filled, so the timeline has no gaps and a quiet week reads as
/// quiet rather than as missing; `all` emits only days with activity, so a long
/// history cannot make the response unbounded.
pub(crate) fn fold(
    classified: &Classified,
    window: Window,
    since: Option<&str>,
) -> Vec<ActivityDay> {
    let mut days: BTreeMap<String, Day> = BTreeMap::new();
    for date in window_days(window, since) {
        days.entry(date).or_default();
    }
    for row in &classified.rows {
        if row.date.is_empty() {
            continue;
        }
        let day = days.entry(row.date.to_string()).or_default();
        match row.usd {
            Some(cost) => {
                day.usd += cost;
                day.priced = true;
            }
            None => day.floor |= row.cause.is_some(),
        }
        if let Some(issue) = row.issue() {
            day.issues.insert(issue);
        }
    }

    let peak_usd = days.values().map(|d| d.usd).fold(0.0_f64, f64::max);
    let peak_deliveries = days.values().map(|d| d.issues.len()).max().unwrap_or(0) as u64;
    days.into_iter()
        .map(|(date, day)| {
            let deliveries = day.issues.len() as u64;
            ActivityDay {
                date,
                usd: day.usd,
                // A day whose whole volume was unpriceable reads `~$?`, never
                // `$0.00`: the bar is empty because nobody could price it, not
                // because nothing was spent.
                usd_label: fmt_total(day.priced.then_some(day.usd), day.floor),
                usd_share: if peak_usd > 0.0 {
                    day.usd / peak_usd
                } else {
                    0.0
                },
                deliveries,
                deliveries_share: share_of(deliveries, peak_deliveries),
                floor: day.floor,
            }
        })
        .collect()
}

/// Every civil date a bounded window covers, starting at `since`'s own date.
/// An unbounded window seeds nothing — only the days with activity appear.
fn window_days(window: Window, since: Option<&str>) -> Vec<String> {
    let (Some(days), Some(since)) = (window.days(), since) else {
        return Vec::new();
    };
    let Ok(start) = NaiveDate::parse_from_str(&since[..since.len().min(10)], "%Y-%m-%d") else {
        return Vec::new();
    };
    start
        .iter_days()
        .take(days as usize)
        .map(|date| date.format("%Y-%m-%d").to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::super::fixtures::{fold_within, LedgerRow};
    use super::super::Window;

    /// The band's whole point: cost and deliveries on the SAME days, so a spike
    /// can be read against what it bought.
    #[test]
    fn the_band_carries_spend_and_deliveries_on_one_timeline() {
        let rows = [
            LedgerRow {
                issue: 1,
                ts: "2026-07-29T09:00:00+00:00",
                input: 1_000_000,
                ..Default::default()
            }
            .json(),
            LedgerRow {
                issue: 2,
                ts: "2026-07-30T09:00:00+00:00",
                input: 2_000_000,
                ..Default::default()
            }
            .json(),
            LedgerRow {
                issue: 3,
                ts: "2026-07-30T18:00:00+00:00",
                input: 1_000_000,
                ..Default::default()
            }
            .json(),
        ];
        let summary = fold_within(&rows, &[], Window::All, None);

        assert_eq!(
            summary
                .activity
                .iter()
                .map(|d| (d.date.as_str(), d.usd, d.deliveries))
                .collect::<Vec<_>>(),
            [("2026-07-29", 15.0, 1), ("2026-07-30", 45.0, 2)],
            "one bucket per day, in date order"
        );
        // The bars are relative to the peak day, which anchors at full height.
        assert_eq!(summary.activity[1].usd_share, 1.0);
        assert_eq!(summary.activity[1].deliveries_share, 1.0);
        assert!((summary.activity[0].usd_share - 1.0 / 3.0).abs() < 1e-9);
        assert_eq!(summary.activity[0].usd_label, "$15.00");
    }

    /// A bounded window emits every one of its days: a quiet Tuesday must read
    /// as quiet, not vanish and let two busy days sit side by side.
    #[test]
    fn a_bounded_window_zero_fills_its_quiet_days() {
        let rows = [LedgerRow {
            issue: 1,
            ts: "2026-07-16T09:00:00+00:00",
            input: 1_000_000,
            ..Default::default()
        }
        .json()];
        let summary = fold_within(&rows, &[], Window::Week, Some("2026-07-15T00:00:00+00:00"));

        assert_eq!(summary.activity.len(), 7, "every day of the window");
        assert_eq!(
            summary.activity.iter().filter(|d| d.usd > 0.0).count(),
            1,
            "exactly one active day"
        );
        assert_eq!(summary.activity[0].date, "2026-07-15");
        assert_eq!(summary.activity[6].date, "2026-07-21");
        // A quiet day is a gap, not a zero-dollar claim.
        assert_eq!(summary.activity[0].usd_label, "~$?");
        assert_eq!(summary.activity[1].usd_label, "$15.00");
    }
}
