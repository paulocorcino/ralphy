//! The central, append-only token-usage ledger (ADR-0008 D6). One JSON object
//! per line, one line per completed phase, in `~/.ralphy/usage/<project-id>.jsonl`
//! — outside the per-run scratch so accumulation survives the run branch it never
//! pushes. The unit of truth is **tokens**; no `cost`/`usd` is ever written (D2),
//! it is derived at read-time from a price table (D8).
//!
//! Everything here is best-effort by contract: a write or parse failure logs and
//! is swallowed by the caller so token measurement never gates or breaks the
//! orchestration it observes (D9).

use std::path::PathBuf;

use anyhow::{anyhow, Result};
use serde::ser::SerializeStruct;
use serde::{Serialize, Serializer};

use crate::Usage;

/// One ledger line (ADR-0008 D6). Serialized as a flat JSON object whose `tokens`
/// member carries only the four numeric token fields — the top-level `model` is
/// the model key (D8), so `Usage::model` is deliberately omitted from `tokens`.
#[derive(Debug, Clone, Serialize)]
pub struct LedgerRecord {
    /// `owner/repo` git-remote slug, or a path-hash fallback (D7).
    pub project: String,
    /// `git config user.email` — the actor key (D7).
    pub actor_email: String,
    /// `git config user.name` — the actor display name (D7).
    pub actor_name: String,
    /// The orchestrator build, `env!("CARGO_PKG_VERSION")` (D6).
    pub ralphy_version: String,
    pub issue: u64,
    /// `plan` | `execute`.
    pub phase: String,
    /// The adapter label (`claude` | `codex` | `opencode`), self-reported.
    pub agent: String,
    /// The model the price table resolves on (D8), or `unknown`.
    pub model: String,
    /// The terminal status of this phase (D6).
    pub outcome: String,
    /// The four-way token split, written WITHOUT `Usage::model`.
    #[serde(serialize_with = "serialize_tokens")]
    pub tokens: Usage,
    /// RFC3339 UTC timestamp.
    pub ts: String,
}

/// Serialize a [`Usage`] as a `tokens` object carrying only the four numeric
/// fields — never `model`, which is the record's top-level `model` field (D6).
fn serialize_tokens<S>(usage: &Usage, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    let mut state = serializer.serialize_struct("Tokens", 4)?;
    state.serialize_field("input", &usage.input)?;
    state.serialize_field("output", &usage.output)?;
    state.serialize_field("cache_read", &usage.cache_read)?;
    state.serialize_field("cache_creation", &usage.cache_creation)?;
    state.end()
}

/// Serialize one record to a single JSON line (no trailing newline). Pure, so the
/// field set and the no-`cost`/`usd` invariant unit-test without the filesystem.
pub fn record_line(rec: &LedgerRecord) -> Result<String> {
    Ok(serde_json::to_string(rec)?)
}

/// Sum the four token fields across every parseable JSONL line's `tokens` object.
/// Tolerant of malformed lines and missing fields — a bad line is skipped, not
/// fatal (the ledger is append-only and best-effort).
pub fn sum_tokens(jsonl: &str) -> Usage {
    let mut total = Usage::default();
    for line in jsonl.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let Some(tokens) = value.get("tokens") else {
            continue;
        };
        let field = |k: &str| tokens.get(k).and_then(|v| v.as_u64()).unwrap_or(0);
        total.input += field("input");
        total.output += field("output");
        total.cache_read += field("cache_read");
        total.cache_creation += field("cache_creation");
    }
    total
}

/// The ledger root: `$RALPHY_USAGE_DIR` when set (tests point it at a temp dir),
/// else `<home>/.ralphy/usage`. `None` when no home directory can be resolved.
fn usage_root() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("RALPHY_USAGE_DIR") {
        return Some(PathBuf::from(dir));
    }
    let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME"))?;
    Some(PathBuf::from(home).join(".ralphy").join("usage"))
}

/// The ledger file for `slug`: `<root>/<sanitized>.jsonl`, where the `owner/repo`
/// slug's `/` is sanitized to `-` for the filename (the in-line `project` field
/// keeps the `owner/repo` form — D6).
fn ledger_path(slug: &str) -> Option<PathBuf> {
    let sanitized = slug.replace('/', "-");
    Some(usage_root()?.join(format!("{sanitized}.jsonl")))
}

/// Append one record as a JSON line to its project's ledger file, creating the
/// directory as needed. Keyed by `rec.project`.
pub fn append(rec: &LedgerRecord) -> Result<()> {
    use std::io::Write;
    let path = ledger_path(&rec.project).ok_or_else(|| anyhow!("no usage-ledger root resolved"))?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let line = record_line(rec)?;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)?;
    writeln!(file, "{line}")?;
    Ok(())
}

/// The project's cumulative token totals, summed over its whole ledger file. A
/// missing file (nothing recorded yet) reads as `Usage::default()`.
pub fn project_total(slug: &str) -> Usage {
    let Some(path) = ledger_path(slug) else {
        return Usage::default();
    };
    let Ok(content) = std::fs::read_to_string(&path) else {
        return Usage::default();
    };
    sum_tokens(&content)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_record() -> LedgerRecord {
        LedgerRecord {
            project: "owner/repo".into(),
            actor_email: "dev@example.com".into(),
            actor_name: "Dev Name".into(),
            ralphy_version: "0.1.0-rc5".into(),
            issue: 42,
            phase: "execute".into(),
            agent: "claude".into(),
            model: "claude-opus-4-8".into(),
            outcome: "done".into(),
            tokens: Usage {
                input: 100,
                output: 9,
                cache_read: 1710,
                cache_creation: 94,
                model: Some("claude-opus-4-8".into()),
            },
            ts: "2026-06-15T12:34:56+00:00".into(),
        }
    }

    #[test]
    fn record_line_has_all_fields_and_no_cost_or_usd() {
        let line = record_line(&sample_record()).expect("serialize");
        for key in [
            "project",
            "actor_email",
            "actor_name",
            "ralphy_version",
            "issue",
            "phase",
            "agent",
            "model",
            "outcome",
            "tokens",
            "ts",
        ] {
            assert!(
                line.contains(&format!("\"{key}\"")),
                "record must carry the `{key}` key: {line}"
            );
        }
        // The four token sub-fields are present...
        for key in ["input", "output", "cache_read", "cache_creation"] {
            assert!(line.contains(&format!("\"{key}\"")), "tokens.{key}: {line}");
        }
        // ...but never the model inside `tokens` (it is the top-level field), and
        // never a derived cost — USD is a read-time projection, never stored (D2).
        assert!(
            !line.contains("cost") && !line.contains("usd"),
            "no cost/usd may be written to the ledger: {line}"
        );
    }

    #[test]
    fn sum_tokens_adds_four_fields_across_lines() {
        let jsonl = "\
{\"phase\":\"plan\",\"tokens\":{\"input\":10,\"output\":1,\"cache_read\":100,\"cache_creation\":5}}
{\"phase\":\"execute\",\"tokens\":{\"input\":20,\"output\":2,\"cache_read\":200,\"cache_creation\":7}}
";
        let total = sum_tokens(jsonl);
        assert_eq!(total.input, 30);
        assert_eq!(total.output, 3);
        assert_eq!(total.cache_read, 300);
        assert_eq!(total.cache_creation, 12);
    }

    #[test]
    fn append_then_project_total_round_trips() {
        // Point the ledger root at a unique temp dir so production is untouched.
        let dir = std::env::temp_dir().join(format!(
            "ralphy-ledger-{}-{:x}",
            std::process::id(),
            sample_record().issue
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::env::set_var("RALPHY_USAGE_DIR", &dir);

        let mut first = sample_record();
        first.phase = "plan".into();
        first.tokens = Usage {
            input: 1,
            output: 2,
            cache_read: 3,
            cache_creation: 4,
            model: None,
        };
        let mut second = sample_record();
        second.tokens = Usage {
            input: 10,
            output: 20,
            cache_read: 30,
            cache_creation: 40,
            model: None,
        };
        append(&first).expect("append first");
        append(&second).expect("append second");

        let total = project_total(&sample_record().project);
        assert_eq!(total.input, 11);
        assert_eq!(total.output, 22);
        assert_eq!(total.cache_read, 33);
        assert_eq!(total.cache_creation, 44);

        std::env::remove_var("RALPHY_USAGE_DIR");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
