//! Test-only fixtures shared by the fold's sibling modules. The arithmetic
//! oracle lives HERE, in one place: every test in `spend/` prices through the
//! same inline table, so a `pricing.toml` refresh can never move an assertion
//! out from under them.

use std::collections::BTreeMap;

use ralphy_pricing::PriceTable;

use super::{summarize, SpendInput, SpendSummary, Window};

pub(crate) const OPUS: &str = "claude-opus-4-8";
/// A SECOND priced engine, at the same rates, so a share between two models is
/// exercised without a second set of arithmetic to check by hand.
pub(crate) const SECOND: &str = "kimi-k3";
pub(crate) const PROJECT: &str = "acme/widget";

/// One inline-priced table: 15/75/1.5/18.75 per 1M, the ADR-0008 D8 oracle.
/// Inline rather than `PriceTable::defaults()` so a seed refresh cannot move an
/// arithmetic assertion out from under these tests.
pub(crate) fn table() -> PriceTable {
    let price = || ralphy_pricing::ModelPrice {
        input: 15.0,
        output: 75.0,
        cache_read: 1.5,
        cache_creation: 18.75,
    };
    PriceTable::from_layers(
        BTreeMap::from([(OPUS.to_string(), price()), (SECOND.to_string(), price())]),
        BTreeMap::new(),
        BTreeMap::new(),
        BTreeMap::new(),
    )
}

/// One ledger line, spelled by the fields a test actually cares about — the rest
/// come from [`Default`]. `session` is `None` for the pre-ADR-0033 shape, where
/// the field is skipped entirely rather than written as `null`.
pub(crate) struct LedgerRow<'a> {
    pub project: &'a str,
    pub issue: u64,
    pub phase: &'a str,
    pub model: &'a str,
    pub session: Option<&'a str>,
    pub outcome: &'a str,
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_creation: u64,
    pub ts: &'a str,
}

impl Default for LedgerRow<'_> {
    fn default() -> Self {
        Self {
            project: PROJECT,
            issue: 251,
            phase: "execute",
            model: OPUS,
            session: None,
            outcome: "done",
            input: 0,
            output: 0,
            cache_read: 0,
            cache_creation: 0,
            ts: "2026-07-30T12:00:00+00:00",
        }
    }
}

impl LedgerRow<'_> {
    pub fn json(&self) -> serde_json::Value {
        let mut object = serde_json::json!({
            "project": self.project,
            "issue": self.issue,
            "phase": self.phase,
            "agent": "claude",
            "model": self.model,
            "outcome": self.outcome,
            "tokens": {
                "input": self.input,
                "output": self.output,
                "cache_read": self.cache_read,
                "cache_creation": self.cache_creation,
            },
            "ts": self.ts,
        });
        if let Some(session) = self.session {
            object["session_id"] = serde_json::Value::String(session.to_string());
        }
        object
    }
}

/// One interactive record, as the usage scan produces it (ADR-0033 §2).
pub(crate) struct InteractiveRow<'a> {
    pub project: &'a str,
    pub agent: &'a str,
    pub model: &'a str,
    pub session: &'a str,
    /// `None` renders as `tokens: null` — the vendor keeps no count anywhere.
    pub tokens: Option<(u64, u64)>,
    pub first_ts: &'a str,
    pub last_ts: &'a str,
    pub lower_bound: bool,
}

impl Default for InteractiveRow<'_> {
    fn default() -> Self {
        Self {
            project: PROJECT,
            agent: "claude",
            model: OPUS,
            session: "i1",
            tokens: Some((0, 0)),
            first_ts: "2026-07-30T10:00:00+00:00",
            last_ts: "2026-07-30T11:00:00+00:00",
            lower_bound: false,
        }
    }
}

impl InteractiveRow<'_> {
    pub fn json(&self) -> serde_json::Value {
        let tokens = match self.tokens {
            None => serde_json::Value::Null,
            Some((input, output)) => serde_json::json!({
                "input": input, "output": output, "cache_read": 0, "cache_creation": 0,
            }),
        };
        serde_json::json!({
            "project": self.project,
            "agent": self.agent,
            "model": self.model,
            "session_id": self.session,
            "tokens": tokens,
            "first_ts": self.first_ts,
            "last_ts": self.last_ts,
            "lower_bound": self.lower_bound,
        })
    }
}

/// Fold ledger rows and interactive records inside one window.
pub(crate) fn fold_within(
    records: &[serde_json::Value],
    interactive: &[serde_json::Value],
    window: Window,
    since: Option<&str>,
) -> SpendSummary {
    summarize(&SpendInput {
        records,
        interactive,
        recovered: &BTreeMap::new(),
        prices: &table(),
        project: PROJECT,
        window,
        since,
    })
}
