//! The Codex module of the usage scan (ADR-0033 §2/§6, ADR-0008 D5). Parses the
//! `rollout-*.jsonl` session logs under `~/.codex/{sessions,archived_sessions}/`
//! into per-session × model interactive records.
//!
//! Two things distinguish Codex from Claude. First, `total_token_usage` is a
//! **cumulative** snapshot the session rewrites in place, and compaction/context
//! capping can rewrite it DOWNWARD (ADR-0008 D5); the delta core keeps a
//! session-wide monotonic **watermark** so a regressed snapshot contributes no
//! (negative) delta and never lowers recorded spend. Second, the dedup contract
//! is split (ADR-0033 §5): a run's ledger session id is the rollout file **stem**
//! (`ralphy-agent-codex/src/usage.rs::rollout_session_id`), so run-owned
//! exclusion keys on the stem, while record identity and cross-dir dedup key on
//! `session_meta.id` (the bare uuid).
//!
//! tokscale's `codex.rs` is prior-art only; its fork/subagent replay handling is
//! deliberately NOT ported (out of scope for this slice).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::Path;

use crate::{CodexScan, InteractiveRecord, Tokens};

/// Scan the Codex session store into interactive records (one per session ×
/// model). Walks `codex_dir/sessions` then `codex_dir/archived_sessions`; a
/// missing base or subdir yields no records (never an error). Sessions whose
/// rollout stem OR `session_meta.id` is in `run_session_ids` are Ralphy runs',
/// never interactive, and are excluded. Cross-dir dedup is first-wins on the
/// record `session_id`. `since` drops records whose `last_ts` is strictly before
/// it (§6: an unparseable bound or record keeps the record).
pub fn scan_codex(input: &CodexScan) -> Vec<InteractiveRecord> {
    scan_codex_with_stems(input)
        .into_iter()
        .map(|(_, record)| record)
        .collect()
}

/// Recovery needs the rollout stem as an alias because run ledger rows use it,
/// while interactive records keep `session_meta.id` as their identity.
pub(crate) fn scan_codex_with_stems(input: &CodexScan) -> Vec<(String, InteractiveRecord)> {
    let mut records = Vec::new();
    // slug → resolved git actor email, computed at most once per attributed repo.
    let mut email_cache: HashMap<String, Option<String>> = HashMap::new();
    // Record `session_id` already emitted: first-wins, so a `sessions/` copy
    // shadows an `archived_sessions/` duplicate (scanned second).
    let mut seen: HashSet<String> = HashSet::new();

    // `sessions/` BEFORE `archived_sessions/` — the dedup order is load-bearing.
    let dirs = [
        input.codex_dir.join("sessions"),
        input.codex_dir.join("archived_sessions"),
    ];
    for dir in &dirs {
        for file in crate::walk::files_under(dir, crate::walk::is_jsonl) {
            let Some(session) = read_session(&file, input, &mut seen) else {
                continue;
            };
            let (project, actor_email) =
                attribute(session.cwd.as_deref(), input.repos, &mut email_cache);
            records.extend(session.into_records(project, actor_email));
        }
    }

    retain_since(&mut records, input.since);
    records
}

/// One rollout file that passed the exclusions and the dedup.
struct Session {
    stem: String,
    session_id: String,
    cwd: Option<String>,
    models: BTreeMap<String, ModelAgg>,
}

/// Read one rollout. `None` for an unreadable file, a run-owned session (by
/// stem, then by `session_meta.id`), and a `session_id` already emitted.
/// `seen.insert` runs after both exclusions, so an excluded session never
/// shadows a later copy.
fn read_session(file: &Path, input: &CodexScan, seen: &mut HashSet<String>) -> Option<Session> {
    let stem = file.file_stem().and_then(|s| s.to_str())?.to_string();
    // Run-owned exclusion, primary key: the rollout file stem (ADR-0033 §5).
    if input.run_session_ids.contains(&stem) {
        return None;
    }
    let text = fs::read_to_string(file).ok()?;
    let (meta_id, cwd, models) = parse_rollout(&text);
    // Defensive second exclusion on `session_meta.id` (the record identity).
    if meta_id
        .as_ref()
        .is_some_and(|mid| input.run_session_ids.contains(mid))
    {
        return None;
    }
    let session_id = meta_id.unwrap_or_else(|| stem.clone());
    if !seen.insert(session_id.clone()) {
        return None; // a `sessions/` copy already won
    }
    Some(Session {
        stem,
        session_id,
        cwd,
        models,
    })
}

/// cwd → project attribution, direct or via a linked worktree's `.git`
/// pointer (ADR-0063 §5). No match → project/actor None.
fn attribute(
    cwd: Option<&str>,
    repos: &[crate::RegisteredRepo],
    email_cache: &mut HashMap<String, Option<String>>,
) -> (Option<String>, Option<String>) {
    let matched = cwd.and_then(|c| crate::attribution::find_repo(repos, c));
    let project = matched.map(|r| r.slug.clone());
    let actor_email = matched.and_then(|r| {
        email_cache
            .entry(r.slug.clone())
            .or_insert_with(|| ralphy_git_read::user_email(Path::new(&r.path)))
            .clone()
    });
    (project, actor_email)
}

impl Session {
    /// One record per model, each paired with the rollout stem.
    fn into_records(
        self,
        project: Option<String>,
        actor_email: Option<String>,
    ) -> Vec<(String, InteractiveRecord)> {
        let Session {
            stem,
            session_id,
            models,
            ..
        } = self;
        models
            .into_iter()
            .map(|(model, agg)| {
                let record = InteractiveRecord {
                    agent: "codex".to_string(),
                    model,
                    session_id: session_id.clone(),
                    project: project.clone(),
                    actor_email: actor_email.clone(),
                    // Codex `input_tokens` INCLUDES the cached subset, so subtract
                    // it out; `cache_creation` is always 0 (no write split).
                    tokens: Some(Tokens {
                        input: agg.totals.input.saturating_sub(agg.totals.cached),
                        output: agg.totals.output,
                        cache_read: agg.totals.cached,
                        cache_creation: 0,
                    }),
                    first_ts: agg.first_ts.unwrap_or_default(),
                    last_ts: agg.last_ts.unwrap_or_default(),
                    lower_bound: false,
                };
                (stem.clone(), record)
            })
            .collect()
    }
}

/// Drop records whose `last_ts` is strictly before `since`; an unparseable
/// bound or record keeps the record.
fn retain_since(records: &mut Vec<(String, InteractiveRecord)>, since: Option<&str>) {
    let Some(since_dt) = since.and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok()) else {
        return;
    };
    records.retain(
        |(_, r)| match chrono::DateTime::parse_from_rfc3339(&r.last_ts) {
            Ok(last) => last >= since_dt,
            Err(_) => true, // never hide spend on a parse miss
        },
    );
}

/// The three cumulative Codex token counters. `input` is the vendor's raw
/// `input_tokens` (cached subset INCLUDED); `cached` is `cached_input_tokens`.
#[derive(Default, Clone, Copy)]
struct RawTotals {
    input: u64,
    output: u64,
    cached: u64,
}

/// Per-model accumulator: the summed guarded per-field deltas plus the ts span of
/// the events that contributed them.
#[derive(Default)]
struct ModelAgg {
    totals: RawTotals,
    first_ts: Option<String>,
    last_ts: Option<String>,
}

/// Parse one rollout into `(session_meta.id, session_meta.cwd, per-model deltas)`.
///
/// The delta core (Decision 2): a session-wide `prev` watermark holds the last
/// ACCEPTED cumulative snapshot. For each populated `total_token_usage`, accept it
/// only when EVERY field `>= prev` — then add the per-field delta to the current
/// model's aggregate and advance `prev`; otherwise (a regression, or any field
/// still below the watermark) contribute nothing and leave `prev` unchanged. Each
/// accepted delta is attributed to `current_model` — the latest
/// `turn_context.payload.model` seen (rollouts carry no per-event model), default
/// `"unknown"`.
fn parse_rollout(jsonl: &str) -> (Option<String>, Option<String>, BTreeMap<String, ModelAgg>) {
    let mut head = Header {
        meta_id: None,
        cwd: None,
        current_model: "unknown".to_string(),
    };
    let mut prev = RawTotals::default();
    let mut models: BTreeMap<String, ModelAgg> = BTreeMap::new();

    for line in jsonl.lines() {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line.trim()) else {
            continue;
        };
        // The real fields live under `payload`; fall back to the value itself so a
        // flat shape still parses (mirrors the adapter).
        let payload = value.get("payload").unwrap_or(&value);
        head.track(value.get("type").and_then(|v| v.as_str()), payload);

        let Some(snap) = read_snapshot(payload) else {
            continue;
        };
        // The monotonic watermark: a snapshot with any field below `prev` is a
        // compaction/cap rewrite — drop it whole, keep the peak.
        if snap.is_below(&prev) {
            continue;
        }
        let ts = value.get("timestamp").and_then(|v| v.as_str());
        models
            .entry(head.current_model.clone())
            .or_default()
            .add_delta(&snap, &prev, ts);
        prev = snap;
    }

    (head.meta_id, head.cwd, models)
}

/// What the header lines of a rollout set: the first `session_meta` id and cwd,
/// and the latest model.
struct Header {
    meta_id: Option<String>,
    cwd: Option<String>,
    current_model: String,
}

impl Header {
    /// Take the `session_meta` id and cwd (first wins) or the `turn_context`
    /// model (last wins) of one line; `kind` is the line's own `type`.
    fn track(&mut self, kind: Option<&str>, payload: &serde_json::Value) {
        let text = |k: &str| payload.get(k).and_then(|v| v.as_str()).map(str::to_string);
        match kind {
            Some("session_meta") => {
                if self.meta_id.is_none() {
                    self.meta_id = text("id");
                }
                if self.cwd.is_none() {
                    self.cwd = text("cwd");
                }
            }
            Some("turn_context") => {
                if let Some(m) = text("model") {
                    self.current_model = m;
                }
            }
            _ => {}
        }
    }
}

/// The cumulative snapshot of a `token_count` payload. `None` for any other
/// payload, and for the `{}`/null `total_token_usage` placeholders.
fn read_snapshot(payload: &serde_json::Value) -> Option<RawTotals> {
    if payload.get("type").and_then(|v| v.as_str()) != Some("token_count") {
        return None;
    }
    let ttu = payload.get("info")?.get("total_token_usage")?;
    if !ttu.is_object() {
        return None;
    }
    let field = |k: &str| ttu.get(k).and_then(|v| v.as_u64()).unwrap_or(0);
    Some(RawTotals {
        input: field("input_tokens"),
        output: field("output_tokens"),
        cached: field("cached_input_tokens"),
    })
}

impl RawTotals {
    /// `true` when any field is below the same field of `prev`.
    fn is_below(&self, prev: &RawTotals) -> bool {
        self.input < prev.input || self.output < prev.output || self.cached < prev.cached
    }
}

impl ModelAgg {
    /// Add the per-field delta from `prev` to `snap` and widen the ts span.
    fn add_delta(&mut self, snap: &RawTotals, prev: &RawTotals, ts: Option<&str>) {
        self.totals.input += snap.input - prev.input;
        self.totals.output += snap.output - prev.output;
        self.totals.cached += snap.cached - prev.cached;
        let Some(ts) = ts else {
            return;
        };
        if self.first_ts.as_deref().is_none_or(|cur| ts_lt(ts, cur)) {
            self.first_ts = Some(ts.to_string());
        }
        if self.last_ts.as_deref().is_none_or(|cur| ts_lt(cur, ts)) {
            self.last_ts = Some(ts.to_string());
        }
    }
}

/// `a < b` for two RFC3339 timestamp strings, comparing parsed instants (a `…Z`
/// form and a `+00:00` offset order correctly), lexical fallback on a parse miss.
/// Verbatim from `claude.rs` (ADR-0033 §7 accepts per-vendor duplication).
fn ts_lt(a: &str, b: &str) -> bool {
    match (
        chrono::DateTime::parse_from_rfc3339(a),
        chrono::DateTime::parse_from_rfc3339(b),
    ) {
        (Ok(a), Ok(b)) => a < b,
        _ => a < b,
    }
}

#[cfg(test)]
mod tests;
