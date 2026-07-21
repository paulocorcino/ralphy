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
//!   `cached: 8133`), but [`Tokens`](crate::Tokens)' four buckets are DISJOINT —
//!   consumers SUM them. So the reported `input` is `input − cached` and
//!   `cache_read` is `cached`, mirroring `codex.rs`. Reporting the raw `input`
//!   beside `cache_read` would bill every cache hit twice.
//!
//! LOWER BOUND: every run also makes a silent `utility_router` model call whose
//! tokens are NEVER written to disk (ADR-0043 D10, measured at 20–35% of the
//! envelope total). What this scan reports is therefore a floor, not the bill.
//! Every record this module emits carries `lower_bound: true` so the operator is
//! never shown the floor as a total; the workbench's Usage modal renders it as
//! `≥ n (lower bound)` (`assets/ui/app.js::usageTokens`).

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::Path;

use crate::{GeminiScan, InteractiveRecord, Tokens};

/// Scan the Gemini store into interactive records (one per session × model).
/// Enumerates every project directory under `<gemini_dir>/tmp/`, maps it back to
/// a registered repo through its `.project_root` sibling, and folds both
/// `chats/*.jsonl` and the nested `chats/<parent-sid>/*.jsonl` subagent logs.
/// A missing, unreadable or malformed store contributes nothing and NEVER
/// errors. Sessions whose id is in `run_session_ids` are Ralphy runs', never
/// interactive, and are excluded. `since` drops records whose `last_ts` is
/// strictly before it (§6: an unparseable bound or record keeps the record).
pub fn scan_gemini(input: &GeminiScan) -> Vec<InteractiveRecord> {
    let mut out: Vec<InteractiveRecord> = Vec::new();
    // path → resolved git actor email, computed at most once per attributed repo
    // (mirrors `cursor.rs`): the resolver spawns `git`.
    let mut email_cache: HashMap<String, Option<String>> = HashMap::new();

    let Ok(projects) = fs::read_dir(input.gemini_dir.join("tmp")) else {
        return out;
    };
    for project in projects.flatten() {
        let dir = project.path();
        let root = fs::read_to_string(dir.join(".project_root")).unwrap_or_default();
        let root = root.trim();
        let (slug, actor_email) = attribute(input, &mut email_cache, root);

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
            for (model, tokens) in fold.by_model {
                out.push(InteractiveRecord {
                    agent: "gemini".to_string(),
                    model,
                    session_id: fold.session_id.clone(),
                    project: slug.clone(),
                    actor_email: actor_email.clone(),
                    tokens: Some(tokens),
                    first_ts: fold.first_ts.clone(),
                    last_ts: fold.last_ts.clone(),
                    lower_bound: true,
                });
            }
        }
    }

    out.sort_by(|a, b| {
        (&a.session_id, &a.model)
            .cmp(&(&b.session_id, &b.model))
            .then_with(|| a.first_ts.cmp(&b.first_ts))
    });

    if let Some(since) = input.since {
        if let Ok(since_dt) = chrono::DateTime::parse_from_rfc3339(since) {
            out.retain(|r| match chrono::DateTime::parse_from_rfc3339(&r.last_ts) {
                Ok(last) => last >= since_dt,
                Err(_) => true, // never hide spend on a parse miss
            });
        }
    }
    out
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
    let mut session_id: Option<String> = None;
    let mut first_ts = String::new();
    let mut last_ts = String::new();
    let mut by_model: BTreeMap<String, Tokens> = BTreeMap::new();

    for line in lines.lines() {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue; // a malformed line contributes nothing
        };
        if let Some(id) = value.get("sessionId").and_then(|v| v.as_str()) {
            session_id.get_or_insert_with(|| id.to_string());
        }
        if let Some(start) = value.get("startTime").and_then(|v| v.as_str()) {
            if first_ts.is_empty() || earlier(start, &first_ts) {
                first_ts = start.to_string();
            }
        }
        // `lastUpdated` appears both on the header and inside `$set` mutations.
        for updated in [value.get("lastUpdated"), value.pointer("/$set/lastUpdated")]
            .into_iter()
            .flatten()
            .filter_map(|v| v.as_str())
        {
            if last_ts.is_empty() || earlier(&last_ts, updated) {
                last_ts = updated.to_string();
            }
        }
        let (Some(tokens), Some(model)) = (
            value.get("tokens"),
            value.get("model").and_then(|v| v.as_str()),
        ) else {
            continue;
        };
        let n = |k: &str| tokens.get(k).and_then(|v| v.as_u64()).unwrap_or(0);
        let (input, cached) = (n("input"), n("cached"));
        // A record with no `total` cannot be differenced — reconstruct the sum
        // from the parts instead, or the turn would report input-only spend.
        let total = match tokens.get("total").and_then(|v| v.as_u64()) {
            Some(t) => t,
            None => input + n("output") + n("thoughts"),
        };
        let agg = by_model.entry(model.to_string()).or_default();
        // `input` INCLUDES `cached` in this store (live: 14134 = 8133 cached +
        // 6001 fresh, and 14134 + 37 output + 218 thoughts = 14389 total), but
        // `Tokens`' four buckets are DISJOINT — a consumer sums them. Subtract the
        // cached subset out, exactly as `codex.rs` does for `input_tokens`.
        agg.input += input.saturating_sub(cached);
        // `total` already carries `thoughts`; the bare `output` field does not.
        agg.output += total.saturating_sub(input);
        agg.cache_read += cached;
    }

    let session_id = session_id?;
    if last_ts.is_empty() {
        last_ts = first_ts.clone();
    }
    Some(Fold {
        session_id,
        by_model,
        first_ts,
        last_ts,
    })
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
    input: &GeminiScan,
    cache: &mut HashMap<String, Option<String>>,
    root: &str,
) -> (Option<String>, Option<String>) {
    if root.is_empty() {
        return (None, None);
    }
    match input.repos.iter().find(|r| paths_eq(&r.path, root)) {
        Some(r) => (
            Some(r.slug.clone()),
            cache
                .entry(r.slug.clone())
                .or_insert_with(|| repo_actor_email(&r.path))
                .clone(),
        ),
        None => (None, None),
    }
}

/// Normalize a filesystem path for a case-insensitive compare: `\` → `/`, trailing
/// `/` trimmed. Duplicated from `cursor.rs` (ADR-0033 §7 accepts per-vendor
/// duplication).
fn normalize_path(p: &str) -> String {
    p.replace('\\', "/").trim_end_matches('/').to_string()
}

/// True when two paths name the same directory. Duplicated from `cursor.rs`.
fn paths_eq(a: &str, b: &str) -> bool {
    normalize_path(a).eq_ignore_ascii_case(&normalize_path(b))
}

/// `git config user.email` for the attributed repo (ADR-0008 D7). `None` on a
/// non-zero exit or empty output. Duplicated from `cursor.rs`.
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    /// The header record, verbatim in shape from the spike's captured store
    /// (`docs/research/gemini-cli-adapter-spike.md`).
    const HEADER: &str = r#"{"sessionId":"ralphy-probe-p1p2p3p4p6","projectHash":"3c489ab0","startTime":"2026-07-21T00:56:00Z","lastUpdated":"2026-07-21T01:00:00Z","kind":"main"}"#;

    /// One assistant turn, reproduced VERBATIM from the spike's captured record.
    /// A format change reds against the same bytes the spike observed.
    const TURN: &str = r#"{"id":"78d80d17","type":"gemini","content":"OK","tokens":{"input":20637,"output":30,"cached":0,"thoughts":257,"tool":0,"total":20924},"model":"gemini-3.1-pro-preview-customtools"}"#;

    /// A turn with a REAL cache hit, copied verbatim off this host's live store
    /// (`~/.gemini/tmp/*/chats/`). The arithmetic that matters:
    /// `14134 + 37 + 218 = 14389`, so `total` carries `thoughts`, AND
    /// `cached: 8133 < input: 14134`, so `input` carries the cached subset.
    const CACHED_TURN: &str = r#"{"id":"c1","type":"gemini","content":"OK","tokens":{"input":14134,"output":37,"cached":8133,"thoughts":218,"tool":0,"total":14389},"model":"gemini-3.5-flash"}"#;

    /// A subagent turn on its own model, from the same spike observation
    /// (17 595 tokens on `gemini-3.5-flash`).
    const SUBAGENT: &str = r#"{"sessionId":"78d80d17-sub","startTime":"2026-07-21T00:58:00Z","lastUpdated":"2026-07-21T00:59:00Z","kind":"subagent"}
{"id":"s1","type":"gemini","content":"OK","tokens":{"input":17000,"output":95,"cached":0,"thoughts":500,"tool":0,"total":17595},"model":"gemini-3.5-flash"}"#;

    fn seed(base: &Path, project: &str, root: &str, rel: &str, body: &str) {
        let file = base.join("tmp").join(project).join("chats").join(rel);
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(&file, body).unwrap();
        fs::write(base.join("tmp").join(project).join(".project_root"), root).unwrap();
    }

    fn scan(base: &Path) -> Vec<InteractiveRecord> {
        scan_gemini(&GeminiScan {
            gemini_dir: base,
            run_session_ids: &HashSet::new(),
            repos: &[],
            since: None,
        })
    }

    #[test]
    fn a_single_turn_folds_total_minus_input_as_billable_output() {
        let tmp = tempfile::tempdir().unwrap();
        seed(
            tmp.path(),
            "fincal",
            "c:\\dev\\fincal",
            "session-x.jsonl",
            &format!("{HEADER}\n{TURN}\n"),
        );

        let records = scan(tmp.path());
        assert_eq!(records.len(), 1, "{records:?}");
        assert_eq!(records[0].agent, "gemini");
        assert_eq!(records[0].session_id, "ralphy-probe-p1p2p3p4p6");
        assert_eq!(records[0].model, "gemini-3.1-pro-preview-customtools");
        assert_eq!(
            records[0].tokens,
            Some(Tokens {
                input: 20637,
                // 20924 − 20637: the bare `output` field (30) cannot produce it.
                output: 287,
                cache_read: 0,
                cache_creation: 0,
            })
        );
        assert_eq!(records[0].first_ts, "2026-07-21T00:56:00Z");
        assert_eq!(records[0].last_ts, "2026-07-21T01:00:00Z");
    }

    /// ADR-0043 D10: the silent `utility_router` call is never on disk, so every
    /// Gemini record is a floor and must say so to its consumers.
    #[test]
    fn the_record_is_flagged_a_lower_bound() {
        let tmp = tempfile::tempdir().unwrap();
        seed(
            tmp.path(),
            "fincal",
            "c:\\dev\\fincal",
            "session-x.jsonl",
            &format!("{HEADER}\n{TURN}\n"),
        );

        let records = scan(tmp.path());
        assert_eq!(records.len(), 1, "{records:?}");
        assert!(records[0].lower_bound);
    }

    /// ADR-0040 C6's bill-multiplier trap in the opposite direction: a keep-last
    /// implementation returns `20637`/`287` here and reds.
    ///
    /// The two turns are DIFFERENT records on DIFFERENT models, so this also reds
    /// a fold that folds one turn and multiplies by the turn count, and one that
    /// mis-keys the per-model aggregate — both of which two byte-identical turns
    /// would satisfy.
    #[test]
    fn usage_is_summed_not_kept_last() {
        let tmp = tempfile::tempdir().unwrap();
        seed(
            tmp.path(),
            "fincal",
            "c:\\dev\\fincal",
            "session-x.jsonl",
            &format!("{HEADER}\n{TURN}\n{TURN}\n{CACHED_TURN}\n"),
        );

        let mut records = scan(tmp.path());
        // One session, two models → two records, both under the same session id.
        assert_eq!(records.len(), 2, "{records:?}");
        records.sort_by(|a, b| a.model.cmp(&b.model));
        assert!(records
            .iter()
            .all(|r| r.session_id == "ralphy-probe-p1p2p3p4p6"));

        let pro = records[0].tokens.clone().unwrap();
        assert_eq!(records[0].model, "gemini-3.1-pro-preview-customtools");
        assert_eq!(pro.input, 41274, "two identical turns SUM");
        assert_eq!(pro.output, 574);

        let flash = records[1].tokens.clone().unwrap();
        assert_eq!(records[1].model, "gemini-3.5-flash");
        assert_eq!(
            (flash.input, flash.output, flash.cache_read),
            (6001, 255, 8133),
            "the third turn's model must keep its OWN aggregate"
        );
    }

    /// `Tokens`' four buckets are DISJOINT — every consumer sums them
    /// (`app.js` renders `input + output + cache_read + cache_creation`). This
    /// store's `input` INCLUDES `cached`, so the cached subset must be subtracted
    /// out of `input`, the way `codex.rs` does. A fold that reports the raw
    /// `input` alongside `cache_read` bills the cached tokens twice; one that
    /// drops `cache_read` loses them. Both red here, and no `cached: 0` fixture
    /// can discriminate either.
    #[test]
    fn a_cache_hit_is_reported_once_not_folded_into_input_as_well() {
        let tmp = tempfile::tempdir().unwrap();
        seed(
            tmp.path(),
            "fincal",
            "c:\\dev\\fincal",
            "session-x.jsonl",
            &format!("{HEADER}\n{CACHED_TURN}\n"),
        );

        let records = scan(tmp.path());
        assert_eq!(records.len(), 1, "{records:?}");
        let tokens = records[0].tokens.clone().unwrap();
        assert_eq!(
            tokens,
            Tokens {
                input: 6001,      // 14134 − 8133: the FRESH prompt only
                output: 255,      // 14389 − 14134: output + thoughts
                cache_read: 8133, // reported here, and only here
                cache_creation: 0,
            }
        );
        // The whole turn is still accounted for: nothing was lost by splitting it.
        assert_eq!(
            tokens.input + tokens.output + tokens.cache_read,
            14389,
            "the disjoint buckets must still sum to the store's own `total`"
        );
    }

    /// A turn with no `total` cannot be differenced. Reconstructing the sum from
    /// the parts keeps its output; a bare `saturating_sub` clamps output to 0 and
    /// silently reports input-only spend.
    #[test]
    fn a_turn_missing_total_reconstructs_its_output_from_the_parts() {
        let tmp = tempfile::tempdir().unwrap();
        seed(
            tmp.path(),
            "fincal",
            "c:\\dev\\fincal",
            "session-x.jsonl",
            &format!(
                "{HEADER}\n{}\n",
                r#"{"id":"n1","type":"gemini","tokens":{"input":100,"output":7,"cached":0,"thoughts":11},"model":"m"}"#
            ),
        );

        let tokens = scan(tmp.path())[0].tokens.clone().unwrap();
        assert_eq!(tokens.input, 100);
        assert_eq!(tokens.output, 18, "output + thoughts, not a clamped 0");
    }

    #[test]
    fn a_nested_subagent_file_is_counted() {
        let tmp = tempfile::tempdir().unwrap();
        seed(
            tmp.path(),
            "fincal",
            "c:\\dev\\fincal",
            "session-x.jsonl",
            &format!("{HEADER}\n{TURN}\n"),
        );
        seed(
            tmp.path(),
            "fincal",
            "c:\\dev\\fincal",
            "ralphy-probe-policy/78d80d17.jsonl",
            SUBAGENT,
        );

        // A `chats/*.jsonl` glob returns one record and reds here.
        let records = scan(tmp.path());
        assert_eq!(records.len(), 2, "{records:?}");
        let sub = records
            .iter()
            .find(|r| r.model == "gemini-3.5-flash")
            .expect("the nested subagent log must contribute its own record");
        assert_eq!(sub.session_id, "78d80d17-sub");
        assert_eq!(sub.tokens.clone().unwrap().output, 595);
    }

    #[test]
    fn a_missing_or_malformed_store_is_empty_never_an_error() {
        assert!(scan(Path::new("does-not-exist")).is_empty());

        let empty = tempfile::tempdir().unwrap();
        fs::create_dir_all(empty.path().join("tmp")).unwrap();
        assert!(scan(empty.path()).is_empty());

        let garbage = tempfile::tempdir().unwrap();
        seed(
            garbage.path(),
            "fincal",
            "c:\\dev\\fincal",
            "session-x.jsonl",
            "not json\nnot json either\n",
        );
        assert!(scan(garbage.path()).is_empty());
    }

    /// `lastUpdated` lives on `$set` mutation records after the header — trusting
    /// the header's copy alone dates every live session to its first second.
    #[test]
    fn last_ts_follows_the_set_mutations_not_just_the_header() {
        let tmp = tempfile::tempdir().unwrap();
        seed(
            tmp.path(),
            "fincal",
            "c:\\dev\\fincal",
            "session-x.jsonl",
            &format!(
                "{HEADER}\n{TURN}\n{}\n",
                r#"{"$set":{"lastUpdated":"2026-07-21T02:00:00Z"}}"#
            ),
        );

        let records = scan(tmp.path());
        assert_eq!(records[0].last_ts, "2026-07-21T02:00:00Z");
    }

    #[test]
    fn a_session_is_attributed_through_its_project_root_sibling() {
        let tmp = tempfile::tempdir().unwrap();
        seed(
            tmp.path(),
            "fincal",
            "c:\\dev\\fincal",
            "session-x.jsonl",
            &format!("{HEADER}\n{TURN}\n"),
        );

        let records = scan_gemini(&GeminiScan {
            gemini_dir: tmp.path(),
            run_session_ids: &HashSet::new(),
            repos: &[crate::RegisteredRepo {
                slug: "acme/fincal".to_string(),
                // `.project_root` holds the lowercased Windows form: the match
                // must cross both the separator and the case difference.
                path: "C:/Dev/FinCal/".to_string(),
            }],
            since: None,
        });
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].project.as_deref(), Some("acme/fincal"));
    }

    #[test]
    fn a_run_owned_session_id_is_excluded() {
        let tmp = tempfile::tempdir().unwrap();
        seed(
            tmp.path(),
            "fincal",
            "c:\\dev\\fincal",
            "session-x.jsonl",
            &format!("{HEADER}\n{TURN}\n"),
        );
        let owned: HashSet<String> = ["ralphy-probe-p1p2p3p4p6".to_string()]
            .into_iter()
            .collect();

        let records = scan_gemini(&GeminiScan {
            gemini_dir: tmp.path(),
            run_session_ids: &owned,
            repos: &[],
            since: None,
        });
        assert!(records.is_empty(), "{records:?}");
    }

    /// §6: `since` is INCLUSIVE at the boundary, and an unparseable bound must
    /// not filter at all.
    #[test]
    fn since_is_inclusive_and_never_filters_on_an_unparseable_bound() {
        let tmp = tempfile::tempdir().unwrap();
        seed(
            tmp.path(),
            "fincal",
            "c:\\dev\\fincal",
            "session-x.jsonl",
            &format!("{HEADER}\n{TURN}\n"),
        );

        let with_since = |since: &str| {
            scan_gemini(&GeminiScan {
                gemini_dir: tmp.path(),
                run_session_ids: &HashSet::new(),
                repos: &[],
                since: Some(since),
            })
        };
        assert_eq!(with_since("2026-07-21T01:00:00Z").len(), 1, "inclusive");
        assert_eq!(with_since("not-a-timestamp").len(), 1);
        assert!(with_since("2026-07-21T01:00:01Z").is_empty());
    }

    #[test]
    fn the_scan_writes_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        seed(
            tmp.path(),
            "fincal",
            "c:\\dev\\fincal",
            "session-x.jsonl",
            &format!("{HEADER}\n{TURN}\n"),
        );
        let before = fs::read_to_string(
            tmp.path()
                .join("tmp/fincal/chats/session-x.jsonl")
                .to_string_lossy()
                .to_string(),
        )
        .unwrap();
        let meta = fs::metadata(tmp.path().join("tmp/fincal/chats/session-x.jsonl")).unwrap();

        assert_eq!(scan(tmp.path()).len(), 1);
        let after = fs::metadata(tmp.path().join("tmp/fincal/chats/session-x.jsonl")).unwrap();
        assert_eq!(after.modified().unwrap(), meta.modified().unwrap());
        assert_eq!(
            fs::read_to_string(tmp.path().join("tmp/fincal/chats/session-x.jsonl")).unwrap(),
            before
        );
    }
}
