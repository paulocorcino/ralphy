//! The Gemini module of the usage scan (ADR-0033 §2/§6/§7, ADR-0043 D10). Parses
//! the JSONL session event logs the Gemini CLI leaves under
//! `~/.gemini/tmp/<basename>/chats/` into per-session × model interactive
//! records.
//!
//! The store is an append-only event log, not a document: a header record, then
//! `$set` mutation records, then one `type: "gemini"` record per assistant turn
//! carrying that turn's `tokens` block and `model`. Usage is INCREMENTAL — the
//! records are SUMMED, never kept-last (ADR-0040 C6: getting this backwards
//! multiplies the bill).
//!
//! Two arithmetic rules, both verified against live records on the build host:
//!
//! - Billable output is `tokens.total − tokens.input`, not `tokens.output`:
//!   `total` already contains `thoughts` (20637 + 30 + 257 = 20924 on the spike's
//!   captured record), so the bare `output` field under-reports every reasoning
//!   turn.
//! - `tokens.input` INCLUDES the cached subset (live: `input: 14134` with
//!   `cached: 8133`), but [`Tokens`]' four buckets are DISJOINT —
//!   consumers SUM them. So the reported `input` is `input − cached` and
//!   `cache_read` is `cached`, mirroring `codex.rs`. Reporting the raw `input`
//!   beside `cache_read` would bill every cache hit twice.
//!
//! LOWER BOUND: every run also makes a silent `utility_router` model call whose
//! tokens are NEVER written to disk (ADR-0043 D10, measured at 20–35% of the
//! envelope total). What this scan reports is therefore a floor, not the bill.
//! Every record this module emits carries `lower_bound: true` so the operator is
//! never shown the floor as a total; the Spend tab's Ledger grid renders such a
//! row's counts as `≥ n` with `(lower bound)` beside it
//! (`assets/ui/wb-spend.ts::boundMark`).

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::Path;

use crate::{since, GeminiScan, InteractiveRecord, Tokens};

/// Scan the Gemini store into interactive records (one per session × model).
/// Enumerates every project directory under `<gemini_dir>/tmp/`, maps it back to
/// a registered repo through its `.project_root` sibling, and folds both
/// `chats/*.jsonl` and the nested `chats/<parent-sid>/*.jsonl` subagent logs.
/// A missing, unreadable or malformed store contributes nothing and NEVER
/// errors. Sessions whose id is in `run_session_ids` are Ralphy runs', never
/// interactive, and are excluded. `since` drops records whose `last_ts` is
/// strictly before it (§6: an unparseable bound or record keeps the record).
pub fn scan_gemini(input: &GeminiScan<'_>) -> Vec<InteractiveRecord> {
    let mut out: Vec<InteractiveRecord> = Vec::new();
    // path → resolved git actor email, computed at most once per attributed repo
    // (mirrors `cursor.rs`): the resolver spawns `git`.
    let mut email_cache: HashMap<String, Option<String>> = HashMap::new();

    let Ok(projects) = fs::read_dir(input.gemini_dir.join("tmp")) else {
        return out;
    };
    for project in projects.flatten() {
        out.extend(project_records(input, &mut email_cache, &project.path()));
    }

    out.sort_by(|a, b| {
        (&a.session_id, &a.model)
            .cmp(&(&b.session_id, &b.model))
            .then_with(|| a.first_ts.cmp(&b.first_ts))
    });

    since::retain_since(&mut out, input.since);
    out
}

/// The interactive records of one project directory under `tmp/`, attributed
/// through its `.project_root` sibling.
fn project_records(
    input: &GeminiScan<'_>,
    email_cache: &mut HashMap<String, Option<String>>,
    dir: &Path,
) -> Vec<InteractiveRecord> {
    let root = fs::read_to_string(dir.join(".project_root")).unwrap_or_default();
    let root = root.trim();
    let (slug, actor_email) = attribute(input, email_cache, root);

    let mut out = Vec::new();
    for file in chat_files(&dir.join("chats")) {
        let Ok(text) = fs::read_to_string(&file) else {
            continue;
        };
        let Some(fold) = fold_session(&text) else {
            continue;
        };
        if input.run_session_ids.contains(&fold.session_id) {
            continue;
        }
        out.extend(records_of(fold, slug.as_deref(), actor_email.as_deref()));
    }
    out
}

/// One record per model of a folded session.
fn records_of(fold: Fold, slug: Option<&str>, actor_email: Option<&str>) -> Vec<InteractiveRecord> {
    fold.by_model
        .into_iter()
        .map(|(model, tokens)| InteractiveRecord {
            agent: "gemini".to_owned(),
            model,
            session_id: fold.session_id.clone(),
            project: slug.map(str::to_owned),
            actor_email: actor_email.map(str::to_owned),
            tokens: Some(tokens),
            first_ts: fold.first_ts.clone(),
            last_ts: fold.last_ts.clone(),
            lower_bound: true,
        })
        .collect()
}

/// Every session log under `chats/`: the direct `*.jsonl` files AND the ones one
/// level down under `chats/<parent-sid>/`, where the CLI files a subagent's own
/// session. A `chats/*.jsonl` glob misses subagent consumption entirely.
fn chat_files(chats: &Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = fs::read_dir(chats) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if let Ok(nested) = fs::read_dir(&path) {
                out.extend(
                    nested
                        .flatten()
                        .map(|e| e.path())
                        .filter(|p| is_jsonl(p) && p.is_file()),
                );
            }
        } else if is_jsonl(&path) {
            out.push(path);
        }
    }
    out.sort();
    out
}

fn is_jsonl(path: &Path) -> bool {
    path.extension().is_some_and(|e| e == "jsonl")
}

/// One session log folded into its per-model token aggregates and its timestamp
/// span.
struct Fold {
    session_id: String,
    by_model: BTreeMap<String, Tokens>,
    first_ts: String,
    last_ts: String,
}

/// Fold one session log's lines. `None` when no header record names a
/// `sessionId`. `last_ts` is the LATEST `lastUpdated` seen anywhere, header or
/// `$set` mutation: the header's copy is written at session creation and never
/// rewritten, so trusting it alone dates every session to its first second.
fn fold_session(lines: &str) -> Option<Fold> {
    let mut open = OpenFold::default();
    // A malformed line contributes nothing.
    for value in lines
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
    {
        open.add_record(&value);
    }
    open.close()
}

/// A [`Fold`] while its lines are read: the session id stays unknown until a
/// header record names it.
#[derive(Default)]
struct OpenFold {
    session_id: Option<String>,
    by_model: BTreeMap<String, Tokens>,
    first_ts: String,
    last_ts: String,
}

impl OpenFold {
    /// Adds what one record carries: the first `sessionId`, the span bounds,
    /// and an assistant turn's tokens.
    fn add_record(&mut self, value: &serde_json::Value) {
        if let Some(id) = value.get("sessionId").and_then(|v| v.as_str()) {
            self.session_id.get_or_insert_with(|| id.to_owned());
        }
        if let Some(start) = value.get("startTime").and_then(|v| v.as_str()) {
            widen_first(&mut self.first_ts, start);
        }
        // `lastUpdated` appears both on the header and inside `$set` mutations.
        for updated in [value.get("lastUpdated"), value.pointer("/$set/lastUpdated")]
            .into_iter()
            .flatten()
            .filter_map(|v| v.as_str())
        {
            widen_last(&mut self.last_ts, updated);
        }
        if let (Some(tokens), Some(model)) = (
            value.get("tokens"),
            value.get("model").and_then(|v| v.as_str()),
        ) {
            add_turn(&mut self.by_model, tokens, model);
        }
    }

    /// The finished fold, or `None` when no record named a session. A session
    /// with no `lastUpdated` ends where it starts.
    fn close(self) -> Option<Fold> {
        let session_id = self.session_id?;
        let last_ts = if self.last_ts.is_empty() {
            self.first_ts.clone()
        } else {
            self.last_ts
        };
        Some(Fold {
            session_id,
            by_model: self.by_model,
            first_ts: self.first_ts,
            last_ts,
        })
    }
}

/// Move `first_ts` back to `start` when `start` is the earliest seen.
fn widen_first(first_ts: &mut String, start: &str) {
    if first_ts.is_empty() || earlier(start, first_ts) {
        start.clone_into(first_ts);
    }
}

/// Move `last_ts` forward to `updated` when `updated` is the latest seen.
fn widen_last(last_ts: &mut String, updated: &str) {
    if last_ts.is_empty() || earlier(last_ts, updated) {
        updated.clone_into(last_ts);
    }
}

/// Add one assistant turn's `tokens` block to the aggregate of its `model`.
fn add_turn(by_model: &mut BTreeMap<String, Tokens>, tokens: &serde_json::Value, model: &str) {
    let n = |k: &str| {
        tokens
            .get(k)
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0)
    };
    let (input, cached) = (n("input"), n("cached"));
    // A record with no `total` cannot be differenced — reconstruct the sum
    // from the parts instead, or the turn would report input-only spend.
    let total = match tokens.get("total").and_then(serde_json::Value::as_u64) {
        Some(t) => t,
        None => input + n("output") + n("thoughts"),
    };
    let agg = by_model.entry(model.to_owned()).or_default();
    // `input` INCLUDES `cached` in this store (live: 14134 = 8133 cached +
    // 6001 fresh, and 14134 + 37 output + 218 thoughts = 14389 total), but
    // `Tokens`' four buckets are DISJOINT — a consumer sums them. Subtract the
    // cached subset out, exactly as `codex.rs` does for `input_tokens`.
    agg.input += input.saturating_sub(cached);
    // `total` already carries `thoughts`; the bare `output` field does not.
    agg.output += total.saturating_sub(input);
    agg.cache_read += cached;
}

/// True when `a` names a strictly earlier instant than `b`. Compared as parsed
/// instants, falling back to a byte compare when either side is unparseable: this
/// store mixes `…Z` (the header) with whatever a future writer emits, and `Z`
/// sorts AFTER `+00:00` lexicographically, which would pick the wrong span end.
fn earlier(a: &str, b: &str) -> bool {
    match (
        chrono::DateTime::parse_from_rfc3339(a),
        chrono::DateTime::parse_from_rfc3339(b),
    ) {
        (Ok(a), Ok(b)) => a < b,
        _ => a < b,
    }
}

/// `(project slug, git actor email)` for the registered repo whose path is
/// `root`; `(None, None)` when none matches (§6: reported, never dropped).
fn attribute(
    input: &GeminiScan<'_>,
    cache: &mut HashMap<String, Option<String>>,
    root: &str,
) -> (Option<String>, Option<String>) {
    if root.is_empty() {
        return (None, None);
    }
    match crate::attribution::find_repo(input.repos, root) {
        Some(r) => (
            Some(r.slug.clone()),
            cache
                .entry(r.slug.clone())
                .or_insert_with(|| ralphy_git_read::user_email(Path::new(&r.path)))
                .clone(),
        ),
        None => (None, None),
    }
}

#[cfg(test)]
mod tests;
