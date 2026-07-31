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
}

/// The canonical `token_meter` split the operator already reads in the terminal
/// (`↑` input, `⚡` cache-read, `❄` cache-write, `↓` output), carrying the raw
/// counts, the one-line meter, and the same split as four addressable parts. The
/// `k`/`M` abbreviation and every percentage are formatted **here** so neither is
/// reimplemented in JavaScript (PRD #355) — the client renders, it does not
/// compute.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TokenMeter {
    pub input: u64,
    pub cache_read: u64,
    pub cache_creation: u64,
    pub output: u64,
    pub total: u64,
    /// The abbreviated total (`1.8M`).
    pub label: String,
    /// `↑12.4k ⚡184k ❄8.1k ↓3.2k`. The CLI's `fmt_meter`
    /// (`ralphy-cli/src/ui/render.rs`) is the source of this vocabulary; the two
    /// are mirrored deliberately rather than shared, because the daemon must not
    /// depend on the CLI. `meter_is_the_cli_vocabulary` pins the glyphs.
    pub meter: String,
    /// The same four counts, always all four and always in the canonical order,
    /// so the surface can lay the split out as rows without knowing the
    /// vocabulary. A zero part is KEPT — an absent `⚡` would read as "no cache
    /// column exists" rather than "nothing was reused".
    pub parts: Vec<MeterPart>,
}

/// One kind of token in the meter, carrying its own glyph, name and share so a
/// renderer needs no table of its own.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MeterPart {
    /// `input` | `cache_read` | `cache_creation` | `output`.
    pub key: String,
    /// `↑` | `⚡` | `❄` | `↓`.
    pub glyph: String,
    /// `input` | `cache read` | `cache write` | `output`.
    pub name: String,
    pub tokens: u64,
    /// The abbreviated count (`184k`).
    pub label: String,
    /// This kind's share of the meter's total, `0.0..=1.0` — for a bar's width.
    pub share: f64,
    /// The same share, rendered (`38.2%`).
    pub share_label: String,
}

/// The tokens the total could not price, split by cause. First-class, not a
/// footnote: the gap stays visible enough to get fixed, and the operator can
/// tell which part of it is worth working on.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Unpriced {
    /// Every unpriced token: the sum of [`Self::causes`].
    pub tokens: u64,
    /// The rendered volume (`1.66M`), or an empty string when nothing is unpriced.
    pub label: String,
    /// The share of all this project's tokens that went unpriced, `0.0..=1.0`.
    pub share: f64,
    /// The same share, rendered (`42.9%`).
    pub share_label: String,
    /// The complement: the tokens the total DID price, and their share. Carried
    /// so the coverage of the figure can be shown as a proportion rather than
    /// left to be inferred from its gap.
    pub priced: u64,
    pub priced_label: String,
    pub priced_share: f64,
    pub priced_share_label: String,
    /// The causes with volume in them, in the order the operator can act on
    /// them. A cause with nothing in it is DROPPED: a bucket at zero is not a
    /// finding, and three permanent zeroes teach the operator to stop reading the
    /// element that exists to be read.
    pub causes: Vec<UnpricedCause>,
    /// Interactive sessions whose vendor keeps no token count anywhere (`tokens:
    /// null`, e.g. Cursor — ADR-0042 D11). They carry real spend of unmeasurable
    /// size, so they are counted here rather than passing as zero.
    pub unmetered_sessions: u64,
}

/// One reason a token went unpriced. `key` is the closed vocabulary the surface
/// styles on; everything else is already rendered.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct UnpricedCause {
    /// `recoverable` | `no_price` | `lost`.
    pub key: String,
    pub tokens: u64,
    pub label: String,
    /// This cause's share of the UNPRICED volume (not of the project) — the
    /// question a cause row answers is "how much of the gap is this".
    pub share: f64,
    pub share_label: String,
}

/// Fold one project's usage into its priced summary. Pure: same inputs, same
/// document, always.
pub fn summarize(input: &SpendInput) -> SpendSummary {
    let mut meter = Counts::default();
    let mut usd = 0.0;
    let mut any_priced = false;
    let mut floor = false;
    let mut unpriced = Gap::default();

    for row in input.records {
        let Some(object) = row.as_object() else {
            continue;
        };
        if field(object, "project") != Some(input.project) {
            continue;
        }
        let tokens = ledger_tokens(object);
        add(&mut meter, &tokens);
        if tokens.total() == 0 {
            // A zero-token line carries no spend and no signal — pricing it
            // would force a spurious floor marker for nothing.
            continue;
        }
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
        match model {
            Some(model) => match input.prices.cost_usd(model, &tokens) {
                Some(cost) => {
                    usd += cost;
                    any_priced = true;
                }
                None => {
                    unpriced.no_price += tokens.total();
                    floor = true;
                }
            },
            // Unrecovered: the line never said which engine spent these tokens.
            None => {
                if session.is_some() {
                    unpriced.recoverable += tokens.total();
                } else {
                    unpriced.lost += tokens.total();
                }
                floor = true;
            }
        }
    }

    for row in input.interactive {
        let Some(object) = row.as_object() else {
            continue;
        };
        if field(object, "project") != Some(input.project) {
            continue;
        }
        let Some(tokens) = interactive_tokens(object) else {
            // The vendor keeps no count at all — unavailable, never zero.
            unpriced.unmetered_sessions += 1;
            floor = true;
            continue;
        };
        add(&mut meter, &tokens);
        // A `lower_bound` record's counts are a FLOOR, not the bill (ADR-0043
        // D10), so a total containing one is a floor however well it priced.
        if object
            .get("lower_bound")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
        {
            floor = true;
        }
        if tokens.total() == 0 {
            continue;
        }
        let model = field(object, "model")
            .filter(|m| !m.is_empty() && *m != UNKNOWN_MODEL)
            .or_else(|| {
                field(object, "session_id")
                    .and_then(|id| input.recovered.get(id).map(String::as_str))
            });
        match model.and_then(|model| input.prices.cost_usd(model, &tokens)) {
            Some(cost) => {
                usd += cost;
                any_priced = true;
            }
            None => {
                // An interactive record always carries its `session_id`, so an
                // unpriceable one is never *lost* — it is either a model the
                // table lacks or one recovery can still name.
                if model.is_some() {
                    unpriced.no_price += tokens.total();
                } else {
                    unpriced.recoverable += tokens.total();
                }
                floor = true;
            }
        }
    }

    let usd = any_priced.then_some(usd);
    SpendSummary {
        project: input.project.to_string(),
        total: fmt_total(usd, floor),
        usd,
        floor,
        tokens: meter.render(),
        unpriced: unpriced.render(meter.total),
    }
}

/// The four running token counts. Private: the document's [`TokenMeter`] is a
/// RENDERED view of this, and keeping the accumulator separate is what stops a
/// half-formatted struct from ever existing.
#[derive(Debug, Clone, Copy, Default)]
struct Counts {
    input: u64,
    cache_read: u64,
    cache_creation: u64,
    output: u64,
    total: u64,
}

impl Counts {
    /// The canonical order and vocabulary, in one place: `↑` input, `⚡`
    /// cache-read (hot reuse), `❄` cache-write (cold store), `↓` output.
    const KINDS: [(&'static str, &'static str, &'static str); 4] = [
        ("input", "↑", "input"),
        ("cache_read", "⚡", "cache read"),
        ("cache_creation", "❄", "cache write"),
        ("output", "↓", "output"),
    ];

    fn of(&self, key: &str) -> u64 {
        match key {
            "input" => self.input,
            "cache_read" => self.cache_read,
            "cache_creation" => self.cache_creation,
            _ => self.output,
        }
    }

    fn render(self) -> TokenMeter {
        let parts = Self::KINDS
            .iter()
            .map(|(key, glyph, name)| {
                let tokens = self.of(key);
                let share = share_of(tokens, self.total);
                MeterPart {
                    key: (*key).to_string(),
                    glyph: (*glyph).to_string(),
                    name: (*name).to_string(),
                    tokens,
                    label: fmt_tokens(tokens),
                    share,
                    share_label: fmt_share(share),
                }
            })
            .collect::<Vec<_>>();
        TokenMeter {
            input: self.input,
            cache_read: self.cache_read,
            cache_creation: self.cache_creation,
            output: self.output,
            total: self.total,
            label: fmt_tokens(self.total),
            meter: parts
                .iter()
                .map(|p| format!("{}{}", p.glyph, p.label))
                .collect::<Vec<_>>()
                .join(" "),
            parts,
        }
    }
}

/// The running unpriced volume, by cause. Private for the same reason as
/// [`Counts`].
#[derive(Debug, Clone, Copy, Default)]
struct Gap {
    recoverable: u64,
    no_price: u64,
    lost: u64,
    unmetered_sessions: u64,
}

impl Gap {
    /// Ordered by what the operator can do about it: run recovery, add one line
    /// to `pricing.toml`, or nothing at all (ADR-0053 D4).
    const CAUSES: [&'static str; 3] = ["recoverable", "no_price", "lost"];

    fn of(&self, key: &str) -> u64 {
        match key {
            "recoverable" => self.recoverable,
            "no_price" => self.no_price,
            _ => self.lost,
        }
    }

    fn render(self, all_tokens: u64) -> Unpriced {
        let tokens = self.recoverable + self.no_price + self.lost;
        let share = share_of(tokens, all_tokens);
        let priced = all_tokens.saturating_sub(tokens);
        let priced_share = share_of(priced, all_tokens);
        Unpriced {
            tokens,
            label: volume_label(tokens),
            share,
            share_label: fmt_share(share),
            priced,
            priced_label: volume_label(priced),
            priced_share,
            priced_share_label: fmt_share(priced_share),
            causes: Self::CAUSES
                .iter()
                .filter(|key| self.of(key) > 0)
                .map(|key| {
                    let cause = self.of(key);
                    let share = share_of(cause, tokens);
                    UnpricedCause {
                        key: (*key).to_string(),
                        tokens: cause,
                        label: fmt_tokens(cause),
                        share,
                        share_label: fmt_share(share),
                    }
                })
                .collect(),
            unmetered_sessions: self.unmetered_sessions,
        }
    }
}

/// `part / whole` as `0.0..=1.0`, and `0.0` for an empty whole — a share of
/// nothing is not a share, and `NaN` would reach a bar's width.
fn share_of(part: u64, whole: u64) -> f64 {
    if whole == 0 {
        0.0
    } else {
        part as f64 / whole as f64
    }
}

/// A share as a percentage, rendered here rather than in the client: one decimal,
/// because the figure this surface exists to shame (44%) and the figure it should
/// reach (0.2%) must BOTH read, and an integer percent collapses the second.
fn fmt_share(share: f64) -> String {
    format!("{:.1}%", share * 100.0)
}

/// The project total, rendered: `~$?` when nothing priced (ADR-0034 D3 — `$0`
/// would be a lie that hides spend), else `$2,350.59`, with a trailing `+` when
/// the figure omits volume it could not price.
fn fmt_total(usd: Option<f64>, floor: bool) -> String {
    match usd {
        None => "~$?".to_string(),
        Some(value) => format!(
            "${}{}",
            group_thousands(value),
            if floor { "+" } else { "" }
        ),
    }
}

/// `2350.586` → `2,350.59`. Two decimals with `,` every three integer digits —
/// a project total reaches four and five figures, where an ungrouped run of
/// digits stops being readable at a glance.
fn group_thousands(value: f64) -> String {
    let fixed = format!("{value:.2}");
    let (whole, fraction) = fixed.split_once('.').unwrap_or((fixed.as_str(), "00"));
    let (sign, digits) = match whole.strip_prefix('-') {
        Some(rest) => ("-", rest),
        None => ("", whole),
    };
    let mut grouped = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(ch);
    }
    format!("{sign}{grouped}.{fraction}")
}

/// A token volume for the unpriced markers: the abbreviated count, or an empty
/// string for zero so a bucket with nothing in it renders no marker at all.
fn volume_label(tokens: u64) -> String {
    if tokens == 0 {
        String::new()
    } else {
        fmt_tokens(tokens)
    }
}

/// Format a token count compactly: `1.2M`, `8.4k`, or a bare `912` under a
/// thousand. Mirrors the CLI footer's `fmt_tokens` so the web and the terminal
/// abbreviate identically.
fn fmt_tokens(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}k", n as f64 / 1_000.0)
    } else {
        n.to_string()
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

fn add(meter: &mut Counts, tokens: &TokenCounts) {
    meter.input += tokens.input;
    meter.cache_read += tokens.cache_read;
    meter.cache_creation += tokens.cache_creation;
    meter.output += tokens.output;
    meter.total += tokens.total();
}

#[cfg(test)]
mod tests {
    use super::*;

    const OPUS: &str = "claude-opus-4-8";

    /// One inline-priced table: 15/75/1.5/18.75 per 1M, the ADR-0008 D8 oracle.
    /// Inline rather than `PriceTable::defaults()` so a seed refresh cannot move
    /// an arithmetic assertion out from under these tests.
    fn table() -> PriceTable {
        PriceTable::from_layers(
            BTreeMap::from([(
                OPUS.to_string(),
                ralphy_pricing::ModelPrice {
                    input: 15.0,
                    output: 75.0,
                    cache_read: 1.5,
                    cache_creation: 18.75,
                },
            )]),
            BTreeMap::new(),
            BTreeMap::new(),
            BTreeMap::new(),
        )
    }

    /// One ledger line. `session` is `None` for the pre-ADR-0033 shape, where the
    /// field is skipped entirely rather than written as `null`.
    fn row(model: &str, session: Option<&str>, input: u64, output: u64) -> serde_json::Value {
        let mut object = serde_json::json!({
            "project": "acme/widget",
            "issue": 251,
            "phase": "execute",
            "agent": "claude",
            "model": model,
            "outcome": "success",
            "tokens": { "input": input, "output": output, "cache_read": 0, "cache_creation": 0 },
            "ts": "2026-07-30T12:00:00+00:00",
        });
        if let Some(session) = session {
            object["session_id"] = serde_json::Value::String(session.to_string());
        }
        object
    }

    /// One unpriced cause's tokens by key, or `0` when the fold dropped it as
    /// empty — so a test can assert "nothing in this bucket" the same way it
    /// asserts a volume.
    fn cause(summary: &SpendSummary, key: &str) -> u64 {
        summary
            .unpriced
            .causes
            .iter()
            .find(|c| c.key == key)
            .map_or(0, |c| c.tokens)
    }

    fn cause_label(summary: &SpendSummary, key: &str) -> String {
        summary
            .unpriced
            .causes
            .iter()
            .find(|c| c.key == key)
            .map_or(String::new(), |c| c.label.clone())
    }

    fn summarize_rows(
        records: &[serde_json::Value],
        recovered: &BTreeMap<String, String>,
    ) -> SpendSummary {
        summarize(&SpendInput {
            records,
            interactive: &[],
            recovered,
            prices: &table(),
            project: "acme/widget",
        })
    }

    /// The headline rule: a priced project reads as an exact figure with the
    /// canonical meter beside it, and nothing is marked as a floor.
    #[test]
    fn a_fully_priced_project_totals_exactly_and_is_not_a_floor() {
        let rows = [
            row(OPUS, Some("s1"), 1_000_000, 1_000_000),
            row(OPUS, Some("s2"), 1_000_000, 0),
        ];
        let summary = summarize_rows(&rows, &BTreeMap::new());

        // 2M input @15 + 1M output @75 = 30 + 75.
        assert_eq!(summary.usd.map(|v| (v * 100.0).round()), Some(10_500.0));
        assert!(!summary.floor, "nothing was unpriceable");
        assert_eq!(summary.total, "$105.00");
        assert_eq!(summary.tokens.total, 3_000_000);
        assert_eq!(summary.tokens.meter, "↑2.0M ⚡0 ❄0 ↓1.0M");
        assert_eq!(summary.unpriced.tokens, 0);
        assert_eq!(summary.unpriced.label, "");
        assert!(
            summary.unpriced.causes.is_empty(),
            "an empty cause is dropped, not rendered as a permanent zero"
        );
        assert_eq!(summary.unpriced.priced_share_label, "100.0%");
    }

    /// The slice's marquee case (ADR-0034 D3 + ADR-0053 D4): an `unknown` model
    /// never prices to `$0` and never drops out — it makes the total a FLOOR and
    /// lands in the unpriced split, on the side its `session_id` decides.
    #[test]
    fn an_unknown_model_produces_a_floor_total_and_the_recoverable_lost_split() {
        let rows = [
            row(OPUS, Some("s1"), 1_000_000, 0),
            // Carries a session: a vendor store can still name its engine.
            row("unknown", Some("s2"), 400_000, 100_000),
            // No session at all: no key to any store, so it never comes back.
            row("unknown", None, 250_000, 0),
            // A REAL model the table does not know — neither recoverable nor lost.
            row("big-pickle", Some("s4"), 60_000, 0),
        ];
        let summary = summarize_rows(&rows, &BTreeMap::new());

        assert!(summary.floor, "unpriceable volume makes the total a floor");
        assert_eq!(summary.total, "$15.00+", "the floor carries its `+`");
        assert_eq!(summary.usd, Some(15.0), "only the priced share is summed");
        assert_eq!(cause(&summary, "recoverable"), 500_000);
        assert_eq!(cause(&summary, "lost"), 250_000);
        assert_eq!(cause(&summary, "no_price"), 60_000);
        assert_eq!(summary.unpriced.tokens, 810_000);
        assert_eq!(cause_label(&summary, "recoverable"), "500.0k");
        assert_eq!(cause_label(&summary, "lost"), "250.0k");
        assert_eq!(cause_label(&summary, "no_price"), "60.0k");
        // The causes are ordered by what the operator can DO about them, and a
        // cause's share is of the GAP — the question a cause row answers.
        assert_eq!(
            summary
                .unpriced
                .causes
                .iter()
                .map(|c| c.key.as_str())
                .collect::<Vec<_>>(),
            ["recoverable", "no_price", "lost"]
        );
        assert_eq!(summary.unpriced.causes[2].share_label, "30.9%");
        // The priced complement is carried, so coverage is shown, not inferred.
        assert_eq!(summary.unpriced.priced, 1_000_000);
        assert_eq!(summary.unpriced.priced_label, "1.0M");
        // The unpriced tokens stay in the meter: they were spent either way.
        assert_eq!(summary.tokens.total, 1_810_000);
        assert!(
            (summary.unpriced.share - 810_000.0 / 1_810_000.0).abs() < 1e-9,
            "share is of ALL tokens, got {}",
            summary.unpriced.share
        );
    }

    /// Recovery is applied by the FOLD (ADR-0053 D2 — a projection, never a
    /// rewrite): the same `unknown` line prices exactly once its `session_id` is
    /// in the map, and stops being reported as a gap.
    #[test]
    fn a_recovered_model_prices_and_leaves_the_unpriced_split() {
        let rows = [row("unknown", Some("s2"), 1_000_000, 0)];
        let map = BTreeMap::from([("s2".to_string(), OPUS.to_string())]);

        let before = summarize_rows(&rows, &BTreeMap::new());
        assert_eq!(before.total, "~$?", "nothing priced is never `$0`");
        assert_eq!(before.usd, None);
        assert_eq!(cause(&before, "recoverable"), 1_000_000);

        let after = summarize_rows(&rows, &map);
        assert_eq!(after.usd, Some(15.0));
        assert_eq!(after.total, "$15.00");
        assert!(!after.floor, "the gap closed, so the total is exact");
        assert_eq!(after.unpriced.tokens, 0);
    }

    /// The tab is scoped to the OPEN project, so another project's spend must not
    /// leak into the figure — through either input.
    #[test]
    fn another_projects_rows_are_not_in_the_total() {
        let mut other = row(OPUS, Some("s9"), 5_000_000, 0);
        other["project"] = serde_json::Value::String("acme/other".into());
        let rows = [row(OPUS, Some("s1"), 1_000_000, 0), other];
        let interactive = [serde_json::json!({
            "agent": "claude",
            "model": OPUS,
            "session_id": "i9",
            "project": "acme/other",
            "tokens": { "input": 9_000_000, "output": 0, "cache_read": 0, "cache_creation": 0 },
            "first_ts": "2026-07-30T10:00:00+00:00",
            "last_ts": "2026-07-30T11:00:00+00:00",
            "lower_bound": false,
        })];

        let summary = summarize(&SpendInput {
            records: &rows,
            interactive: &interactive,
            recovered: &BTreeMap::new(),
            prices: &table(),
            project: "acme/widget",
        });
        assert_eq!(summary.usd, Some(15.0));
        assert_eq!(summary.tokens.total, 1_000_000);
    }

    /// Interactive usage is project-level overhead, so it IS in the project
    /// total (PRD #355: `Σ deliveries + interactive + consolidation`). Two of the
    /// scan's honesty flags ride with it: `tokens: null` means the vendor keeps no
    /// count (never zero), and `lower_bound` means the counts are a floor.
    #[test]
    fn interactive_overhead_joins_the_total_and_carries_the_scans_caveats() {
        let interactive = [
            serde_json::json!({
                "agent": "claude", "model": OPUS, "session_id": "i1", "project": "acme/widget",
                "tokens": { "input": 1_000_000, "output": 0, "cache_read": 0, "cache_creation": 0 },
                "first_ts": "2026-07-30T10:00:00+00:00", "last_ts": "2026-07-30T11:00:00+00:00",
                "lower_bound": false,
            }),
            // Cursor: the vendor records no count anywhere (ADR-0042 D11).
            serde_json::json!({
                "agent": "cursor", "model": "composer-2.5", "session_id": "i2",
                "project": "acme/widget", "tokens": serde_json::Value::Null,
                "first_ts": "2026-07-30T10:00:00+00:00", "last_ts": "2026-07-30T11:00:00+00:00",
                "lower_bound": false,
            }),
        ];
        let summary = summarize(&SpendInput {
            records: &[],
            interactive: &interactive,
            recovered: &BTreeMap::new(),
            prices: &table(),
            project: "acme/widget",
        });

        assert_eq!(summary.usd, Some(15.0), "interactive spend is in the total");
        assert_eq!(summary.unpriced.unmetered_sessions, 1);
        assert!(
            summary.floor,
            "a session the vendor never counted makes the total a floor"
        );
        assert_eq!(summary.total, "$15.00+");
    }

    /// A `lower_bound` record's counts are a FLOOR, not the bill (ADR-0043 D10),
    /// so the total inherits that even when every model priced.
    #[test]
    fn a_lower_bound_record_makes_a_fully_priced_total_a_floor() {
        let interactive = [serde_json::json!({
            "agent": "gemini", "model": OPUS, "session_id": "i1", "project": "acme/widget",
            "tokens": { "input": 1_000_000, "output": 0, "cache_read": 0, "cache_creation": 0 },
            "first_ts": "2026-07-30T10:00:00+00:00", "last_ts": "2026-07-30T11:00:00+00:00",
            "lower_bound": true,
        })];
        let summary = summarize(&SpendInput {
            records: &[],
            interactive: &interactive,
            recovered: &BTreeMap::new(),
            prices: &table(),
            project: "acme/widget",
        });
        assert!(summary.floor);
        assert_eq!(summary.total, "$15.00+");
        assert_eq!(
            summary.unpriced.tokens, 0,
            "the counts are known, just partial"
        );
    }

    /// A project with nothing in it must read as an honest gap, not as `$0.00`
    /// (which claims a total nobody measured) and not as a floor.
    #[test]
    fn an_empty_project_reads_as_a_gap_not_as_zero() {
        let summary = summarize_rows(&[], &BTreeMap::new());
        assert_eq!(summary.usd, None);
        assert_eq!(summary.total, "~$?");
        assert_eq!(summary.tokens.total, 0);
        assert_eq!(summary.tokens.meter, "↑0 ⚡0 ❄0 ↓0");
        assert!(!summary.floor);
        assert!((summary.unpriced.share - 0.0).abs() < f64::EPSILON);
    }

    /// The ledger is append-only and best-effort: a line missing its `tokens`
    /// object, or one that is not an object at all, is skipped rather than fatal.
    #[test]
    fn malformed_rows_are_skipped_not_fatal() {
        let rows = [
            serde_json::json!("not an object"),
            serde_json::json!({ "project": "acme/widget", "model": OPUS }),
            row(OPUS, Some("s1"), 1_000_000, 0),
        ];
        let summary = summarize_rows(&rows, &BTreeMap::new());
        assert_eq!(summary.usd, Some(15.0));
        assert!(!summary.floor, "a zero-token line forces no floor marker");
    }

    /// A four-figure total is where an ungrouped run of digits stops being
    /// readable — the separator is the reason this renders server-side.
    #[test]
    fn a_large_floor_total_reads_as_grouped_digits() {
        assert_eq!(fmt_total(Some(2_350.586), true), "$2,350.59+");
        assert_eq!(fmt_total(Some(1_234_567.8), false), "$1,234,567.80");
        assert_eq!(fmt_total(Some(0.5), false), "$0.50");
        assert_eq!(fmt_total(None, true), "~$?");
    }

    /// Half this slice's honesty rules live in HTML/JS that no Rust gate
    /// compiles: deleting the floor note, or reintroducing a `k`/`M` abbreviation
    /// in JavaScript, leaves the suite green while the operator reads a floor as
    /// a total. Pins both, the way `usage.rs`'s lower-bound test pins its label.
    #[test]
    fn the_spend_tab_renders_the_servers_figures_and_formats_none_of_its_own() {
        let js = include_str!("../assets/ui/wb-spend.js");
        assert!(
            !js.contains("1e6") && !js.contains("1000000") && !js.contains("toFixed"),
            "wb-spend.js must neither abbreviate a token count nor round a \
             percentage — the daemon renders both"
        );
        assert!(
            js.contains("c.label") && js.contains("c.share_label") && js.contains("p.label"),
            "wb-spend.js must read the daemon's pre-rendered labels and shares"
        );
        for cause in ["recoverable", "no_price", "lost"] {
            assert!(
                js.contains(&format!("{cause}: {{")),
                "wb-spend.js must keep the copy for the `{cause}` unpriced cause"
            );
        }

        let html = include_str!("../assets/ui/index.html");
        assert!(
            html.contains("spendView().total") && html.contains("spendView().meter"),
            "index.html must render the daemon's total and meter verbatim"
        );
        assert!(
            html.contains("spendView().floorNote"),
            "index.html must say IN WORDS what the floor marker means — the `+` \
             the daemon appends teaches nobody on its own"
        );
        assert!(
            html.contains("spendView().unpriced.any") && html.contains("c.hint"),
            "the unpriced volume must be a first-class element with its causes \
             explained on screen, not a footnote behind a tooltip"
        );
        assert!(
            html.contains("openSpend()") && html.contains("data-lucide=\"coins\""),
            "the icon rail must carry the Spend button"
        );
        // The shape PRD #355 fixed: the total is the FIRST OF FIVE TILES, not a
        // band of its own. #359 appends the other four into this same strip, so
        // a slice that quietly turns the tile back into a full-width hero costs
        // that issue a rewrite instead of an append.
        assert!(
            html.contains("class=\"kpi-strip\"") && html.contains("class=\"kpi kpi-primary\""),
            "the total must sit in the primary tile of a KPI strip — PRD #355's \
             \"five tiles carry the executive read\", whose other four are #359"
        );
    }

    /// The meter is the terminal's vocabulary, spelled once on the server. If a
    /// glyph or an abbreviation moves here, the web and the CLI stop agreeing —
    /// and the client is forbidden from reimplementing either.
    #[test]
    fn meter_is_the_cli_vocabulary() {
        assert_eq!(fmt_tokens(912), "912");
        assert_eq!(fmt_tokens(8_400), "8.4k");
        assert_eq!(fmt_tokens(1_240_000), "1.2M");

        let summary = summarize(&SpendInput {
            records: &[row(OPUS, Some("s1"), 12_400, 3_200)],
            interactive: &[],
            recovered: &BTreeMap::new(),
            prices: &table(),
            project: "acme/widget",
        });
        assert_eq!(
            summary.tokens.meter, "↑12.4k ⚡0 ❄0 ↓3.2k",
            "↑ input, ⚡ cache-read, ❄ cache-write, ↓ output"
        );
    }
}
