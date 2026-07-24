//! The daemon's read-only view of the token-usage ledger (ADR-0033 §3). Reads
//! the ledger's JSONL files directly from disk — this crate never imports
//! `ralphy-core` (ADR-0032 §10) — mirroring `registry.rs`, which reparses
//! `repos.toml` itself rather than depending on core.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use ralphy_usage_scan::{
    scan_claude, scan_codex, scan_copilot, scan_cursor, scan_gemini, scan_kimi, scan_opencode,
    ClaudeScan, CodexScan, CopilotScan, CursorScan, GeminiScan, KimiScan, OpenCodeScan,
    RegisteredRepo,
};

use crate::registry::RegistryStore;
use crate::StorePaths;

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

/// The OpenCode SQLite store: `$RALPHY_OPENCODE_DB` when set (tests point it at a
/// temp file), else `<home>/.local/share/opencode/opencode.db` (USERPROFILE on
/// Windows, HOME elsewhere). Mirrors the adapter's `opencode_db_path` and
/// [`codex_dir_path`]; `None` when no home directory can be resolved.
pub fn opencode_db_path() -> anyhow::Result<PathBuf> {
    if let Some(db) = std::env::var_os("RALPHY_OPENCODE_DB") {
        return Ok(PathBuf::from(db));
    }
    let home = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .ok_or_else(|| anyhow::anyhow!("no home directory resolved for the OpenCode store"))?;
    Ok(PathBuf::from(home)
        .join(".local")
        .join("share")
        .join("opencode")
        .join("opencode.db"))
}

/// The Copilot SQLite store: `$RALPHY_COPILOT_DB` when set (tests point it at a
/// temp file), else `$COPILOT_HOME/session-store.db` (Copilot's own base var),
/// else `<home>/.copilot/session-store.db`. Mirrors the adapter's
/// `copilot_store_db` and [`opencode_db_path`].
pub fn copilot_db_path() -> anyhow::Result<PathBuf> {
    if let Some(db) = std::env::var_os("RALPHY_COPILOT_DB") {
        return Ok(PathBuf::from(db));
    }
    if let Some(base) = std::env::var_os("COPILOT_HOME") {
        return Ok(PathBuf::from(base).join("session-store.db"));
    }
    let home = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .ok_or_else(|| anyhow::anyhow!("no home directory resolved for the Copilot store"))?;
    Ok(PathBuf::from(home)
        .join(".copilot")
        .join("session-store.db"))
}

/// The legacy Kimi (`kimi-cli`) session store root: `$RALPHY_KIMI_DIR` when set
/// (tests point it at a temp dir), else `$KIMI_HOME` (Kimi's own base var), else
/// `<home>/.kimi`. This is the `.kimi` BASE — `scan_kimi` walks its `sessions/`
/// subtree and reads its `config.json`. Mirrors [`codex_dir_path`].
pub fn kimi_dir_path() -> anyhow::Result<PathBuf> {
    if let Some(dir) = std::env::var_os("RALPHY_KIMI_DIR") {
        return Ok(PathBuf::from(dir));
    }
    if let Some(dir) = std::env::var_os("KIMI_HOME") {
        return Ok(PathBuf::from(dir));
    }
    let home = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .ok_or_else(|| anyhow::anyhow!("no home directory resolved for the Kimi sessions store"))?;
    Ok(PathBuf::from(home).join(".kimi"))
}

/// The `kimi-code` session store root: `$RALPHY_KIMI_CODE_DIR` when set (tests
/// point it at a temp dir), else `$KIMI_CODE_HOME`, else `<home>/.kimi-code`.
/// This is the `.kimi-code` BASE — `scan_kimi` walks its `sessions/` subtree.
/// Mirrors [`kimi_dir_path`].
pub fn kimi_code_dir_path() -> anyhow::Result<PathBuf> {
    if let Some(dir) = std::env::var_os("RALPHY_KIMI_CODE_DIR") {
        return Ok(PathBuf::from(dir));
    }
    if let Some(dir) = std::env::var_os("KIMI_CODE_HOME") {
        return Ok(PathBuf::from(dir));
    }
    let home = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .ok_or_else(|| {
            anyhow::anyhow!("no home directory resolved for the kimi-code sessions store")
        })?;
    Ok(PathBuf::from(home).join(".kimi-code"))
}

/// The Cursor interactive session store root: `$RALPHY_CURSOR_DIR` when set (tests
/// point it at a temp dir), else `$XDG_CONFIG_HOME/cursor`, else `<home>/.cursor`.
/// This is the `.cursor` BASE — `scan_cursor` walks BOTH its `chats/` and
/// `projects/` subtrees, so one base resolver keeps ONE env override instead of
/// two. Mirrors [`codex_dir_path`].
///
/// It deliberately does NOT read `$CURSOR_CONFIG_DIR`: that is the variable
/// Ralphy points at its own per-run scratch directory (ADR-0042 D17), so honouring
/// it here would resolve Ralphy's throwaway state instead of the OPERATOR's own
/// sessions — which is the only thing this store is read for (D11, #250).
pub fn cursor_dir_path() -> anyhow::Result<PathBuf> {
    if let Some(dir) = std::env::var_os("RALPHY_CURSOR_DIR") {
        return Ok(PathBuf::from(dir));
    }
    if let Some(dir) = std::env::var_os("XDG_CONFIG_HOME") {
        return Ok(PathBuf::from(dir).join("cursor"));
    }
    let home = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .ok_or_else(|| {
            anyhow::anyhow!("no home directory resolved for the Cursor session store")
        })?;
    Ok(PathBuf::from(home).join(".cursor"))
}

/// The Gemini interactive session store root: `$RALPHY_GEMINI_DIR` when set (tests
/// point it at a temp dir), else `<home>/.gemini`. This is the `.gemini` BASE —
/// `scan_gemini` walks its `tmp/<basename>/chats/` subtree. Mirrors
/// [`cursor_dir_path`].
///
/// It deliberately does NOT read `$GEMINI_CLI_HOME`: that is the variable Ralphy
/// points at its OWN owned configuration root (ADR-0043 D4), so honouring it here
/// would resolve Ralphy's per-repo state instead of the OPERATOR's own interactive
/// sessions — the only thing this store is read for.
pub fn gemini_dir_path() -> anyhow::Result<PathBuf> {
    if let Some(dir) = std::env::var_os("RALPHY_GEMINI_DIR") {
        return Ok(PathBuf::from(dir));
    }
    let home = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .ok_or_else(|| {
            anyhow::anyhow!("no home directory resolved for the Gemini session store")
        })?;
    Ok(PathBuf::from(home).join(".gemini"))
}

/// Scan the Claude, Codex, OpenCode, Kimi, Copilot, Cursor AND Gemini stores for
/// interactive usage records, excluding sessions the ledger already owns (their
/// `session_id` appears in `run_records`), and serialize each to JSON
/// (ADR-0033 §2/§6). `registry.repos` supplies the project/actor attribution.
/// Read-only: no scan writes (the Copilot scan reads a private copy, never the
/// live store). The Codex records are chained after the Claude ones, then the
/// OpenCode ones, then the Kimi ones, then the Copilot ones, then the Cursor
/// ones — whose `tokens` is always `null` (ADR-0042 D11: no count exists) — then
/// the Gemini ones, whose counts are a LOWER BOUND (ADR-0043 D10: the router's
/// tokens never reach disk).
pub fn interactive_records(
    stores: &StorePaths,
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
        projects_dir: &stores.claude_projects_dir,
        run_session_ids: &run_session_ids,
        repos: &repos,
        since,
    });
    let codex = scan_codex(&CodexScan {
        codex_dir: &stores.codex_dir,
        run_session_ids: &run_session_ids,
        repos: &repos,
        since,
    });
    let opencode = scan_opencode(&OpenCodeScan {
        db_path: &stores.opencode_db,
        run_session_ids: &run_session_ids,
        repos: &repos,
        since,
    });
    let kimi = scan_kimi(&KimiScan {
        kimi_dir: &stores.kimi_dir,
        kimi_code_dir: &stores.kimi_code_dir,
        run_session_ids: &run_session_ids,
        repos: &repos,
        since,
    });
    let copilot = scan_copilot(&CopilotScan {
        db_path: &stores.copilot_db,
        run_session_ids: &run_session_ids,
        repos: &repos,
        since,
    });
    let cursor = scan_cursor(&CursorScan {
        cursor_dir: &stores.cursor_dir,
        run_session_ids: &run_session_ids,
        repos: &repos,
        since,
    });
    let gemini = scan_gemini(&GeminiScan {
        gemini_dir: &stores.gemini_dir,
        run_session_ids: &run_session_ids,
        repos: &repos,
        since,
    });
    claude
        .iter()
        .chain(codex.iter())
        .chain(opencode.iter())
        .chain(kimi.iter())
        .chain(copilot.iter())
        .chain(cursor.iter())
        .chain(gemini.iter())
        .filter_map(|r| serde_json::to_value(r).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Serializes the env-mutating path resolvers against each other (mirrors
    /// `identity.rs`'s lock); the tests share one process env.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn write_ledger(dir: &Path, name: &str, content: &str) {
        std::fs::write(dir.join(name), content).unwrap();
    }

    /// `$RALPHY_COPILOT_DB` wins over `$COPILOT_HOME`, which wins over the home
    /// default. Env is process-global, so the three legs run in one test.
    #[test]
    fn copilot_db_path_prefers_the_env_override() {
        let guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let restore = (
            std::env::var_os("RALPHY_COPILOT_DB"),
            std::env::var_os("COPILOT_HOME"),
        );

        std::env::set_var("RALPHY_COPILOT_DB", "C:/tmp/override.db");
        std::env::set_var("COPILOT_HOME", "C:/tmp/copilot-home");
        assert_eq!(
            copilot_db_path().unwrap(),
            PathBuf::from("C:/tmp/override.db")
        );

        std::env::remove_var("RALPHY_COPILOT_DB");
        assert_eq!(
            copilot_db_path().unwrap(),
            PathBuf::from("C:/tmp/copilot-home").join("session-store.db")
        );

        std::env::remove_var("COPILOT_HOME");
        let home = copilot_db_path().unwrap();
        assert!(
            home.ends_with(PathBuf::from(".copilot").join("session-store.db")),
            "home default, got {home:?}"
        );

        match restore.0 {
            Some(v) => std::env::set_var("RALPHY_COPILOT_DB", v),
            None => std::env::remove_var("RALPHY_COPILOT_DB"),
        }
        match restore.1 {
            Some(v) => std::env::set_var("COPILOT_HOME", v),
            None => std::env::remove_var("COPILOT_HOME"),
        }
        drop(guard);
    }

    /// D17 points `$CURSOR_CONFIG_DIR` at Ralphy's per-run SCRATCH directory. If
    /// this resolver honoured it, the daemon would report on Ralphy's own throwaway
    /// state instead of the operator's sessions — so the scratch var must not divert
    /// it, while the test-only `$RALPHY_CURSOR_DIR` still wins.
    #[test]
    fn cursor_dir_path_ignores_the_scratch_config_dir() {
        let guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let restore = (
            std::env::var_os("CURSOR_CONFIG_DIR"),
            std::env::var_os("RALPHY_CURSOR_DIR"),
            std::env::var_os("XDG_CONFIG_HOME"),
        );

        std::env::set_var("CURSOR_CONFIG_DIR", "C:/tmp/ralphy-scratch");
        std::env::remove_var("RALPHY_CURSOR_DIR");
        std::env::remove_var("XDG_CONFIG_HOME");
        let got = cursor_dir_path().unwrap();
        assert!(
            got.ends_with(".cursor"),
            "the scratch config dir must not divert the resolver, got {got:?}"
        );
        assert!(
            !got.starts_with("C:/tmp/ralphy-scratch"),
            "resolved Ralphy's own scratch state, got {got:?}"
        );

        std::env::set_var("RALPHY_CURSOR_DIR", "C:/tmp/override");
        assert_eq!(
            cursor_dir_path().unwrap(),
            PathBuf::from("C:/tmp/override"),
            "the test override must still win"
        );

        match restore.0 {
            Some(v) => std::env::set_var("CURSOR_CONFIG_DIR", v),
            None => std::env::remove_var("CURSOR_CONFIG_DIR"),
        }
        match restore.1 {
            Some(v) => std::env::set_var("RALPHY_CURSOR_DIR", v),
            None => std::env::remove_var("RALPHY_CURSOR_DIR"),
        }
        match restore.2 {
            Some(v) => std::env::set_var("XDG_CONFIG_HOME", v),
            None => std::env::remove_var("XDG_CONFIG_HOME"),
        }
        drop(guard);
    }

    /// ADR-0043 D4 points `$GEMINI_CLI_HOME` at Ralphy's OWN owned root. If this
    /// resolver honoured it, the daemon would report Ralphy's per-repo state instead
    /// of the operator's sessions — so it must not divert the resolver, while the
    /// test-only `$RALPHY_GEMINI_DIR` still wins.
    #[test]
    fn gemini_dir_path_ignores_ralphys_own_cli_home() {
        let guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let restore = (
            std::env::var_os("GEMINI_CLI_HOME"),
            std::env::var_os("RALPHY_GEMINI_DIR"),
        );

        std::env::set_var("GEMINI_CLI_HOME", "C:/tmp/ralphy-owned-root");
        std::env::remove_var("RALPHY_GEMINI_DIR");
        let got = gemini_dir_path().unwrap();
        assert!(
            got.ends_with(".gemini"),
            "Ralphy's own CLI home must not divert the resolver, got {got:?}"
        );
        assert!(
            !got.starts_with("C:/tmp/ralphy-owned-root"),
            "resolved Ralphy's own owned root, got {got:?}"
        );

        std::env::set_var("RALPHY_GEMINI_DIR", "C:/tmp/override");
        assert_eq!(
            gemini_dir_path().unwrap(),
            PathBuf::from("C:/tmp/override"),
            "the test override must still win"
        );

        match restore.0 {
            Some(v) => std::env::set_var("GEMINI_CLI_HOME", v),
            None => std::env::remove_var("GEMINI_CLI_HOME"),
        }
        match restore.1 {
            Some(v) => std::env::set_var("RALPHY_GEMINI_DIR", v),
            None => std::env::remove_var("RALPHY_GEMINI_DIR"),
        }
        drop(guard);
    }

    /// ADR-0040 Tier 4 anti-drift: a vendor that reaches the daemon's launch enum
    /// must have a store-path RESOLVER here AND have its scan actually chained into
    /// [`interactive_records`]. Source-text pin over this very file, so it reds the
    /// moment a seventh `Agent::ALL` variant lands with a resolver nobody calls —
    /// the state Cursor was left in by #248 and that #250 closed.
    #[test]
    fn every_launchable_vendor_has_a_store_path_resolver() {
        let src = include_str!("usage.rs");
        for agent in crate::session::Agent::ALL {
            let token = crate::dispatch::agent_flag(agent);
            let found = src.lines().any(|l| {
                l.trim_start()
                    .strip_prefix("pub fn ")
                    .is_some_and(|rest| rest.starts_with(token) && rest.contains("_path("))
            });
            assert!(
                found,
                "no `pub fn {token}…_path(` resolver in usage.rs — {agent:?} reached \
                 the daemon's launch enum without one, so nothing can even locate \
                 its interactive store"
            );
            assert!(
                src.contains(&format!("scan_{token}(&")),
                "no `scan_{token}(&` call in usage.rs — {agent:?} has a store-path \
                 resolver but its scan is never chained into `interactive_records`, \
                 so /api/usage reports none of its interactive sessions"
            );
        }
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

    /// #262's whole deliverable is the LABEL, and it lives in JS/HTML that no
    /// Rust gate compiles: deleting the ternary or the caveat leaves the suite
    /// green while the operator reads a floor as a total (ADR-0043 D10). Pins
    /// both renderers into the served assets, like `dispatch.rs`'s workbench-trio
    /// pin does for the agent list.
    #[test]
    fn the_workbench_labels_a_lower_bound_record() {
        let js = include_str!("../assets/ui/app.js");
        let start = js
            .find("usageTokens(rec) {")
            .expect("app.js: usageTokens moved");
        let body = &js[start..start + 400];
        assert!(
            body.contains("rec.lower_bound"),
            "usageTokens must branch on lower_bound: {body}"
        );
        assert!(
            body.contains("\"\u{2265} \"") && body.contains("\" (lower bound)\""),
            "usageTokens must render `\u{2265} n (lower bound)`: {body}"
        );

        let html = include_str!("../assets/ui/index.html");
        assert!(
            html.contains("usage.interactive.some(r =&gt; r.lower_bound)"),
            "index.html must show the caveat note only when a record is a floor"
        );
        assert!(
            html.contains("a &#8805; figure is a lower bound"),
            "index.html must explain what the \u{2265} means"
        );
    }
}
