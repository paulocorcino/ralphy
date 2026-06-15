//! `ralphy usage` — the read/report layer over the append-only token ledger
//! (ADR-0008 D2/D8/D11). Everything here is a pure read: it reads the project's
//! ledger rows, optionally filters and groups them, and prints a balance plus
//! group-by cuts or an export. USD is applied at **read-time** via the price
//! table; the ledger is never touched.

use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::Result;
use clap::{Args, ValueEnum};
use ralphy_core::{git, read_project_rows, Usage, UsageRow};

use crate::pricing::PriceTable;

/// `ralphy usage` arguments.
#[derive(Args)]
pub struct UsageArgs {
    /// Any path inside the target repo; resolved to its git toplevel for the
    /// project slug (unless `--project` is given).
    #[arg(long, default_value = ".")]
    pub repo: PathBuf,

    /// Group the rows by a dimension instead of printing only the balance.
    #[arg(long = "by", value_enum)]
    pub by: Option<GroupBy>,

    /// Keep only rows on or after this `YYYY-MM-DD` date.
    #[arg(long)]
    pub since: Option<String>,

    /// Read this `owner/repo` project's ledger instead of resolving from `--repo`.
    #[arg(long)]
    pub project: Option<String>,

    /// Output format: the default human table, or `csv`/`json` for export.
    #[arg(long, value_enum)]
    pub format: Option<Format>,
}

/// The dimension `--by` groups on.
#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
pub enum GroupBy {
    Phase,
    Model,
    Actor,
    Version,
}

/// The output format `--format` selects.
#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
pub enum Format {
    Table,
    Csv,
    Json,
}

/// The group key for `row` under `by`.
fn group_key(row: &UsageRow, by: GroupBy) -> String {
    match by {
        GroupBy::Phase => row.phase.clone(),
        GroupBy::Model => row.model.clone(),
        GroupBy::Actor => row.actor_email.clone(),
        GroupBy::Version => row.ralphy_version.clone(),
    }
}

/// Keep only rows whose `ts` date is on or after `since` (a `YYYY-MM-DD` string).
/// The ledger writes RFC3339 timestamps, whose leading 10 chars are the ISO date,
/// so a lexical compare of that prefix against `since` is a correct date filter.
pub fn filter_since(rows: Vec<UsageRow>, since: &str) -> Vec<UsageRow> {
    rows.into_iter()
        .filter(|r| r.ts.len() >= 10 && &r.ts[..10] >= since)
        .collect()
}

/// Group rows by `by`, returning `(key, summed tokens, row count)` per key,
/// ordered by key. Pure over its inputs.
pub fn group_by(rows: &[UsageRow], by: GroupBy) -> Vec<(String, Usage, usize)> {
    let mut map: BTreeMap<String, (Usage, usize)> = BTreeMap::new();
    for row in rows {
        let entry = map.entry(group_key(row, by)).or_default();
        entry.0.add_tokens(&row.tokens);
        entry.1 += 1;
    }
    map.into_iter()
        .map(|(k, (usage, count))| (k, usage, count))
        .collect()
}

/// The read-time USD of a row set, computed **per model** and summed: each model's
/// rows are summed and priced separately (price resolves on the model — D8).
/// Returns `(priced_usd, any_unpriced)`; `any_unpriced` flags a model absent from
/// the table, whose tokens are excluded from `priced_usd` (never reported as `0`).
fn usd_for_rows(rows: &[&UsageRow], table: &PriceTable) -> (f64, bool) {
    let mut by_model: BTreeMap<String, Usage> = BTreeMap::new();
    for row in rows {
        by_model
            .entry(row.model.clone())
            .or_default()
            .add_tokens(&row.tokens);
    }
    let mut usd = 0.0;
    let mut partial = false;
    for (model, tokens) in &by_model {
        match table.cost_usd(model, tokens) {
            Some(c) => usd += c,
            None => partial = true,
        }
    }
    (usd, partial)
}

/// Format a USD figure for display: `~$2.10`, with a `+?` suffix when some model
/// in the set was unpriced, or a bare `~$?` when nothing could be priced.
fn fmt_usd(usd: f64, partial: bool) -> String {
    if partial && usd == 0.0 {
        "~$?".to_string()
    } else if partial {
        format!("~${usd:.2}+?")
    } else {
        format!("~${usd:.2}")
    }
}

/// Format a token count compactly: `1.2M`, `8.4k`, or a bare count.
fn fmt_tokens(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}k", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

/// Render the default (table) report as lines: a balance line (total tokens +
/// read-time USD) and, when `by` is set, one line per group.
pub fn render_table(rows: &[UsageRow], by: Option<GroupBy>, table: &PriceTable) -> Vec<String> {
    let mut lines = Vec::new();

    let all: Vec<&UsageRow> = rows.iter().collect();
    let mut total = Usage::default();
    for r in rows {
        total.add_tokens(&r.tokens);
    }
    let (usd, partial) = usd_for_rows(&all, table);
    lines.push(format!(
        "balance: {} tok · {}",
        fmt_tokens(total.total()),
        fmt_usd(usd, partial)
    ));

    if let Some(by) = by {
        // Group rows (keeping the rows themselves for per-group USD).
        let mut groups: BTreeMap<String, Vec<&UsageRow>> = BTreeMap::new();
        for r in rows {
            groups.entry(group_key(r, by)).or_default().push(r);
        }
        for (key, group_rows) in groups {
            let mut g = Usage::default();
            for r in &group_rows {
                g.add_tokens(&r.tokens);
            }
            let (gusd, gpartial) = usd_for_rows(&group_rows, table);
            lines.push(format!(
                "{key} · {} tok · {} · {} row(s)",
                fmt_tokens(g.total()),
                fmt_usd(gusd, gpartial),
                group_rows.len()
            ));
        }
    }

    lines
}

/// The per-row read-time USD, priced by the row's own model. `None` (rendered as
/// an empty column) when the model is unpriced.
fn row_usd(row: &UsageRow, table: &PriceTable) -> Option<f64> {
    table.cost_usd(&row.model, &row.tokens)
}

/// Quote one CSV field per RFC 4180 when it carries a comma, quote, or newline.
fn csv_field(s: &str) -> String {
    if s.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

/// The CSV column header — the flattened token fields plus a read-time `usd`
/// column, with the timestamp last.
const CSV_HEADER: &str = "project,actor_email,actor_name,ralphy_version,issue,phase,agent,model,outcome,input,output,cache_read,cache_creation,usd,ts";

/// Export the rows as Excel-friendly CSV (ADR-0008 D11): a leading UTF-8 BOM so
/// Excel opens it clean on double-click, a header row, the nested `tokens` object
/// flattened into columns, a read-time `usd` column (empty when unpriced), and the
/// ISO `ts` verbatim.
pub fn export_csv(rows: &[UsageRow], table: &PriceTable) -> String {
    let mut out = String::from("\u{FEFF}");
    out.push_str(CSV_HEADER);
    out.push('\n');
    for r in rows {
        let usd = row_usd(r, table)
            .map(|c| format!("{c:.6}"))
            .unwrap_or_default();
        let cells = [
            csv_field(&r.project),
            csv_field(&r.actor_email),
            csv_field(&r.actor_name),
            csv_field(&r.ralphy_version),
            r.issue.to_string(),
            csv_field(&r.phase),
            csv_field(&r.agent),
            csv_field(&r.model),
            csv_field(&r.outcome),
            r.tokens.input.to_string(),
            r.tokens.output.to_string(),
            r.tokens.cache_read.to_string(),
            r.tokens.cache_creation.to_string(),
            usd,
            csv_field(&r.ts),
        ];
        out.push_str(&cells.join(","));
        out.push('\n');
    }
    out
}

/// Export the rows as a JSON array (for pipelines): one object per row carrying the
/// row fields, the flattened token counts, and a read-time `usd` field (`null` when
/// the model is unpriced — never `0`).
pub fn export_json(rows: &[UsageRow], table: &PriceTable) -> Result<String> {
    let arr: Vec<serde_json::Value> = rows
        .iter()
        .map(|r| {
            serde_json::json!({
                "project": r.project,
                "actor_email": r.actor_email,
                "actor_name": r.actor_name,
                "ralphy_version": r.ralphy_version,
                "issue": r.issue,
                "phase": r.phase,
                "agent": r.agent,
                "model": r.model,
                "outcome": r.outcome,
                "input": r.tokens.input,
                "output": r.tokens.output,
                "cache_read": r.tokens.cache_read,
                "cache_creation": r.tokens.cache_creation,
                "usd": row_usd(r, table),
                "ts": r.ts,
            })
        })
        .collect();
    Ok(serde_json::to_string_pretty(&serde_json::Value::Array(arr))?)
}

/// `ralphy usage`: read the project's ledger and print the balance / group-by cut
/// or an export.
pub fn usage_cmd(args: UsageArgs) -> Result<()> {
    let slug = match &args.project {
        Some(p) => p.clone(),
        None => {
            let repo_root = git::resolve_toplevel(&args.repo)?;
            git::project_slug(&repo_root)
        }
    };

    let mut rows = read_project_rows(&slug);
    if let Some(since) = &args.since {
        rows = filter_since(rows, since);
    }

    let table = PriceTable::load();
    match args.format.unwrap_or(Format::Table) {
        Format::Table => {
            for line in render_table(&rows, args.by, &table) {
                println!("{line}");
            }
        }
        Format::Csv => print!("{}", export_csv(&rows, &table)),
        Format::Json => println!("{}", export_json(&rows, &table)?),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
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
        let rows = two_model_fixture();
        let lines = render_table(&rows, None, &PriceTable::defaults());
        assert!(lines[0].starts_with("balance:"), "balance line: {}", lines[0]);
        assert!(lines[0].contains("tok"), "tokens: {}", lines[0]);
        assert!(lines[0].contains("~$"), "usd: {}", lines[0]);
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
    fn export_json_is_an_array_of_objects_with_a_usd_field() {
        let rows = two_model_fixture();
        let json = export_json(&rows, &PriceTable::defaults()).expect("json");
        let value: serde_json::Value = serde_json::from_str(&json).expect("parses");
        let arr = value.as_array().expect("an array");
        assert_eq!(arr.len(), 3);
        assert!(
            arr[0].get("usd").is_some(),
            "objects carry a usd field: {}",
            arr[0]
        );
    }
}
