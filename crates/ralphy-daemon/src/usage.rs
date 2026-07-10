//! The daemon's read-only view of the token-usage ledger (ADR-0033 §3). Reads
//! the ledger's JSONL files directly from disk — this crate never imports
//! `ralphy-core` (ADR-0032 §10) — mirroring `registry.rs`, which reparses
//! `repos.toml` itself rather than depending on core.

use std::path::{Path, PathBuf};

/// The ledger root: `$RALPHY_USAGE_DIR` when set, else `<home>/.ralphy/usage`.
/// Copied from `ralphy-core`'s `ledger::usage_root()` so the daemon reads the
/// same location core writes. `None` when no home directory can be resolved.
pub fn usage_dir_path() -> anyhow::Result<PathBuf> {
    if let Some(dir) = std::env::var_os("RALPHY_USAGE_DIR") {
        return Ok(PathBuf::from(dir));
    }
    let home = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .ok_or_else(|| anyhow::anyhow!("no home directory resolved for the usage ledger"))?;
    Ok(PathBuf::from(home).join(".ralphy").join("usage"))
}

/// Read every ledger record under `dir`, optionally filtered by `since`
/// (inclusive: a record's `ts` string `>=` `since`; ledger timestamps are
/// always RFC3339 UTC, so string comparison orders correctly). Tolerant like
/// `ledger::read_rows`: a non-`.jsonl` file, an unreadable file, a blank line,
/// or a line that does not parse as a JSON object is skipped. A missing or
/// unreadable `dir` yields an empty vec. Records are sorted by `ts` ascending
/// for deterministic output.
pub fn run_records(dir: &Path, since: Option<&str>) -> Vec<serde_json::Value> {
    let mut records = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return records;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };
        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
                continue;
            };
            if !value.is_object() {
                continue;
            }
            if let Some(since) = since {
                let ts = value.get("ts").and_then(|v| v.as_str()).unwrap_or("");
                if ts < since {
                    continue;
                }
            }
            records.push(value);
        }
    }
    records.sort_by(|a, b| {
        let ts = |v: &serde_json::Value| {
            v.get("ts")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string()
        };
        ts(a).cmp(&ts(b))
    });
    records
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_ledger(dir: &Path, name: &str, content: &str) {
        std::fs::write(dir.join(name), content).unwrap();
    }

    #[test]
    fn run_records_returns_all_lines_when_since_is_none() {
        let dir = tempfile::tempdir().unwrap();
        write_ledger(
            dir.path(),
            "owner-repo.jsonl",
            "{\"session_id\":\"sess-a\",\"ts\":\"2026-06-15T12:00:00+00:00\"}\n\
             {\"session_id\":\"sess-b\",\"ts\":\"2026-06-15T12:05:00+00:00\"}\n",
        );
        let records = run_records(dir.path(), None);
        assert_eq!(records.len(), 2);
    }

    #[test]
    fn run_records_since_filters_to_matching_or_later_ts() {
        let dir = tempfile::tempdir().unwrap();
        write_ledger(
            dir.path(),
            "owner-repo.jsonl",
            "{\"session_id\":\"sess-a\",\"ts\":\"2026-06-15T12:00:00+00:00\"}\n\
             {\"session_id\":\"sess-b\",\"ts\":\"2026-06-15T12:05:00+00:00\"}\n",
        );
        let records = run_records(dir.path(), Some("2026-06-15T12:05:00+00:00"));
        assert_eq!(records.len(), 1);
        assert_eq!(
            records[0].get("session_id").and_then(|v| v.as_str()),
            Some("sess-b")
        );
    }

    #[test]
    fn run_records_skips_a_malformed_middle_line() {
        let dir = tempfile::tempdir().unwrap();
        write_ledger(
            dir.path(),
            "owner-repo.jsonl",
            "{\"session_id\":\"sess-a\",\"ts\":\"2026-06-15T12:00:00+00:00\"}\n\
             { this is not valid json\n\
             {\"session_id\":\"sess-b\",\"ts\":\"2026-06-15T12:05:00+00:00\"}\n",
        );
        let records = run_records(dir.path(), None);
        assert_eq!(records.len(), 2, "malformed middle line is skipped");
    }
}
