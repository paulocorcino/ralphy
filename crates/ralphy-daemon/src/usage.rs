//! The daemon's read-only view of the token-usage ledger (ADR-0033 §3). Reads
//! the ledger's JSONL files directly from disk — this crate never imports
//! `ralphy-core` (ADR-0032 §10) — mirroring `registry.rs`, which reparses
//! `repos.toml` itself rather than depending on core.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use ralphy_usage_scan::{scan_claude, scan_codex, ClaudeScan, CodexScan, RegisteredRepo};

use crate::registry::RegistryStore;

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

/// The Claude projects store root: `$RALPHY_CLAUDE_PROJECTS_DIR` when set (tests
/// point it at a temp dir), else `<home>/.claude/projects`. Mirrors
/// [`usage_dir_path`]; `None` when no home directory can be resolved.
pub fn claude_projects_dir_path() -> anyhow::Result<PathBuf> {
    if let Some(dir) = std::env::var_os("RALPHY_CLAUDE_PROJECTS_DIR") {
        return Ok(PathBuf::from(dir));
    }
    let home = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .ok_or_else(|| {
            anyhow::anyhow!("no home directory resolved for the Claude projects store")
        })?;
    Ok(PathBuf::from(home).join(".claude").join("projects"))
}

/// The Codex session store root: `$RALPHY_CODEX_DIR` when set (tests point it at
/// a temp dir), else `$CODEX_HOME` (Codex's own base var), else `<home>/.codex`.
/// This is the `.codex` BASE — `scan_codex` walks its `sessions`/
/// `archived_sessions` subtrees. Mirrors [`claude_projects_dir_path`].
pub fn codex_dir_path() -> anyhow::Result<PathBuf> {
    if let Some(dir) = std::env::var_os("RALPHY_CODEX_DIR") {
        return Ok(PathBuf::from(dir));
    }
    if let Some(dir) = std::env::var_os("CODEX_HOME") {
        return Ok(PathBuf::from(dir));
    }
    let home = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .ok_or_else(|| {
            anyhow::anyhow!("no home directory resolved for the Codex sessions store")
        })?;
    Ok(PathBuf::from(home).join(".codex"))
}

/// Scan the Claude AND Codex stores for interactive usage records, excluding
/// sessions the ledger already owns (their `session_id` appears in
/// `run_records`), and serialize each to JSON (ADR-0033 §2/§6). `registry.repos`
/// supplies the project/actor attribution. Read-only: neither scan writes. The
/// Codex records are chained after the Claude ones.
pub fn interactive_records(
    claude_dir: &Path,
    codex_dir: &Path,
    registry: &RegistryStore,
    run_records: &[serde_json::Value],
    since: Option<&str>,
) -> Vec<serde_json::Value> {
    let run_session_ids: HashSet<String> = run_records
        .iter()
        .filter_map(|r| r.get("session_id").and_then(|v| v.as_str()))
        .map(str::to_string)
        .collect();
    let repos: Vec<RegisteredRepo> = registry
        .repos
        .iter()
        .map(|(slug, entry)| RegisteredRepo {
            slug: slug.clone(),
            path: entry.path.clone(),
        })
        .collect();
    let claude = scan_claude(&ClaudeScan {
        projects_dir: claude_dir,
        run_session_ids: &run_session_ids,
        repos: &repos,
        since,
    });
    let codex = scan_codex(&CodexScan {
        codex_dir,
        run_session_ids: &run_session_ids,
        repos: &repos,
        since,
    });
    claude
        .iter()
        .chain(codex.iter())
        .filter_map(|r| serde_json::to_value(r).ok())
        .collect()
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
