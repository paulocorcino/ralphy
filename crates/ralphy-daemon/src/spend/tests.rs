use super::fixtures::{table, InteractiveRow, LedgerRow, OPUS};
use super::format::fmt_tokens;
use super::*;

/// The annotator is the Ledger grid's whole filter: a row it fails to mark is
/// a gap the operator clicked through to see and does not find. Covers the
/// four causes AND the negative control — a row that prices carries no key at
/// all, which is what reds if the `Some`-only insert is ever dropped.
#[test]
fn the_annotator_marks_every_unpriced_cause_and_leaves_a_priced_row_bare() {
    let unpriced = |row: &serde_json::Value| {
        row.get("unpriced_cause")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
    };
    let mut records = vec![
        // `unknown` + a session a vendor store could still be read for.
        LedgerRow {
            model: "unknown",
            session: Some("s1"),
            input: 400_000,
            ..Default::default()
        }
        .json(),
        // `unknown` with no session: no key to any store exists.
        LedgerRow {
            model: "unknown",
            session: None,
            input: 250_000,
            ..Default::default()
        }
        .json(),
        // A REAL model the table does not know — one `pricing.toml` entry away.
        LedgerRow {
            model: "big-pickle",
            session: Some("s3"),
            input: 60_000,
            ..Default::default()
        }
        .json(),
        // The negative control: this one prices.
        LedgerRow {
            model: OPUS,
            session: Some("s4"),
            input: 1_000_000,
            ..Default::default()
        }
        .json(),
    ];
    let mut interactive = vec![
        // The vendor keeps no count anywhere (ADR-0042 D11).
        InteractiveRow {
            session: "i1",
            tokens: None,
            ..Default::default()
        }
        .json(),
        InteractiveRow {
            session: "i2",
            model: OPUS,
            tokens: Some((1_000_000, 0)),
            ..Default::default()
        }
        .json(),
    ];

    annotate_unpriced(&mut records, &mut interactive, &BTreeMap::new(), &table());

    assert_eq!(unpriced(&records[0]).as_deref(), Some("recoverable"));
    assert_eq!(unpriced(&records[1]).as_deref(), Some("lost"));
    assert_eq!(unpriced(&records[2]).as_deref(), Some("no_price"));
    assert!(
        records[3].get("unpriced_cause").is_none(),
        "a row that priced must carry NO key — absence is the `fine` answer"
    );
    assert_eq!(unpriced(&interactive[0]).as_deref(), Some("unmetered"));
    assert!(
        interactive[1].get("unpriced_cause").is_none(),
        "a priced interactive row must carry no key either"
    );
}

/// Recovery is the same projection the Overview applies (ADR-0053 D2): once
/// the session is in the map the row prices, so it drops out of the filter.
#[test]
fn a_recovered_row_loses_its_unpriced_mark() {
    let mut records = vec![LedgerRow {
        model: "unknown",
        session: Some("s1"),
        input: 400_000,
        ..Default::default()
    }
    .json()];
    annotate_unpriced(
        &mut records,
        &mut [],
        &BTreeMap::from([("s1".to_string(), OPUS.to_string())]),
        &table(),
    );
    assert!(
        records[0].get("unpriced_cause").is_none(),
        "the recovery map named the engine, so the row prices"
    );
}

/// One ledger line by the four fields these tests vary. `session` is `None`
/// for the pre-ADR-0033 shape, where the field is skipped entirely rather
/// than written as `null`.
fn row(model: &str, session: Option<&str>, input: u64, output: u64) -> serde_json::Value {
    LedgerRow {
        model,
        session,
        input,
        output,
        ..Default::default()
    }
    .json()
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
        window: Window::All,
        since: None,
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
        window: Window::All,
        since: None,
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
        window: Window::All,
        since: None,
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
        window: Window::All,
        since: None,
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

/// Half this slice's honesty rules live in HTML/JS that no Rust gate
/// compiles: deleting the floor note, or reintroducing a `k`/`M` abbreviation
/// in JavaScript, leaves the suite green while the operator reads a floor as
/// a total. Pins both, the way `usage.rs`'s lower-bound test pins its label.
#[test]
fn the_spend_tab_renders_the_servers_figures_and_formats_none_of_its_own() {
    let js = include_str!("../../assets/ui/wb-spend.js");
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

    let html = include_str!("../../assets/ui/index.html");
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

    // #359's own surface. Each claim pins the EXPRESSION, not a noun a
    // comment could satisfy: the control that rescopes the page, the
    // attempt count that explains a delivery's cost, and the two elements
    // whose PLACEMENT is the rule (the band full-width, the overhead lines
    // beside the delivery rows rather than inside them).
    assert!(
        html.contains("setSpendPeriod($event.target.value)"),
        "the period must be a control that rescopes the page, not a caption"
    );
    assert!(
        html.contains("d.attempts"),
        "a delivery row must show how many runs executed it — the count is \
             what explains a cost that includes failed attempts"
    );
    assert!(
        html.contains("class=\"spend-band\"") && html.contains("class=\"spend-overhead\""),
        "the activity band and the overhead block must each be their own \
             element: overhead rendered inside `.delivery-rows` would read as an \
             issue that cost that much"
    );
    assert!(
        !js.contains("fetch(") && !js.contains("board.list"),
        "wb-spend.js must never fetch anything — issue titles ride whatever \
             the board already holds, because the board fold spawns a CLI that \
             makes tracker calls and a cost page must not pay it"
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
        window: Window::All,
        since: None,
    });
    assert_eq!(
        summary.tokens.meter, "↑12.4k ⚡0 ❄0 ↓3.2k",
        "↑ input, ⚡ cache-read, ❄ cache-write, ↓ output"
    );
}
