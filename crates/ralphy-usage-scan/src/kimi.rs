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
use std::path::Path;

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

impl Snap {
    /// Keep-largest by total; on a tie the later ts wins.
    fn supersedes(&self, existing: &Snap) -> bool {
        self.total > existing.total
            || (self.total == existing.total && self.ts_ms >= existing.ts_ms)
    }
}

/// The surviving legacy snapshots of one wire file. Keyed snapshots dedup by
/// `message_id`; unkeyed (missing or empty id) each count.
#[derive(Default)]
struct Snaps {
    keyed: HashMap<String, Snap>,
    unkeyed: Vec<Snap>,
}

impl Snaps {
    fn insert(&mut self, key: Option<String>, snap: Snap) {
        match key {
            Some(key) => {
                if self.keyed.get(&key).is_none_or(|old| snap.supersedes(old)) {
                    self.keyed.insert(key, snap);
                }
            }
            None => self.unkeyed.push(snap),
        }
    }

    fn is_empty(&self) -> bool {
        self.keyed.is_empty() && self.unkeyed.is_empty()
    }

    /// Sum every surviving snapshot into `agg`.
    fn fold_into(&self, agg: &mut Agg) {
        for snap in self.keyed.values().chain(self.unkeyed.iter()) {
            agg.add(&snap.tokens, snap.ts_ms);
        }
    }
}

/// Read one wire line as a `StatusUpdate` snapshot and its `message_id` key.
/// `None` for a blank, unparseable or `metadata` line, any other message type,
/// and a zero-total snapshot.
fn read_status_update(line: &str) -> Option<(Option<String>, Snap)> {
    let val: serde_json::Value = serde_json::from_str(line.trim()).ok()?;
    if val.get("type").and_then(|v| v.as_str()) == Some("metadata") {
        return None;
    }
    let message = val.get("message")?;
    if message.get("type").and_then(|v| v.as_str()) != Some("StatusUpdate") {
        return None;
    }
    let payload = message.get("payload")?;
    let tu = payload.get("token_usage")?;
    let tokens = to_breakdown(
        i64_field(tu, "input_other"),
        i64_field(tu, "output"),
        i64_field(tu, "input_cache_read"),
        i64_field(tu, "input_cache_creation"),
    )?;
    // Unix seconds (float) → ms.
    let ts_ms = val
        .get("timestamp")
        .and_then(|v| v.as_f64())
        .map(|s| (s * 1000.0) as i64);
    let total = tokens.input + tokens.output + tokens.cache_read + tokens.cache_creation;
    let key = payload
        .get("message_id")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    Some((
        key,
        Snap {
            tokens,
            total,
            ts_ms,
        },
    ))
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
    for file in crate::walk::files_under(&sessions_root, is_wire_log) {
        let session_id = session_id_from_path(&file, &sessions_root);
        if run_session_ids.contains(&session_id) {
            continue;
        }
        let Ok(text) = fs::read_to_string(&file) else {
            continue;
        };
        let mut snaps = Snaps::default();
        for (key, snap) in text.lines().filter_map(read_status_update) {
            snaps.insert(key, snap);
        }
        if snaps.is_empty() {
            continue; // no non-zero snapshot → no aggregate (else a zero record)
        }
        snaps.fold_into(out.entry((session_id, model.clone())).or_default());
    }
}

/// One `kimi-code` turn-scoped `usage.record`: its model, tokens and time.
struct UsageRecord {
    model: String,
    tokens: Tokens,
    ts_ms: Option<i64>,
}

/// Read one wire line as a turn-scoped `usage.record`. `None` for a blank or
/// unparseable line, any other type, a missing scope (not "turn": session-scoped
/// bookkeeping) and a zero-total record.
fn read_usage_record(line: &str) -> Option<UsageRecord> {
    let val: serde_json::Value = serde_json::from_str(line.trim()).ok()?;
    if val.get("type").and_then(|v| v.as_str()) != Some("usage.record") {
        return None;
    }
    if val.get("usageScope").and_then(|v| v.as_str()) != Some("turn") {
        return None;
    }
    let usage = val.get("usage")?;
    let tokens = to_breakdown(
        i64_field(usage, "inputOther"),
        i64_field(usage, "output"),
        i64_field(usage, "inputCacheRead"),
        i64_field(usage, "inputCacheCreation"),
    )?;
    let model = val.get("model").and_then(|v| v.as_str()).map_or_else(
        || DEFAULT_MODEL.to_owned(),
        |m| m.strip_prefix("kimi-code/").unwrap_or(m).to_owned(),
    );
    Some(UsageRecord {
        model,
        tokens,
        ts_ms: val.get("time").and_then(|v| v.as_i64()),
    })
}

impl Agg {
    /// Set the project and actor only while they are empty.
    fn attribute(&mut self, project: &Option<String>, actor_email: &Option<String>) {
        if self.project.is_none() {
            self.project = project.clone();
        }
        if self.actor_email.is_none() {
            self.actor_email = actor_email.clone();
        }
    }
}

/// Scan the `kimi-code` store: `kimi_code_dir/sessions/**/wire.jsonl` carrying
/// top-level `usage.record` lines. Only `usageScope == "turn"` records count (a
/// missing scope is treated as session-scoped by kimi-code itself). Model is the
/// line's `.model` with the `kimi-code/` prefix stripped (default
/// [`DEFAULT_MODEL`]). Attributes project/actor from the WORKSPACE path segment
/// via [`crate::attribution::find_repo`] against `repos`. Sums into the shared
/// aggregate.
fn scan_kimi_code(
    kimi_code_dir: &Path,
    run_session_ids: &std::collections::HashSet<String>,
    repos: &[crate::RegisteredRepo],
    out: &mut BTreeMap<(String, String), Agg>,
) {
    let sessions_root = kimi_code_dir.join("sessions");
    // slug → resolved git actor email, computed at most once per attributed repo.
    let mut email_cache: HashMap<String, Option<String>> = HashMap::new();
    for file in crate::walk::files_under(&sessions_root, is_wire_log) {
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
        for rec in text.lines().filter_map(read_usage_record) {
            let agg = out.entry((session_id.clone(), rec.model)).or_default();
            agg.add(&rec.tokens, rec.ts_ms);
            agg.attribute(&project, &actor_email);
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
/// against `repos` via [`crate::attribution::find_repo`], caching the git email
/// per matched slug.
fn workspace_attribution(
    file: &Path,
    sessions_root: &Path,
    repos: &[crate::RegisteredRepo],
    email_cache: &mut HashMap<String, Option<String>>,
) -> (Option<String>, Option<String>) {
    let rel = file.strip_prefix(sessions_root).unwrap_or(file);
    let workspace = rel.components().find_map(|c| c.as_os_str().to_str());
    let matched = workspace.and_then(|w| crate::attribution::find_repo(repos, w));
    let project = matched.map(|r| r.slug.clone());
    let actor_email = matched.and_then(|r| {
        email_cache
            .entry(r.slug.clone())
            .or_insert_with(|| ralphy_git_read::user_email(Path::new(&r.path)))
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

/// `true` for a file named `wire.jsonl`, the Kimi session log. The filter is on
/// the file name, not the extension.
fn is_wire_log(path: &Path) -> bool {
    path.file_name().and_then(|n| n.to_str()) == Some("wire.jsonl")
}

#[cfg(test)]
mod tests;
