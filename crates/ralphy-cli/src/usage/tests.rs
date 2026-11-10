use super::*;

fn row(model: &str, phase: &str, actor: &str, version: &str, ts: &str, tok: Usage) -> UsageRow {
    UsageRow {
        project: "owner/repo".into(),
        actor_email: actor.into(),
        actor_name: "Name".into(),
        ralphy_version: version.into(),
        issue: 45,
        phase: phase.into(),
        agent: "claude".into(),
        model: model.into(),
        session_id: None,
        outcome: "done".into(),
        tokens: tok,
        ts: ts.into(),
    }
}

fn tok(input: u64, output: u64) -> Usage {
    Usage {
        input,
        output,
        cache_read: 0,
        cache_creation: 0,
        model: None,
    }
}

fn two_model_fixture() -> Vec<UsageRow> {
    vec![
        row(
            "claude-opus-4-8",
            "plan",
            "a@x.io",
            "rc5",
            "2026-06-15T10:00:00+00:00",
            tok(100, 10),
        ),
        row(
            "claude-opus-4-8",
            "execute",
            "a@x.io",
            "rc5",
            "2026-06-15T11:00:00+00:00",
            tok(200, 20),
        ),
        row(
            "claude-sonnet-4-6",
            "execute",
            "b@x.io",
            "rc4",
            "2026-06-15T12:00:00+00:00",
            tok(50, 5),
        ),
    ]
}

#[test]
fn resolve_slug_passes_through_a_verbatim_owner_repo() {
    // A slug that is not an existing directory is taken verbatim — no git
    // resolution, so an offline `--project owner/repo` still reads that ledger.
    let slug = resolve_slug(Some("acme/widgets"), Path::new(".")).expect("verbatim slug");
    assert_eq!(slug, "acme/widgets");
}

#[test]
fn resolve_slug_treats_a_dot_project_as_a_path_not_a_literal_slug() {
    // `.` is an existing directory, so it must NOT survive as the literal slug
    // "." — it resolves through git like `--repo` does. The crate dir is inside
    // the ralphy repo, so resolution yields a real slug distinct from ".".
    let slug = resolve_slug(Some("."), Path::new(".")).expect("path resolves");
    assert_ne!(
        slug, ".",
        "`.` must be resolved, not used as a literal slug"
    );
    assert_ne!(slug, "");
}

#[test]
fn group_by_model_sums_exact_per_model_tokens() {
    let groups = group_by(&two_model_fixture(), GroupBy::Model);
    let opus = groups
        .iter()
        .find(|(k, _, _)| k == "claude-opus-4-8")
        .expect("opus group");
    assert_eq!(opus.1.input, 300, "opus input = 100 + 200");
    assert_eq!(opus.1.output, 30, "opus output = 10 + 20");
    assert_eq!(opus.2, 2, "two opus rows");

    let sonnet = groups
        .iter()
        .find(|(k, _, _)| k == "claude-sonnet-4-6")
        .expect("sonnet group");
    assert_eq!(sonnet.1.input, 50);
    assert_eq!(sonnet.1.output, 5);
    assert_eq!(sonnet.2, 1);
}

#[test]
fn group_by_actor_phase_version_group_on_their_key() {
    let rows = two_model_fixture();
    let by_actor = group_by(&rows, GroupBy::Actor);
    assert_eq!(by_actor.len(), 2, "two distinct actors");
    let by_phase = group_by(&rows, GroupBy::Phase);
    assert_eq!(by_phase.len(), 2, "plan + execute");
    let exec = by_phase
        .iter()
        .find(|(k, _, _)| k == "execute")
        .expect("execute phase");
    assert_eq!(exec.1.input, 250, "execute input = 200 + 50");
    let by_version = group_by(&rows, GroupBy::Version);
    assert_eq!(by_version.len(), 2, "rc4 + rc5");
}

#[test]
fn filter_since_drops_an_older_dated_row() {
    let rows = vec![
        row(
            "claude-opus-4-8",
            "plan",
            "a@x.io",
            "rc5",
            "2026-06-14T23:59:00+00:00",
            tok(1, 1),
        ),
        row(
            "claude-opus-4-8",
            "plan",
            "a@x.io",
            "rc5",
            "2026-06-15T00:01:00+00:00",
            tok(2, 2),
        ),
    ];
    let kept = filter_since(rows, "2026-06-15");
    assert_eq!(kept.len(), 1, "the 2026-06-14 row is dropped");
    assert_eq!(kept[0].tokens.input, 2);
}

#[test]
fn render_table_balance_carries_tokens_and_usd() {
    // Million-scale counts, so the USD shows at two decimals. Per million
    // input/output: opus-4-8 $15/$75 (its floor), sonnet-4-6 $3/$15.
    let rows = vec![
        row(
            "claude-opus-4-8",
            "plan",
            "a@x.io",
            "rc5",
            "2026-06-15T10:00:00+00:00",
            tok(1_000_000, 100_000),
        ),
        row(
            "claude-sonnet-4-6",
            "execute",
            "b@x.io",
            "rc4",
            "2026-06-15T12:00:00+00:00",
            tok(1_000_000, 0),
        ),
    ];
    let lines = render_table(&rows, None, &PriceTable::defaults());
    assert_eq!(lines, vec!["balance: 2.1M tok · ~$25.50".to_string()]);
}

#[test]
fn export_csv_has_bom_header_flattened_columns_and_iso_ts() {
    let rows = two_model_fixture();
    let csv = export_csv(&rows, &PriceTable::defaults());
    // Leading UTF-8 BOM so Excel opens it clean on double-click.
    assert!(csv.starts_with('\u{FEFF}'), "leading BOM");
    let body = csv.trim_start_matches('\u{FEFF}');
    let mut lines = body.lines();
    let header = lines.next().expect("header row");
    // Flattened token columns + a usd column are named.
    for col in [
        "input",
        "output",
        "cache_read",
        "cache_creation",
        "usd",
        "ts",
    ] {
        assert!(header.contains(col), "header carries `{col}`: {header}");
    }
    // A data row carries the ISO `ts` verbatim.
    let first = lines.next().expect("a data row");
    assert!(
        first.contains("2026-06-15T10:00:00+00:00"),
        "ISO ts verbatim: {first}"
    );
}

#[test]
fn export_json_is_an_array_with_numeric_usd_for_priced_and_null_for_unpriced() {
    let mut rows = two_model_fixture();
    // Add an unpriced (unknown-model) row so the usd field discriminates: a
    // priced row carries a number, an unpriced one carries null (never 0).
    rows.push(row(
        "big-pickle",
        "execute",
        "c@x.io",
        "rc5",
        "2026-06-15T13:00:00+00:00",
        tok(99, 9),
    ));
    let json = export_json(&rows, &PriceTable::defaults()).expect("json");
    let value: serde_json::Value = serde_json::from_str(&json).expect("parses");
    let arr = value.as_array().expect("an array");
    assert_eq!(arr.len(), 4);
    // arr[0] is an opus row → usd is a number; the unpriced tail row → null.
    assert!(
        arr[0]["usd"].is_number(),
        "priced row carries a numeric usd: {}",
        arr[0]
    );
    assert!(
        arr[3]["usd"].is_null(),
        "unpriced row carries null usd (never 0): {}",
        arr[3]
    );
}
