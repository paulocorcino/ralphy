//! The daemon's read-only view of the token-usage ledger (ADR-0033 §3). Reads
//! the ledger's JSONL files directly from disk — this crate never imports
//! `ralphy-core` (ADR-0032 §10) — mirroring `registry.rs`, which reparses
//! `repos.toml` itself rather than depending on core.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use ralphy_usage_scan::{
    scan_claude, scan_codex, scan_copilot, scan_cursor, scan_gemini, scan_kimi, scan_opencode,
    ClaudeScan, CodexScan, CopilotScan, CursorScan, GeminiScan, KimiScan, OpenCodeScan,
    RegisteredRepo,
};

use crate::registry::RegistryStore;
use crate::StorePaths;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct UsageContribution {
    pub daemon_id: Option<String>,
    pub records: Vec<serde_json::Value>,
    pub interactive: Vec<serde_json::Value>,
}

#[derive(Debug, Clone, serde::Serialize, PartialEq, Eq)]
pub struct MissingUsageContribution {
    pub daemon_id: String,
    pub environment: String,
    pub why: String,
}

#[derive(Debug, Clone, serde::Serialize, PartialEq)]
pub struct FleetUsage {
    pub daemon_id: Option<String>,
    pub records: Vec<serde_json::Value>,
    pub interactive: Vec<serde_json::Value>,
    pub missing: Vec<MissingUsageContribution>,
}

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

pub fn local_contribution(
    usage_dir: &Path,
    stores: &StorePaths,
    registry: &RegistryStore,
    daemon_id: Option<String>,
    since: Option<&str>,
) -> UsageContribution {
    let mut records = run_records(usage_dir, since);
    project_recovered_models(usage_dir, &mut records);
    let interactive = interactive_records(stores, registry, &records, since);
    let mut contribution = UsageContribution {
        daemon_id,
        records,
        interactive,
    };
    stamp_rows(&mut contribution);
    contribution
}

/// The persisted `session_id → model` recovery map beside the ledger (ADR-0053
/// D3), or an empty map when it is missing or malformed — a reader that ignores
/// it still produces today's answer, so a bad map degrades to "nothing recovered"
/// rather than failing the read.
pub fn recovered_models(usage_dir: &Path) -> BTreeMap<String, String> {
    std::fs::read(usage_dir.join("session-models.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<BTreeMap<String, String>>(&bytes).ok())
        .unwrap_or_default()
}

fn project_recovered_models(usage_dir: &Path, records: &mut [serde_json::Value]) {
    let models = recovered_models(usage_dir);
    if models.is_empty() {
        return;
    }
    for record in records {
        let Some(object) = record.as_object_mut() else {
            continue;
        };
        if object.get("model").and_then(|value| value.as_str()) != Some("unknown") {
            continue;
        }
        let Some(session_id) = object.get("session_id").and_then(|value| value.as_str()) else {
            continue;
        };
        if let Some(model) = models.get(session_id) {
            object.insert("model".into(), serde_json::Value::String(model.clone()));
        }
    }
}

pub fn fold_fleet_usage(
    mut local: UsageContribution,
    peers: impl IntoIterator<
        Item = (
            crate::peer::PeerDescriptor,
            Result<UsageContribution, String>,
        ),
    >,
    rejected: impl IntoIterator<Item = crate::peer::PeerReject>,
) -> FleetUsage {
    stamp_rows(&mut local);
    let mut fleet = FleetUsage {
        daemon_id: local.daemon_id,
        records: local.records,
        interactive: local.interactive,
        missing: rejected
            .into_iter()
            .filter_map(|reject| match &reject {
                crate::peer::PeerReject::IncompatibleVersion {
                    daemon_id,
                    environment,
                    ..
                } => Some(MissingUsageContribution {
                    daemon_id: daemon_id.clone(),
                    environment: environment.clone(),
                    why: reject.why(),
                }),
                crate::peer::PeerReject::Malformed { .. }
                | crate::peer::PeerReject::DuplicateIdentity { .. } => None,
            })
            .collect(),
    };
    for (descriptor, result) in peers {
        match result {
            Ok(mut contribution) => {
                contribution.daemon_id = Some(descriptor.daemon_id.clone());
                stamp_rows(&mut contribution);
                fleet.records.extend(contribution.records);
                fleet.interactive.extend(contribution.interactive);
            }
            Err(why) => fleet.missing.push(MissingUsageContribution {
                daemon_id: descriptor.daemon_id,
                environment: descriptor.environment,
                why,
            }),
        }
    }
    fleet
}

/// Narrow a folded fleet reading to ONE project's rows — what the Spend tab's
/// Ledger grid reads, so opening a project's ledger never ships another
/// project's lines to the browser.
///
/// `missing` is deliberately untouched: a peer whose usage never arrived is
/// fleet health, not project data, and hiding it behind a project filter would
/// make the grid claim completeness it does not have.
pub fn scope_to_project(fleet: &mut FleetUsage, project: &str) {
    let mine = |row: &serde_json::Value| {
        row.get("project").and_then(serde_json::Value::as_str) == Some(project)
    };
    fleet.records.retain(mine);
    fleet.interactive.retain(mine);
}

fn stamp_rows(contribution: &mut UsageContribution) {
    let Some(daemon_id) = contribution.daemon_id.as_ref() else {
        return;
    };
    for row in contribution
        .records
        .iter_mut()
        .chain(contribution.interactive.iter_mut())
    {
        if let Some(object) = row.as_object_mut() {
            object
                .entry("daemon_id")
                .or_insert_with(|| serde_json::Value::String(daemon_id.clone()));
        }
    }
}

#[cfg(test)]
mod tests;
