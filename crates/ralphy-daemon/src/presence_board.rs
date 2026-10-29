//! Whether the operator is present at one of this daemon's pages
//! (docs/adr/0085 D14). Each page reports on its `/ws` socket whether its tab
//! is visible and when the operator's last key, click or touch in it was; the
//! board keeps one entry per open socket, in memory only. The operator is
//! present when one page is visible and its last input is less than
//! [`WINDOW`] old. No page, a hidden page, or an old input is "not present".
//! A peer's pages count too: a main daemon reads the peer's `present` on the
//! peer's own attention list.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use serde::Deserialize as _;

/// How recent the last input of a visible page must be for the operator to
/// be present.
pub(crate) const WINDOW: Duration = Duration::from_secs(5 * 60);

/// The longest origin a report may carry. A browser origin is a scheme, a
/// host and a port: a DNS name has at most 253 bytes.
pub(crate) const MAX_ORIGIN_BYTES: usize = 256;

/// The largest message `/ws` reads: one report, whose origin is the only
/// field of free length (ADR-0072 D11). The rest is three short fields and
/// the frame's JSON.
pub(crate) const MAX_REPORT_BYTES: usize = MAX_ORIGIN_BYTES + 256;

/// The board, shared by the `/ws` sockets and the attention tick.
pub(crate) type Shared = Arc<Mutex<PresenceBoard>>;

/// What one page said last.
#[derive(Debug, Default, Clone)]
struct Page {
    visible: bool,
    /// The daemon's time of the page's last input, or `None` before one.
    last_input: Option<SystemTime>,
}

/// One `presence.report` payload. The two times are the page's own clock in
/// milliseconds, so only their difference is used: the clock of a phone and
/// the clock of the daemon may differ.
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Report {
    visible: bool,
    last_input: Option<f64>,
    now: f64,
    origin: String,
}

/// The pages of this daemon, by `/ws` connection.
#[derive(Debug, Default)]
pub(crate) struct PresenceBoard {
    next: u64,
    pages: HashMap<u64, Page>,
    last_origin: Option<String>,
}

impl PresenceBoard {
    /// A new socket: an entry that is not present until it reports.
    pub(crate) fn open(&mut self) -> u64 {
        self.next += 1;
        self.pages.insert(self.next, Page::default());
        self.next
    }

    /// The socket `conn` closed: its entry goes. The last origin stays, so a
    /// message still links to the page the operator used last.
    pub(crate) fn close(&mut self, conn: u64) {
        self.pages.remove(&conn);
    }

    /// Take the report `payload` of `conn`, received at `received`. A report
    /// that does not parse, has a bad origin or names no open socket changes
    /// nothing, and the answer is `false`.
    pub(crate) fn report(
        &mut self,
        conn: u64,
        payload: &serde_json::Value,
        received: SystemTime,
    ) -> bool {
        let Ok(report) = Report::deserialize(payload) else {
            return false;
        };
        if !origin_is_valid(&report.origin) || !report.now.is_finite() {
            return false;
        }
        let last_input = match report.last_input {
            None => None,
            Some(at) if at.is_finite() => {
                let idle = Duration::from_millis((report.now - at).max(0.0) as u64);
                Some(received.checked_sub(idle).unwrap_or(SystemTime::UNIX_EPOCH))
            }
            Some(_) => return false,
        };
        let Some(page) = self.pages.get_mut(&conn) else {
            return false;
        };
        page.visible = report.visible;
        page.last_input = last_input;
        self.last_origin = Some(report.origin);
        true
    }

    /// Whether the operator is present at `now`: one visible page whose last
    /// input is less than [`WINDOW`] old.
    pub(crate) fn present(&self, now: SystemTime) -> bool {
        self.pages.values().any(|page| {
            // An input the daemon's clock puts after `now` is a fresh one.
            page.visible
                && page
                    .last_input
                    .is_some_and(|at| now.duration_since(at).unwrap_or(Duration::ZERO) < WINDOW)
        })
    }

    /// Whether the operator is present at `now` at a page of this daemon or
    /// at a page of a peer: `peers_present` holds what each peer said.
    pub(crate) fn present_with(&self, peers_present: &[bool], now: SystemTime) -> bool {
        self.present(now) || peers_present.iter().any(|present| *present)
    }

    /// The origin of the last report: where a link to a console opens.
    pub(crate) fn last_origin(&self) -> Option<String> {
        self.last_origin.clone()
    }
}

/// Whether the operator is present now at a page of this daemon. A poisoned
/// board is logged and reads as "not present".
pub(crate) fn present_now(board: &Shared) -> bool {
    match board.lock() {
        Ok(board) => board.present(SystemTime::now()),
        Err(e) => {
            tracing::warn!(error = %e, "the presence board is poisoned");
            false
        }
    }
}

/// Whether `origin` is a browser origin: `http://` or `https://`, then a
/// host and an optional port, with no path. It goes into a link the operator
/// opens, so nothing else is taken.
fn origin_is_valid(origin: &str) -> bool {
    let rest = origin
        .strip_prefix("https://")
        .or_else(|| origin.strip_prefix("http://"));
    let Some(host) = rest else {
        return false;
    };
    origin.len() <= MAX_ORIGIN_BYTES
        && !host.is_empty()
        && host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b':' | b'[' | b']'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn at(secs: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000 + secs)
    }

    /// A report from a page whose clock is far from the daemon's: the input
    /// happened `idle_secs` before the page sent it.
    fn report(visible: bool, idle_secs: Option<u64>) -> serde_json::Value {
        let now = 9_000_000_000.0;
        json!({
            "visible": visible,
            "lastInput": idle_secs.map(|s| now - (s * 1000) as f64),
            "now": now,
            "origin": "https://ralphy.example:8443",
        })
    }

    #[test]
    fn presence_follows_the_five_minute_rule() {
        let malformed = [
            json!({ "visible": false }),
            json!({ "visible": false, "lastInput": null, "now": 1.0, "origin": "javascript:alert(1)" }),
            json!({ "visible": false, "lastInput": null, "now": 1.0, "origin": "https://a.example/x?y" }),
            json!({ "visible": false, "lastInput": null, "now": 1.0, "origin": "https://a.example", "more": 1 }),
            json!("hidden"),
        ];
        // (case, the reports the page sends at 0 s, checked at, present)
        let rows: [(&str, Vec<serde_json::Value>, u64, bool); 5] = [
            (
                "a visible page, input 4 min 59 s old",
                vec![report(true, Some(0))],
                299,
                true,
            ),
            (
                "a visible page, input 5 min 1 s old",
                vec![report(true, Some(0))],
                301,
                false,
            ),
            (
                "a hidden page, input 10 s old",
                vec![report(false, Some(0))],
                10,
                false,
            ),
            ("no page", vec![], 10, false),
            (
                "a malformed report after a visible one",
                std::iter::once(report(true, Some(0)))
                    .chain(malformed.iter().cloned())
                    .collect(),
                10,
                true,
            ),
        ];
        for (case, sent, check_at, want) in rows {
            let mut board = PresenceBoard::default();
            if let Some((first, rest)) = sent.split_first() {
                let conn = board.open();
                assert!(
                    board.report(conn, first, at(0)),
                    "{case}: the report is taken"
                );
                for payload in rest {
                    assert!(
                        !board.report(conn, payload, at(0)),
                        "{case}: refused {payload}"
                    );
                }
                assert_eq!(
                    board.last_origin().as_deref(),
                    Some("https://ralphy.example:8443"),
                    "{case}"
                );
            }
            assert_eq!(board.present(at(check_at)), want, "{case}");
        }
    }
}
