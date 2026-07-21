//! The Kimi module of the usage scan (ADR-0033 §2/§6/§7). Parses the `wire.jsonl`
//! session logs of BOTH Kimi vendors into per-session × model interactive
//! records:
//!
//! - Legacy `kimi-cli` under `<kimi_dir>/sessions/<GROUP>/<SESSION>/wire.jsonl` —
//!   token data lives in `StatusUpdate` messages, deduped keep-largest by
//!   `message_id` (progressive snapshots of the same message).
//! - `kimi-code` under `<kimi_code_dir>/sessions/<WORKSPACE>/<SESSION>/agents/<AGENT>/wire.jsonl`
//!   — token data lives in top-level `usage.record` lines, counted only when
//!   `usageScope == "turn"` (a `session`/missing scope is aggregate bookkeeping).
//!
//! Sub-agent wire files (a `subagents`/`agents` path component) fold into their
//! PARENT session's aggregate — diverging from the oracle's per-message model,
//! since this crate's output unit is a session × model aggregate.
//!
//! PORTED from tokscale (`junhoyeo/tokscale`, MIT-licensed) — its
//! `crates/tokscale-core/src/sessions/kimi.rs` settled the field aliases, the
//! keep-largest dedup rule, and the zero-total skip.
//!
//! CAVEAT: the kimi-code branch ships ported but LOCALLY UNVALIDATED — no real
//! `usage.record` line was available on the build host (only config stubs), so
//! its fixtures match the oracle's documented shape, the strongest oracle
//! available, until first real kimi-code usage confirms it.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

use crate::{InteractiveRecord, KimiScan, Tokens};

/// Scan both Kimi stores into interactive records (one per session × model).
/// Walks `kimi_dir/sessions` (legacy) then `kimi_code_dir/sessions` (kimi-code)
/// into ONE aggregate, so a sub-agent wire file and its parent session's main
/// file merge. A missing store contributes nothing (never an error). `since`
/// drops records whose `last_ts` is strictly before it (§6: an unparseable bound
/// or record keeps the record).
pub fn scan_kimi(input: &KimiScan) -> Vec<InteractiveRecord> {
    let mut out: BTreeMap<(String, String), Agg> = BTreeMap::new();
    scan_legacy(input.kimi_dir, input.run_session_ids, input.repos, &mut out);
    scan_kimi_code(
        input.kimi_code_dir,
        input.run_session_ids,
        input.repos,
        &mut out,
    );

    let mut records: Vec<InteractiveRecord> = out
        .into_iter()
        .map(|((session_id, model), agg)| InteractiveRecord {
            agent: "kimi".to_string(),
            model,
            session_id,
            project: agg.project,
            actor_email: agg.actor_email,
            tokens: Some(agg.tokens),
            first_ts: ms_to_rfc3339(agg.first_ms),
            last_ts: ms_to_rfc3339(agg.last_ms),
            lower_bound: false,
        })
        .collect();

    if let Some(since) = input.since {
        if let Ok(since_dt) = chrono::DateTime::parse_from_rfc3339(since) {
            records.retain(|r| match chrono::DateTime::parse_from_rfc3339(&r.last_ts) {
                Ok(last) => last >= since_dt,
                Err(_) => true, // never hide spend on a parse miss
            });
        }
    }
    records
}

/// Per-(session, model) accumulator: the summed tokens, the ts span (unix ms),
/// and the resolved project/actor attribution (legacy leaves these `None`).
#[derive(Default)]
struct Agg {
    tokens: Tokens,
    first_ms: Option<i64>,
    last_ms: Option<i64>,
    project: Option<String>,
    actor_email: Option<String>,
}

impl Agg {
    /// Add a breakdown and widen the ts span.
    fn add(&mut self, t: &Tokens, ts_ms: Option<i64>) {
        self.tokens.input += t.input;
        self.tokens.output += t.output;
        self.tokens.cache_read += t.cache_read;
        self.tokens.cache_creation += t.cache_creation;
        if let Some(ms) = ts_ms {
            self.first_ms = Some(self.first_ms.map_or(ms, |cur| cur.min(ms)));
            self.last_ms = Some(self.last_ms.map_or(ms, |cur| cur.max(ms)));
        }
    }
}

/// One deduped legacy snapshot: the breakdown, its total (the keep-largest key),
/// and its ts.
struct Snap {
    tokens: Tokens,
    total: u64,
    ts_ms: Option<i64>,
}

/// Scan the legacy `kimi-cli` store: `kimi_dir/sessions/**/wire.jsonl` carrying
/// `StatusUpdate` messages. Model comes from `kimi_dir/config.json` `.model`
/// (default [`DEFAULT_MODEL`]). Progressive snapshots of the same `message_id`
/// dedup keep-largest (ties → later ts); missing-id messages each count. Legacy
/// project/actor stay `None` (the group segment is an irreversible hash, not a
/// path — Decision 2). Sums surviving snapshots into the shared aggregate.
fn scan_legacy(
    kimi_dir: &Path,
    run_session_ids: &std::collections::HashSet<String>,
    _repos: &[crate::RegisteredRepo],
    out: &mut BTreeMap<(String, String), Agg>,
) {
    let sessions_root = kimi_dir.join("sessions");
    let model = read_config_model(kimi_dir);
    for file in wire_files(&sessions_root) {
        let session_id = session_id_from_path(&file, &sessions_root);
        if run_session_ids.contains(&session_id) {
            continue;
        }
        let Ok(text) = fs::read_to_string(&file) else {
            continue;
        };
        // Keyed snapshots dedup by `message_id`; unkeyed (missing id) each count.
        let mut keyed: HashMap<String, Snap> = HashMap::new();
        let mut unkeyed: Vec<Snap> = Vec::new();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let Ok(val) = serde_json::from_str::<serde_json::Value>(line) else {
                continue;
            };
            if val.get("type").and_then(|v| v.as_str()) == Some("metadata") {
                continue;
            }
            let Some(message) = val.get("message") else {
                continue;
            };
            if message.get("type").and_then(|v| v.as_str()) != Some("StatusUpdate") {
                continue;
            }
            let Some(payload) = message.get("payload") else {
                continue;
            };
            let Some(tu) = payload.get("token_usage") else {
                continue;
            };
            let Some(tokens) = to_breakdown(
                i64_field(tu, "input_other"),
                i64_field(tu, "output"),
                i64_field(tu, "input_cache_read"),
                i64_field(tu, "input_cache_creation"),
            ) else {
                continue; // zero-total snapshot
            };
            // Unix seconds (float) → ms.
            let ts_ms = val
                .get("timestamp")
                .and_then(|v| v.as_f64())
                .map(|s| (s * 1000.0) as i64);
            let total = tokens.input + tokens.output + tokens.cache_read + tokens.cache_creation;
            let snap = Snap {
                tokens,
                total,
                ts_ms,
            };
            let key = payload
                .get("message_id")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty());
            match key {
                Some(key) => {
                    // Keep the largest snapshot; on a tie the later ts wins.
                    let replace = keyed.get(key).is_none_or(|existing| {
                        snap.total > existing.total
                            || (snap.total == existing.total && snap.ts_ms >= existing.ts_ms)
                    });
                    if replace {
                        keyed.insert(key.to_string(), snap);
                    }
                }
                None => unkeyed.push(snap),
            }
        }
        if keyed.is_empty() && unkeyed.is_empty() {
            continue; // no non-zero snapshot → no aggregate (else a zero record)
        }
        let agg = out.entry((session_id, model.clone())).or_default();
        for snap in keyed.values().chain(unkeyed.iter()) {
            agg.add(&snap.tokens, snap.ts_ms);
        }
    }
}

/// Scan the `kimi-code` store: `kimi_code_dir/sessions/**/wire.jsonl` carrying
/// top-level `usage.record` lines. Only `usageScope == "turn"` records count (a
/// missing scope is treated as session-scoped by kimi-code itself). Model is the
/// line's `.model` with the `kimi-code/` prefix stripped (default
/// [`DEFAULT_MODEL`]). Attributes project/actor from the WORKSPACE path segment
/// (Decision 3) via `paths_eq` against `repos`. Sums into the shared aggregate.
fn scan_kimi_code(
    kimi_code_dir: &Path,
    run_session_ids: &std::collections::HashSet<String>,
    repos: &[crate::RegisteredRepo],
    out: &mut BTreeMap<(String, String), Agg>,
) {
    let sessions_root = kimi_code_dir.join("sessions");
    // slug → resolved git actor email, computed at most once per attributed repo.
    let mut email_cache: HashMap<String, Option<String>> = HashMap::new();
    for file in wire_files(&sessions_root) {
        let session_id = session_id_from_path(&file, &sessions_root);
        if run_session_ids.contains(&session_id) {
            continue;
        }
        // WORKSPACE is the first path segment under `sessions/`.
        let (project, actor_email) =
            workspace_attribution(&file, &sessions_root, repos, &mut email_cache);
        let Ok(text) = fs::read_to_string(&file) else {
            continue;
        };
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let Ok(val) = serde_json::from_str::<serde_json::Value>(line) else {
                continue;
            };
            if val.get("type").and_then(|v| v.as_str()) != Some("usage.record") {
                continue;
            }
            // A missing scope is NOT "turn" → skip (session-scoped bookkeeping).
            if val.get("usageScope").and_then(|v| v.as_str()) != Some("turn") {
                continue;
            }
            let Some(usage) = val.get("usage") else {
                continue;
            };
            let Some(tokens) = to_breakdown(
                i64_field(usage, "inputOther"),
                i64_field(usage, "output"),
                i64_field(usage, "inputCacheRead"),
                i64_field(usage, "inputCacheCreation"),
            ) else {
                continue;
            };
            let model = val
                .get("model")
                .and_then(|v| v.as_str())
                .map(|m| m.strip_prefix("kimi-code/").unwrap_or(m).to_string())
                .unwrap_or_else(|| DEFAULT_MODEL.to_string());
            let ts_ms = val.get("time").and_then(|v| v.as_i64());
            let agg = out.entry((session_id.clone(), model)).or_default();
            agg.add(&tokens, ts_ms);
            if agg.project.is_none() {
                agg.project = project.clone();
            }
            if agg.actor_email.is_none() {
                agg.actor_email = actor_email.clone();
            }
        }
    }
}

/// Default model when `config.json` / the record's `model` is absent (oracle
/// `DEFAULT_MODEL`).
const DEFAULT_MODEL: &str = "kimi-for-coding";

/// Read `<kimi_dir>/config.json` `.model`; [`DEFAULT_MODEL`] when absent/empty.
fn read_config_model(kimi_dir: &Path) -> String {
    let path = kimi_dir.join("config.json");
    let model = fs::read_to_string(&path)
        .ok()
        .and_then(|c| serde_json::from_str::<serde_json::Value>(&c).ok())
        .and_then(|v| v.get("model").and_then(|m| m.as_str()).map(str::to_string))
        .filter(|m| !m.is_empty());
    model.unwrap_or_else(|| DEFAULT_MODEL.to_string())
}

/// The session id a `wire.jsonl` belongs to (below `sessions_root`): the segment
/// immediately BEFORE a `subagents`/`agents` component if present (fold a
/// sub-agent file into its parent session), else the file's parent directory
/// name; `"unknown"` when neither resolves.
fn session_id_from_path(path: &Path, sessions_root: &Path) -> String {
    let rel = path.strip_prefix(sessions_root).unwrap_or(path);
    let comps: Vec<&str> = rel
        .components()
        .filter_map(|c| c.as_os_str().to_str())
        .collect();
    if let Some(idx) = comps
        .iter()
        .position(|&c| c == "subagents" || c == "agents")
    {
        if idx > 0 {
            return comps[idx - 1].to_string();
        }
    }
    path.parent()
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        .unwrap_or("unknown")
        .to_string()
}

/// Resolve (project, actor_email) from the WORKSPACE segment (`sessions/<WORKSPACE>/…`)
/// against `repos` via [`paths_eq`], caching the git email per matched slug.
fn workspace_attribution(
    file: &Path,
    sessions_root: &Path,
    repos: &[crate::RegisteredRepo],
    email_cache: &mut HashMap<String, Option<String>>,
) -> (Option<String>, Option<String>) {
    let rel = file.strip_prefix(sessions_root).unwrap_or(file);
    let workspace = rel
        .components()
        .filter_map(|c| c.as_os_str().to_str())
        .next();
    let matched = workspace.and_then(|w| repos.iter().find(|r| paths_eq(&r.path, w)));
    let project = matched.map(|r| r.slug.clone());
    let actor_email = matched.and_then(|r| {
        email_cache
            .entry(r.slug.clone())
            .or_insert_with(|| repo_actor_email(&r.path))
            .clone()
    });
    (project, actor_email)
}

/// Clamp four signed token counts to `>= 0` and build a [`Tokens`]. `None` when
/// all four are zero (skip the entry, oracle `to_breakdown`). A signed field is
/// parsed as `i64` first so a legitimately-negative value clamps to 0 rather than
/// being read as absent (which straight-to-`u64` parsing would do).
fn to_breakdown(
    input_other: i64,
    output: i64,
    cache_read: i64,
    cache_creation: i64,
) -> Option<Tokens> {
    let input = input_other.max(0);
    let output = output.max(0);
    let cache_read = cache_read.max(0);
    let cache_creation = cache_creation.max(0);
    if input + output + cache_read + cache_creation == 0 {
        return None;
    }
    Some(Tokens {
        input: input as u64,
        output: output as u64,
        cache_read: cache_read as u64,
        cache_creation: cache_creation as u64,
    })
}

/// A JSON object's `key` as `i64`, 0 when absent/non-numeric.
fn i64_field(obj: &serde_json::Value, key: &str) -> i64 {
    obj.get(key).and_then(|v| v.as_i64()).unwrap_or(0)
}

/// A unix-ms instant → RFC3339 UTC; empty string when absent or out of range.
/// Duplicated from `opencode.rs` (ADR-0033 §7 accepts per-vendor duplication).
fn ms_to_rfc3339(ms: Option<i64>) -> String {
    ms.and_then(chrono::DateTime::from_timestamp_millis)
        .map(|d| d.to_rfc3339())
        .unwrap_or_default()
}

/// Normalize a filesystem path for a case-insensitive compare: `\` → `/`, trailing
/// `/` trimmed. Duplicated from `opencode.rs`.
fn normalize_path(p: &str) -> String {
    p.replace('\\', "/").trim_end_matches('/').to_string()
}

/// True when two paths name the same directory: normalized and compared with
/// `eq_ignore_ascii_case`. Duplicated from `opencode.rs`.
fn paths_eq(a: &str, b: &str) -> bool {
    normalize_path(a).eq_ignore_ascii_case(&normalize_path(b))
}

/// `git config user.email` for the attributed repo (ADR-0008 D7). `None` on a
/// non-zero exit or empty output. Duplicated from `opencode.rs` (ADR-0033 §7).
fn repo_actor_email(path: &str) -> Option<String> {
    let output = std::process::Command::new("git")
        .args(["-C", path, "config", "user.email"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let email = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!email.is_empty()).then_some(email)
}

/// Every `wire.jsonl` under `dir`, recursively. Tolerant: a missing/unreadable
/// dir yields nothing. Order unspecified. Mirrors `codex.rs::jsonl_files`, but
/// filters on the file NAME `wire.jsonl` (not the extension).
fn wire_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = fs::read_dir(&d) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.file_name().and_then(|n| n.to_str()) == Some("wire.jsonl") {
                out.push(path);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::RegisteredRepo;
    use std::collections::HashSet;

    fn no_runs() -> HashSet<String> {
        HashSet::new()
    }

    /// Write `content` to `<root>/<rel>`, creating parents.
    fn write_wire(root: &Path, rel: &str, content: &str) {
        let path = root.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    /// Scan a legacy-only store rooted at `kimi_dir` (kimi-code empty).
    fn scan_legacy_only(kimi_dir: &Path) -> Vec<InteractiveRecord> {
        scan_kimi(&KimiScan {
            kimi_dir,
            kimi_code_dir: Path::new("does-not-exist-kimi-code"),
            run_session_ids: &no_runs(),
            repos: &[],
            since: None,
        })
    }

    /// Scan a kimi-code-only store rooted at `kimi_code_dir` (legacy empty).
    fn scan_code_only(kimi_code_dir: &Path) -> Vec<InteractiveRecord> {
        scan_kimi(&KimiScan {
            kimi_dir: Path::new("does-not-exist-kimi"),
            kimi_code_dir,
            run_session_ids: &no_runs(),
            repos: &[],
            since: None,
        })
    }

    fn status_update(
        ts: f64,
        input: i64,
        output: i64,
        cache_read: i64,
        message_id: &str,
    ) -> String {
        format!(
            "{{\"timestamp\": {ts}, \"message\": {{\"type\": \"StatusUpdate\", \"payload\": {{\"token_usage\": {{\"input_other\": {input}, \"output\": {output}, \"input_cache_read\": {cache_read}, \"input_cache_creation\": 0}}, \"message_id\": \"{message_id}\"}}}}}}"
        )
    }

    #[test]
    fn legacy_dedups_progressive_status_updates_by_message_id() {
        let tmp = tempfile::tempdir().unwrap();
        // Same message_id, totals 110 then 155 → keep the larger snapshot only.
        let body = format!(
            "{{\"type\": \"metadata\"}}\n{}\n{}",
            status_update(1770983410.0, 100, 10, 0, "msg-p"),
            status_update(1770983420.0, 120, 30, 5, "msg-p"),
        );
        write_wire(tmp.path(), "sessions/GRP/SESS/wire.jsonl", &body);
        let records = scan_legacy_only(tmp.path());
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].tokens.as_ref().unwrap().input, 120);
        assert_eq!(records[0].tokens.as_ref().unwrap().output, 30);
        assert_eq!(records[0].tokens.as_ref().unwrap().cache_read, 5);
        assert_eq!(records[0].session_id, "SESS");
        assert_eq!(records[0].agent, "kimi");
    }

    #[test]
    fn legacy_keeps_distinct_and_missing_message_ids_separate() {
        let tmp = tempfile::tempdir().unwrap();
        // Two distinct ids (out 1, 2) + two missing-id (out 3, 4): all summed = 10.
        let missing = |ts: f64, input: i64, output: i64| {
            format!(
                "{{\"timestamp\": {ts}, \"message\": {{\"type\": \"StatusUpdate\", \"payload\": {{\"token_usage\": {{\"input_other\": {input}, \"output\": {output}, \"input_cache_read\": 0, \"input_cache_creation\": 0}}}}}}}}"
            )
        };
        let body = format!(
            "{}\n{}\n{}\n{}",
            status_update(1770983410.0, 10, 1, 0, "msg-1"),
            status_update(1770983420.0, 20, 2, 0, "msg-2"),
            missing(1770983430.0, 30, 3),
            missing(1770983440.0, 40, 4),
        );
        write_wire(tmp.path(), "sessions/GRP/SESS/wire.jsonl", &body);
        let records = scan_legacy_only(tmp.path());
        assert_eq!(records.len(), 1);
        assert_eq!(
            records[0].tokens.as_ref().unwrap().output,
            10,
            "1+2+3+4 all summed"
        );
    }

    fn usage_record(scope: Option<&str>, input: i64, output: i64, time: i64) -> String {
        let scope = match scope {
            Some(s) => format!(",\"usageScope\":\"{s}\""),
            None => String::new(),
        };
        format!(
            "{{\"type\":\"usage.record\",\"model\":\"kimi-code/kimi-for-coding\",\"usage\":{{\"inputOther\":{input},\"output\":{output},\"inputCacheRead\":0,\"inputCacheCreation\":0}}{scope},\"time\":{time}}}"
        )
    }

    #[test]
    fn kimi_code_counts_only_turn_scope() {
        let tmp = tempfile::tempdir().unwrap();
        let body = format!(
            "{}\n{}\n{}",
            usage_record(Some("session"), 999, 999, 1780319377000),
            usage_record(None, 888, 888, 1780319377005),
            usage_record(Some("turn"), 100, 50, 1780319377010),
        );
        write_wire(tmp.path(), "sessions/WS/SESS/agents/main/wire.jsonl", &body);
        let records = scan_code_only(tmp.path());
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].tokens.as_ref().unwrap().input, 100);
        assert_eq!(records[0].tokens.as_ref().unwrap().output, 50);
    }

    #[test]
    fn kimi_code_strips_model_prefix() {
        let tmp = tempfile::tempdir().unwrap();
        let body = usage_record(Some("turn"), 10, 5, 1780319377010);
        write_wire(tmp.path(), "sessions/WS/SESS/agents/main/wire.jsonl", &body);
        let records = scan_code_only(tmp.path());
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].model, "kimi-for-coding");
    }

    #[test]
    fn subagent_wire_attributed_to_parent_session() {
        let tmp = tempfile::tempdir().unwrap();
        // Parent main file + a subagent file, same session, same model → merged.
        write_wire(
            tmp.path(),
            "sessions/GRP/SESS/wire.jsonl",
            &status_update(1770983410.0, 100, 0, 0, "msg-main"),
        );
        write_wire(
            tmp.path(),
            "sessions/GRP/SESS/subagents/SUB/wire.jsonl",
            &status_update(1770983420.0, 50, 0, 0, "msg-sub"),
        );
        let records = scan_legacy_only(tmp.path());
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].session_id, "SESS");
        assert_eq!(records[0].tokens.as_ref().unwrap().input, 150);
    }

    #[test]
    fn zero_usage_stub_parses_to_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let body = "{\"type\": \"metadata\"}\n{\"timestamp\": 1770983410.0, \"message\": {\"type\": \"StatusUpdate\", \"payload\": {\"token_usage\": {\"input_other\": 0, \"output\": 0, \"input_cache_read\": 0, \"input_cache_creation\": 0}, \"message_id\": \"msg-empty\"}}}";
        write_wire(tmp.path(), "sessions/GRP/SESS/wire.jsonl", body);
        let records = scan_legacy_only(tmp.path());
        assert!(records.is_empty());
    }

    #[test]
    fn excludes_run_owned_sessions() {
        let tmp = tempfile::tempdir().unwrap();
        write_wire(
            tmp.path(),
            "sessions/GRP/RUN/wire.jsonl",
            &status_update(1770983410.0, 10, 1, 0, "m-run"),
        );
        write_wire(
            tmp.path(),
            "sessions/GRP/INT/wire.jsonl",
            &status_update(1770983420.0, 20, 2, 0, "m-int"),
        );
        let mut runs = HashSet::new();
        runs.insert("RUN".to_string());
        let records = scan_kimi(&KimiScan {
            kimi_dir: tmp.path(),
            kimi_code_dir: Path::new("does-not-exist-kimi-code"),
            run_session_ids: &runs,
            repos: &[],
            since: None,
        });
        assert!(records.iter().any(|r| r.session_id == "INT"));
        assert!(!records.iter().any(|r| r.session_id == "RUN"));
    }

    #[test]
    fn kimi_missing_stores_contribute_zero() {
        let records = scan_kimi(&KimiScan {
            kimi_dir: Path::new("no-such-kimi-dir-anywhere"),
            kimi_code_dir: Path::new("no-such-kimi-code-dir-anywhere"),
            run_session_ids: &no_runs(),
            repos: &[],
            since: None,
        });
        assert!(records.is_empty());
    }

    #[test]
    fn legacy_default_model_when_config_absent() {
        let tmp = tempfile::tempdir().unwrap();
        write_wire(
            tmp.path(),
            "sessions/GRP/SESS/wire.jsonl",
            &status_update(1770983410.0, 10, 1, 0, "m"),
        );
        let records = scan_legacy_only(tmp.path());
        assert_eq!(records[0].model, "kimi-for-coding");
    }

    #[test]
    fn legacy_reads_model_from_config() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir_all(tmp.path()).unwrap();
        fs::write(
            tmp.path().join("config.json"),
            "{\"model\": \"kimi-k2-custom\"}",
        )
        .unwrap();
        write_wire(
            tmp.path(),
            "sessions/GRP/SESS/wire.jsonl",
            &status_update(1770983410.0, 10, 1, 0, "m"),
        );
        let records = scan_legacy_only(tmp.path());
        assert_eq!(records[0].model, "kimi-k2-custom");
    }

    #[test]
    fn kimi_code_attributes_workspace_to_registered_repo() {
        let tmp = tempfile::tempdir().unwrap();
        // WORKSPACE segment equals a registered repo path (raw-path treatment).
        write_wire(
            tmp.path(),
            "sessions/wsrepo/SESS/agents/main/wire.jsonl",
            &usage_record(Some("turn"), 10, 5, 1780319377010),
        );
        let repos = vec![RegisteredRepo {
            slug: "o/wsrepo".into(),
            path: "wsrepo".into(),
        }];
        let records = scan_kimi(&KimiScan {
            kimi_dir: Path::new("does-not-exist-kimi"),
            kimi_code_dir: tmp.path(),
            run_session_ids: &no_runs(),
            repos: &repos,
            since: None,
        });
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].project.as_deref(), Some("o/wsrepo"));
    }
}
