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
use std::path::{Path, PathBuf};

use crate::{CodexScan, InteractiveRecord, Tokens};

/// Scan the Codex session store into interactive records (one per session ×
/// model). Walks `codex_dir/sessions` then `codex_dir/archived_sessions`; a
/// missing base or subdir yields no records (never an error). Sessions whose
/// rollout stem OR `session_meta.id` is in `run_session_ids` are Ralphy runs',
/// never interactive, and are excluded. Cross-dir dedup is first-wins on the
/// record `session_id`. `since` drops records whose `last_ts` is strictly before
/// it (§6: an unparseable bound or record keeps the record).
pub fn scan_codex(input: &CodexScan) -> Vec<InteractiveRecord> {
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
        for file in jsonl_files(dir) {
            let Some(stem) = file
                .file_stem()
                .and_then(|s| s.to_str())
                .map(str::to_string)
            else {
                continue;
            };
            // Run-owned exclusion, primary key: the rollout file stem (ADR-0033 §5).
            if input.run_session_ids.contains(&stem) {
                continue;
            }
            let Ok(text) = fs::read_to_string(&file) else {
                continue;
            };
            let (meta_id, cwd, models) = parse_rollout(&text);
            // Defensive second exclusion on `session_meta.id` (the record identity).
            if let Some(mid) = &meta_id {
                if input.run_session_ids.contains(mid) {
                    continue;
                }
            }
            let session_id = meta_id.unwrap_or(stem);
            if !seen.insert(session_id.clone()) {
                continue; // a `sessions/` copy already won
            }

            // cwd → project attribution: normalize both sides and compare
            // case-insensitively (Decision 3). No match → project/actor None.
            let matched = cwd
                .as_deref()
                .and_then(|c| input.repos.iter().find(|r| paths_eq(&r.path, c)));
            let project = matched.map(|r| r.slug.clone());
            let actor_email = matched.and_then(|r| {
                email_cache
                    .entry(r.slug.clone())
                    .or_insert_with(|| repo_actor_email(&r.path))
                    .clone()
            });

            for (model, agg) in models {
                records.push(InteractiveRecord {
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
                });
            }
        }
    }

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
    let mut meta_id: Option<String> = None;
    let mut cwd: Option<String> = None;
    let mut current_model = "unknown".to_string();
    let mut prev = RawTotals::default();
    let mut models: BTreeMap<String, ModelAgg> = BTreeMap::new();

    for line in jsonl.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        // The real fields live under `payload`; fall back to the value itself so a
        // flat shape still parses (mirrors the adapter).
        let payload = value.get("payload").unwrap_or(&value);
        match value.get("type").and_then(|v| v.as_str()) {
            Some("session_meta") => {
                if meta_id.is_none() {
                    meta_id = payload
                        .get("id")
                        .and_then(|v| v.as_str())
                        .map(str::to_string);
                }
                if cwd.is_none() {
                    cwd = payload
                        .get("cwd")
                        .and_then(|v| v.as_str())
                        .map(str::to_string);
                }
            }
            Some("turn_context") => {
                if let Some(m) = payload.get("model").and_then(|v| v.as_str()) {
                    current_model = m.to_string();
                }
            }
            _ => {}
        }

        if payload.get("type").and_then(|v| v.as_str()) != Some("token_count") {
            continue;
        }
        let Some(ttu) = payload
            .get("info")
            .and_then(|info| info.get("total_token_usage"))
        else {
            continue;
        };
        // `{}`/null `info.total_token_usage` snapshots are placeholders; ignore.
        if !ttu.is_object() {
            continue;
        }
        let field = |k: &str| ttu.get(k).and_then(|v| v.as_u64()).unwrap_or(0);
        let snap = RawTotals {
            input: field("input_tokens"),
            output: field("output_tokens"),
            cached: field("cached_input_tokens"),
        };
        // The monotonic watermark: a snapshot with any field below `prev` is a
        // compaction/cap rewrite — drop it whole, keep the peak.
        if snap.input < prev.input || snap.output < prev.output || snap.cached < prev.cached {
            continue;
        }
        let agg = models.entry(current_model.clone()).or_default();
        agg.totals.input += snap.input - prev.input;
        agg.totals.output += snap.output - prev.output;
        agg.totals.cached += snap.cached - prev.cached;
        if let Some(ts) = value.get("timestamp").and_then(|v| v.as_str()) {
            if agg.first_ts.as_deref().is_none_or(|cur| ts_lt(ts, cur)) {
                agg.first_ts = Some(ts.to_string());
            }
            if agg.last_ts.as_deref().is_none_or(|cur| ts_lt(cur, ts)) {
                agg.last_ts = Some(ts.to_string());
            }
        }
        prev = snap;
    }

    (meta_id, cwd, models)
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

/// Normalize a filesystem path for a case-insensitive compare: `\` → `/`, trailing
/// `/` trimmed. Codex's `session_meta.cwd` uses a lowercase drive + backslashes
/// (`c:\Dev\…`) while registry paths use backslashes with varying drive case.
fn normalize_path(p: &str) -> String {
    p.replace('\\', "/").trim_end_matches('/').to_string()
}

/// True when two paths name the same directory (Decision 3): normalized and
/// compared with `eq_ignore_ascii_case` (correct on Windows' case-insensitive FS).
fn paths_eq(a: &str, b: &str) -> bool {
    normalize_path(a).eq_ignore_ascii_case(&normalize_path(b))
}

/// `git config user.email` for the attributed repo (ADR-0008 D7). `None` on a
/// non-zero exit or empty output. Duplicated from `claude.rs` (ADR-0033 §7); the
/// scan crate cannot depend on core (ADR-0032), so it shells out directly.
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

/// Every `*.jsonl` under `dir`, recursively. Tolerant: an unreadable or missing
/// dir yields nothing. Order is unspecified. Duplicated from `claude.rs`.
fn jsonl_files(dir: &Path) -> Vec<PathBuf> {
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
            } else if path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
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

    fn write(dir: &Path, rel: &str, content: &str) {
        let path = dir.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    fn no_runs() -> HashSet<String> {
        HashSet::new()
    }

    /// A `session_meta` header line for a given id and cwd.
    fn meta_line(id: &str, cwd: &str) -> String {
        let cwd = cwd.replace('\\', "\\\\");
        format!(
            "{{\"timestamp\":\"2026-07-10T10:00:00Z\",\"type\":\"session_meta\",\"payload\":{{\"id\":\"{id}\",\"cwd\":\"{cwd}\"}}}}"
        )
    }

    /// A `turn_context` line naming the invocation model.
    fn model_line(model: &str) -> String {
        format!(
            "{{\"timestamp\":\"2026-07-10T10:00:00Z\",\"type\":\"turn_context\",\"payload\":{{\"model\":\"{model}\"}}}}"
        )
    }

    /// A `token_count` event with a populated cumulative `total_token_usage`.
    fn token_line(input: u64, cached: u64, output: u64, ts: &str) -> String {
        format!(
            "{{\"timestamp\":\"{ts}\",\"type\":\"event_msg\",\"payload\":{{\"type\":\"token_count\",\"info\":{{\"total_token_usage\":{{\"input_tokens\":{input},\"cached_input_tokens\":{cached},\"output_tokens\":{output}}}}}}}}}"
        )
    }

    /// A full rollout: header + model + the given token lines, one per line.
    fn rollout(id: &str, cwd: &str, model: &str, tokens: &[String]) -> String {
        let mut lines = vec![meta_line(id, cwd), model_line(model)];
        lines.extend(tokens.iter().cloned());
        lines.join("\n")
    }

    #[test]
    fn cached_subset_maps_without_double_count() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let body = rollout(
            "sess-1",
            "c:\\Dev\\x",
            "gpt-5.3-codex",
            &[token_line(1000, 800, 200, "2026-07-10T10:00:00Z")],
        );
        write(root, "sessions/2026/07/10/rollout-a.jsonl", &body);

        let records = scan_codex(&CodexScan {
            codex_dir: root,
            run_session_ids: &no_runs(),
            repos: &[],
            since: None,
        });
        assert_eq!(records.len(), 1);
        assert_eq!(
            records[0].tokens.as_ref().unwrap().input,
            200,
            "1000 - 800 cached"
        );
        assert_eq!(records[0].tokens.as_ref().unwrap().cache_read, 800);
        assert_eq!(records[0].tokens.as_ref().unwrap().cache_creation, 0);
        assert_eq!(records[0].tokens.as_ref().unwrap().output, 200);
        assert!(
            !records[0].lower_bound,
            "Codex writes every token to disk — this is a total, not a floor"
        );
    }

    #[test]
    fn cumulative_to_delta_monotonic_guard_ignores_regression() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        // input totals 100 → 300 → 250 → 280: accept 100 (+100) and 300 (+200);
        // 250 and 280 are below the 300 watermark → no delta. Mapped input = 300
        // (not take-last 280, not saturating-no-watermark 330).
        let body = rollout(
            "sess-1",
            "c:\\Dev\\x",
            "gpt-5.3-codex",
            &[
                token_line(100, 0, 0, "2026-07-10T10:00:00Z"),
                token_line(300, 0, 0, "2026-07-10T10:00:01Z"),
                token_line(250, 0, 0, "2026-07-10T10:00:02Z"),
                token_line(280, 0, 0, "2026-07-10T10:00:03Z"),
            ],
        );
        write(root, "sessions/rollout-a.jsonl", &body);

        let records = scan_codex(&CodexScan {
            codex_dir: root,
            run_session_ids: &no_runs(),
            repos: &[],
            since: None,
        });
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].tokens.as_ref().unwrap().input, 300);
    }

    #[test]
    fn record_carries_session_meta_id_and_model() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        // File stem differs from the meta id: the record must key on the meta id.
        let body = rollout(
            "019c5131-meta",
            "c:\\Dev\\x",
            "gpt-5.3-codex",
            &[token_line(10, 0, 5, "2026-07-10T10:00:00Z")],
        );
        write(root, "sessions/rollout-2026-uuid.jsonl", &body);

        let records = scan_codex(&CodexScan {
            codex_dir: root,
            run_session_ids: &no_runs(),
            repos: &[],
            since: None,
        });
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].session_id, "019c5131-meta");
        assert_eq!(records[0].model, "gpt-5.3-codex");
        assert_eq!(records[0].agent, "codex");
    }

    #[test]
    fn attributes_cwd_to_registered_repo() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let body = rollout(
            "sess-1",
            "c:\\Dev\\ralphy",
            "gpt-5.3-codex",
            &[token_line(10, 0, 0, "2026-07-10T10:00:00Z")],
        );
        write(root, "sessions/rollout-a.jsonl", &body);
        let repos = vec![RegisteredRepo {
            slug: "o/ralphy".into(),
            path: "C:\\Dev\\ralphy".into(),
        }];
        let records = scan_codex(&CodexScan {
            codex_dir: root,
            run_session_ids: &no_runs(),
            repos: &repos,
            since: None,
        });
        assert_eq!(records.len(), 1);
        assert_eq!(
            records[0].project.as_deref(),
            Some("o/ralphy"),
            "lowercase-drive backslash cwd matches drive-case registry path"
        );
    }

    #[test]
    fn unmatched_cwd_yields_null_project() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let body = rollout(
            "sess-1",
            "c:\\Dev\\elsewhere",
            "gpt-5.3-codex",
            &[token_line(10, 0, 0, "2026-07-10T10:00:00Z")],
        );
        write(root, "sessions/rollout-a.jsonl", &body);
        let repos = vec![RegisteredRepo {
            slug: "o/ralphy".into(),
            path: "C:\\Dev\\ralphy".into(),
        }];
        let records = scan_codex(&CodexScan {
            codex_dir: root,
            run_session_ids: &no_runs(),
            repos: &repos,
            since: None,
        });
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].project, None);
        assert_eq!(records[0].actor_email, None);
    }

    #[test]
    fn attributed_record_carries_git_actor_email() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("codex");
        let repo = tmp.path().join("repo");
        fs::create_dir_all(&repo).unwrap();
        let run = |args: &[&str]| {
            std::process::Command::new("git")
                .args(["-C", repo.to_str().unwrap()])
                .args(args)
                .output()
                .unwrap();
        };
        run(&["init"]);
        run(&["config", "user.email", "t@example.com"]);

        let repo_path = repo.to_string_lossy().to_string();
        let body = rollout(
            "sess-1",
            &repo_path,
            "gpt-5.3-codex",
            &[token_line(10, 0, 0, "2026-07-10T10:00:00Z")],
        );
        write(&root, "sessions/rollout-a.jsonl", &body);
        let repos = vec![RegisteredRepo {
            slug: "o/repo".into(),
            path: repo_path,
        }];
        let records = scan_codex(&CodexScan {
            codex_dir: &root,
            run_session_ids: &no_runs(),
            repos: &repos,
            since: None,
        });
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].project.as_deref(), Some("o/repo"));
        assert_eq!(records[0].actor_email.as_deref(), Some("t@example.com"));
    }

    #[test]
    fn excludes_run_owned_by_rollout_stem() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write(
            root,
            "sessions/rollout-run.jsonl",
            &rollout(
                "run-meta",
                "c:\\Dev\\x",
                "m",
                &[token_line(10, 0, 0, "2026-07-10T10:00:00Z")],
            ),
        );
        write(
            root,
            "sessions/rollout-int.jsonl",
            &rollout(
                "int-meta",
                "c:\\Dev\\x",
                "m",
                &[token_line(20, 0, 0, "2026-07-10T10:00:00Z")],
            ),
        );
        let mut runs = HashSet::new();
        runs.insert("rollout-run".to_string()); // the file STEM, not the meta id

        let records = scan_codex(&CodexScan {
            codex_dir: root,
            run_session_ids: &runs,
            repos: &[],
            since: None,
        });
        assert!(records.iter().any(|r| r.session_id == "int-meta"));
        assert!(!records.iter().any(|r| r.session_id == "run-meta"));
    }

    #[test]
    fn session_meta_id_dedups_across_sessions_and_archived() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        // Same `session_meta.id` under both dirs, different file stems. The
        // `sessions/` copy wins; exactly one record for the session.
        let body = rollout(
            "shared-id",
            "c:\\Dev\\x",
            "gpt-5.3-codex",
            &[token_line(100, 0, 0, "2026-07-10T10:00:00Z")],
        );
        write(root, "sessions/rollout-live.jsonl", &body);
        write(root, "archived_sessions/rollout-archived.jsonl", &body);

        let records = scan_codex(&CodexScan {
            codex_dir: root,
            run_session_ids: &no_runs(),
            repos: &[],
            since: None,
        });
        let count = records
            .iter()
            .filter(|r| r.session_id == "shared-id")
            .count();
        assert_eq!(count, 1, "identical session across both dirs → one record");
    }

    #[test]
    fn since_filters_by_last_ts() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write(
            root,
            "sessions/rollout-old.jsonl",
            &rollout(
                "old",
                "c:\\Dev\\x",
                "m",
                &[token_line(10, 0, 0, "2026-07-01T10:00:00Z")],
            ),
        );
        write(
            root,
            "sessions/rollout-new.jsonl",
            &rollout(
                "new",
                "c:\\Dev\\x",
                "m",
                &[token_line(20, 0, 0, "2026-07-10T10:00:00Z")],
            ),
        );
        let records = scan_codex(&CodexScan {
            codex_dir: root,
            run_session_ids: &no_runs(),
            repos: &[],
            since: Some("2026-07-05T00:00:00+00:00"),
        });
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].session_id, "new");
    }

    #[test]
    fn missing_codex_dir_contributes_zero() {
        let records = scan_codex(&CodexScan {
            codex_dir: Path::new("does-not-exist-anywhere"),
            run_session_ids: &no_runs(),
            repos: &[],
            since: None,
        });
        assert!(records.is_empty());
    }

    #[test]
    fn sessions_without_archived_still_scan() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        // Only `sessions/` exists — no `archived_sessions/` dir at all.
        write(
            root,
            "sessions/rollout-a.jsonl",
            &rollout(
                "sess-1",
                "c:\\Dev\\x",
                "m",
                &[token_line(10, 0, 0, "2026-07-10T10:00:00Z")],
            ),
        );
        let records = scan_codex(&CodexScan {
            codex_dir: root,
            run_session_ids: &no_runs(),
            repos: &[],
            since: None,
        });
        assert_eq!(records.len(), 1);
    }
}
