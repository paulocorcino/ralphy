//! The window the Spend view is scoped to. A closed vocabulary, because a
//! period is a fixed set of values and a `String` would let an unrecognized one
//! silently mean "all time" — the exact misread this surface exists to prevent.
//!
//! The split is deliberate: [`Window`] is the vocabulary and lives here;
//! [`Period`] is the rendered form the document carries, with the `since`
//! instant the ROUTE computed from the clock. The fold never asks what time it
//! is — it only compares timestamps against the `since` it was handed.

use serde::{Deserialize, Serialize};

/// The four windows the surface offers, and nothing else.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Window {
    #[default]
    All,
    Week,
    Month,
    Quarter,
}

impl Window {
    /// The query key, the label the client renders, and the window's length in
    /// days — one table, so a new window cannot arrive half-defined.
    const VOCABULARY: [(Window, &'static str, &'static str, Option<u32>); 4] = [
        (Window::All, "all", "all time", None),
        (Window::Week, "7d", "last 7 days", Some(7)),
        (Window::Month, "30d", "last 30 days", Some(30)),
        (Window::Quarter, "90d", "last 90 days", Some(90)),
    ];

    /// The window a query key names, or `None` when the key is not in the
    /// vocabulary. An unknown key is a REFUSAL upstream, never a fallback to
    /// `all`: all-time figures under a window label are a lie about the period.
    pub fn parse(key: &str) -> Option<Self> {
        Self::VOCABULARY
            .iter()
            .find(|(_, k, _, _)| *k == key)
            .map(|(window, _, _, _)| *window)
    }

    fn entry(self) -> &'static (Window, &'static str, &'static str, Option<u32>) {
        Self::VOCABULARY
            .iter()
            .find(|(window, _, _, _)| *window == self)
            // Every variant is in the table; a miss would be a bug in it.
            .expect("every Window variant has a vocabulary entry")
    }

    pub fn key(self) -> &'static str {
        self.entry().1
    }

    pub fn label(self) -> &'static str {
        self.entry().2
    }

    /// The window's length in days, or `None` for the unbounded `all`.
    pub fn days(self) -> Option<u32> {
        self.entry().3
    }

    /// The rendered form for the document, carrying the `since` instant the
    /// caller derived from the clock.
    pub fn render(self, since: Option<String>) -> Period {
        Period {
            key: self.key().to_string(),
            label: self.label().to_string(),
            since,
        }
    }
}

/// The window as the document carries it: the key the client sends back, the
/// label it renders, and the RFC3339 instant the figures start at (`None` for
/// all time).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Period {
    /// `all` | `7d` | `30d` | `90d`.
    pub key: String,
    /// `all time` | `last 7 days` | …
    pub label: String,
    /// The inclusive lower bound, RFC3339, or `None` for all time.
    pub since: Option<String>,
}

impl Default for Period {
    fn default() -> Self {
        Window::All.render(None)
    }
}

impl Period {
    /// The window this period was built from — the key is the identity, so a
    /// document that came back over the wire re-reads as the same vocabulary.
    pub fn window(&self) -> Window {
        Window::parse(&self.key).unwrap_or_default()
    }
}

/// Is `ts` inside the window? The boundary instant is IN (`>= since`), and a row
/// with no timestamp at all is KEPT — the ledger is best-effort, and dropping a
/// line for a missing field would silently shrink the total.
///
/// The comparison is lexicographic on RFC3339 text, which orders correctly for
/// the shape both sides use: the runner writes `chrono::Utc::now().to_rfc3339()`
/// (`+00:00`) and the route derives `since` the same way.
pub(crate) fn in_window(ts: Option<&str>, since: Option<&str>) -> bool {
    match (ts, since) {
        (_, None) => true,
        (None, Some(_)) => true,
        (Some(ts), Some(since)) => ts >= since,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spend::fixtures::{fold_within, InteractiveRow, LedgerRow};

    const SINCE: &str = "2026-07-15T00:00:00+00:00";

    /// The rule the whole period control rests on: `since` is INCLUSIVE. A row
    /// stamped exactly at the boundary is inside the window; one second earlier
    /// is outside, and its tokens leave every figure — the fold does not merely
    /// hide it from a list.
    #[test]
    fn period_filtering_includes_the_boundary_instant() {
        let rows = [
            LedgerRow {
                issue: 1,
                ts: "2026-07-14T23:59:59+00:00",
                input: 1_000_000,
                ..Default::default()
            }
            .json(),
            LedgerRow {
                issue: 2,
                ts: SINCE,
                input: 1_000_000,
                ..Default::default()
            }
            .json(),
            LedgerRow {
                issue: 3,
                ts: "2026-07-16T00:00:00+00:00",
                input: 1_000_000,
                ..Default::default()
            }
            .json(),
        ];

        let within = fold_within(&rows, &[], Window::Week, Some(SINCE));
        assert_eq!(
            within.tokens.total, 2_000_000,
            "the row one second before `since` is out of the window entirely"
        );
        assert_eq!(within.usd, Some(30.0));
        assert_eq!(within.period.key, "7d");
        assert_eq!(within.period.since.as_deref(), Some(SINCE));

        // Negative control: with no window, the same rows all count.
        let all = fold_within(&rows, &[], Window::All, None);
        assert_eq!(all.tokens.total, 3_000_000);
        assert_eq!(all.usd, Some(45.0));
        assert_eq!(all.period.key, "all");
        assert_eq!(all.period.since, None);
    }

    /// An interactive session is placed by its most recent activity, so a
    /// long-running session that touched the window is IN even though it started
    /// before it — and one that went quiet before the window is out.
    #[test]
    fn an_interactive_session_is_placed_by_its_last_activity() {
        let records = [
            InteractiveRow {
                session: "old",
                first_ts: "2026-07-01T00:00:00+00:00",
                last_ts: "2026-07-14T23:59:59+00:00",
                tokens: Some((1_000_000, 0)),
                ..Default::default()
            }
            .json(),
            InteractiveRow {
                session: "live",
                first_ts: "2026-07-01T00:00:00+00:00",
                last_ts: "2026-07-16T00:00:00+00:00",
                tokens: Some((1_000_000, 0)),
                ..Default::default()
            }
            .json(),
        ];
        let summary = fold_within(&[], &records, Window::Week, Some(SINCE));
        assert_eq!(summary.tokens.total, 1_000_000, "only the live session");
        assert_eq!(summary.usd, Some(15.0));
    }

    #[test]
    fn the_period_vocabulary_is_closed() {
        assert_eq!(Window::parse("7d"), Some(Window::Week));
        assert_eq!(Window::parse("all"), Some(Window::All));
        assert_eq!(Window::parse("fortnight"), None, "not in the vocabulary");
        assert_eq!(Window::parse(""), None);
        assert_eq!(Window::Quarter.days(), Some(90));
        assert_eq!(Window::All.days(), None);
        assert_eq!(Window::Month.render(None).label, "last 30 days");
    }

    /// A timestamp EQUAL to `since` is inside the window, and a row that carries
    /// no timestamp is never dropped by the filter.
    #[test]
    fn the_boundary_instant_is_inside_the_window() {
        let since = Some("2026-07-15T00:00:00+00:00");
        assert!(!in_window(Some("2026-07-14T23:59:59+00:00"), since));
        assert!(in_window(Some("2026-07-15T00:00:00+00:00"), since));
        assert!(in_window(Some("2026-07-16T00:00:00+00:00"), since));
        assert!(in_window(None, since), "a row with no ts is kept");
        assert!(in_window(Some("2020-01-01T00:00:00+00:00"), None));
    }
}
