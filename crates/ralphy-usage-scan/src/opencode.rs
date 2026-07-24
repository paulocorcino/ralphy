//! The OpenCode module of the usage scan (ADR-0033 §2/§6, ADR-0008 D5). Reads the
//! `opencode.db` SQLite `message` store read-only into per-session × model
//! interactive records.
//!
//! Two OpenCode-specific rules, both proven live by the adapter
//! (`ralphy-agent-opencode/src/usage.rs`, ADR-0008 D5): (1) `tokens.reasoning`
//! is NOT added to `output` — OpenCode's own `total = input + output +
//! cache.read` reconciles without it, so reasoning already sits inside `output`;
//! re-adding it double-counts. (2) The CLI `cost` field is never read (0 for the
//! operator's un-priced custom providers).
//!
//! Aggregation is a plain per-session × model SUM over assistant rows with no
//! row-id dedup: forks land under a distinct `session_id`, and a session never
//! replays its own rows (ADR-0033 §2). Any `rusqlite` error — missing db, lock
//! (`SQLITE_BUSY`), schema drift, corrupt file — funnels through one
//! `unwrap_or_default` into an empty vec, never failing the verb.

use std::collections::{BTreeMap, HashMap};

use crate::{InteractiveRecord, OpenCodeScan, Tokens};

/// Scan the OpenCode SQLite store into interactive records (one per session ×
/// model). Fully best-effort: any `rusqlite` error (missing db, lock, schema
/// drift, corrupt file) yields an empty vec via the single [`read_opencode`]
/// error funnel. `since` drops records whose `last_ts` is strictly before it
/// (§6: an unparseable bound or record keeps the record).
pub fn scan_opencode(input: &OpenCodeScan) -> Vec<InteractiveRecord> {
    read_opencode(input).unwrap_or_default()
}

/// Per-model accumulator: the summed per-field tokens plus the ts span
/// (`data.time.created`, unix ms) of the rows that contributed them.
#[derive(Default)]
struct ModelAgg {
    tokens: Tokens,
    first_ms: Option<i64>,
    last_ms: Option<i64>,
}

/// The fallible core of [`scan_opencode`], separated so every `rusqlite` error
/// funnels through one `unwrap_or_default`. Opens read-only, joins the
/// `message` rows to their `session.directory`, and aggregates per session ×
/// model. Falls back to a directory-less query when the `session` table / its
/// `directory` column is absent (older DBs, ADR-0033 §6).
fn read_opencode(input: &OpenCodeScan) -> rusqlite::Result<Vec<InteractiveRecord>> {
    use rusqlite::{Connection, OpenFlags};

    // READ-ONLY: this scan is the reader, OpenCode the writer. WAL rows committed
    // but not yet checkpointed can be invisible to a read-only handle — under-count
    // rather than fail, acceptable for a best-effort measurement path.
    let conn = Connection::open_with_flags(input.db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;

    // slug → resolved git actor email, computed at most once per attributed repo.
    let mut email_cache: HashMap<String, Option<String>> = HashMap::new();
    // (session_id, model) → aggregate.
    let mut groups: BTreeMap<(String, String), ModelAgg> = BTreeMap::new();
    // session_id → its (project, actor_email) attribution, resolved once.
    let mut attribution: HashMap<String, (Option<String>, Option<String>)> = HashMap::new();

    let mut stmt = conn
        .prepare(
            "SELECT m.session_id, m.data, NULLIF(s.directory,'') AS directory \
             FROM message m LEFT JOIN session s ON s.id = m.session_id",
        )
        .or_else(|_| conn.prepare("SELECT session_id, data, NULL AS directory FROM message"))?;

    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, Option<String>>(2)?,
        ))
    })?;

    for (session_id, data, directory) in rows.flatten() {
        // Run-owned sessions are Ralphy runs', never interactive (ADR-0033 §5).
        if input.run_session_ids.contains(&session_id) {
            continue;
        }
        let Ok(val) = serde_json::from_str::<serde_json::Value>(&data) else {
            continue;
        };
        if val.get("role").and_then(|v| v.as_str()) != Some("assistant") {
            continue;
        }
        let Some(tokens) = val.get("tokens").filter(|t| t.is_object()) else {
            continue;
        };
        let field =
            |obj: &serde_json::Value, k: &str| obj.get(k).and_then(|v| v.as_u64()).unwrap_or(0);
        let cache = tokens.get("cache");
        // reasoning NOT added — `output` already includes it (module doc / ADR-0008 D5).
        let row_tokens = Tokens {
            input: field(tokens, "input"),
            output: field(tokens, "output"),
            cache_read: cache.map(|c| field(c, "read")).unwrap_or(0),
            cache_creation: cache.map(|c| field(c, "write")).unwrap_or(0),
        };
        let model = val
            .get("modelID")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();
        let ts_ms = val
            .get("time")
            .and_then(|t| t.get("created"))
            .and_then(|v| v.as_i64());

        // Resolve this session's project/actor once, keyed on session_id.
        attribution.entry(session_id.clone()).or_insert_with(|| {
            let matched = directory
                .as_deref()
                .and_then(|d| input.repos.iter().find(|r| paths_eq(&r.path, d)));
            let project = matched.map(|r| r.slug.clone());
            let actor_email = matched.and_then(|r| {
                email_cache
                    .entry(r.slug.clone())
                    .or_insert_with(|| repo_actor_email(&r.path))
                    .clone()
            });
            (project, actor_email)
        });

        let agg = groups.entry((session_id.clone(), model)).or_default();
        agg.tokens.input += row_tokens.input;
        agg.tokens.output += row_tokens.output;
        agg.tokens.cache_read += row_tokens.cache_read;
        agg.tokens.cache_creation += row_tokens.cache_creation;
        if let Some(ms) = ts_ms {
            agg.first_ms = Some(agg.first_ms.map_or(ms, |cur| cur.min(ms)));
            agg.last_ms = Some(agg.last_ms.map_or(ms, |cur| cur.max(ms)));
        }
    }

    let mut records: Vec<InteractiveRecord> = groups
        .into_iter()
        .map(|((session_id, model), agg)| {
            let (project, actor_email) = attribution
                .get(&session_id)
                .cloned()
                .unwrap_or((None, None));
            InteractiveRecord {
                agent: "opencode".to_string(),
                model,
                session_id,
                project,
                actor_email,
                tokens: Some(agg.tokens),
                first_ts: ms_to_rfc3339(agg.first_ms),
                last_ts: ms_to_rfc3339(agg.last_ms),
                lower_bound: false,
            }
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
    Ok(records)
}

/// A unix-ms instant → RFC3339 UTC; empty string when absent or out of range.
fn ms_to_rfc3339(ms: Option<i64>) -> String {
    ms.and_then(chrono::DateTime::from_timestamp_millis)
        .map(|d| d.to_rfc3339())
        .unwrap_or_default()
}

/// Normalize a filesystem path for a case-insensitive compare: `\` → `/`, trailing
/// `/` trimmed. Duplicated from `codex.rs` (ADR-0033 §7 accepts per-vendor
/// duplication).
fn normalize_path(p: &str) -> String {
    p.replace('\\', "/").trim_end_matches('/').to_string()
}

/// True when two paths name the same directory: normalized and compared with
/// `eq_ignore_ascii_case`. Duplicated from `codex.rs`.
fn paths_eq(a: &str, b: &str) -> bool {
    normalize_path(a).eq_ignore_ascii_case(&normalize_path(b))
}

/// `git config user.email` for the attributed repo (ADR-0008 D7). `None` on a
/// non-zero exit or empty output. Duplicated from `codex.rs` (ADR-0033 §7).
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
    use crate::RegisteredRepo;
    use std::collections::HashSet;
    use std::fs;
    use std::path::{Path, PathBuf};

    fn no_runs() -> HashSet<String> {
        HashSet::new()
    }

    /// A row: `(session_id, data-json)`. `session` entries: `(session_id, directory)`.
    fn seed_db(dir: &Path, rows: &[(&str, &str)], sessions: &[(&str, &str)]) -> PathBuf {
        use rusqlite::Connection;
        let path = dir.join("opencode.db");
        let conn = Connection::open(&path).unwrap();
        conn.execute(
            "CREATE TABLE message (id TEXT, session_id TEXT, data TEXT)",
            [],
        )
        .unwrap();
        conn.execute("CREATE TABLE session (id TEXT, directory TEXT)", [])
            .unwrap();
        for (i, (sid, data)) in rows.iter().enumerate() {
            conn.execute(
                "INSERT INTO message (id, session_id, data) VALUES (?1, ?2, ?3)",
                rusqlite::params![format!("msg_{i}"), sid, data],
            )
            .unwrap();
        }
        for (sid, directory) in sessions {
            conn.execute(
                "INSERT INTO session (id, directory) VALUES (?1, ?2)",
                rusqlite::params![sid, directory],
            )
            .unwrap();
        }
        path
    }

    fn scan(
        db_path: &Path,
        repos: &[RegisteredRepo],
        since: Option<&str>,
    ) -> Vec<InteractiveRecord> {
        scan_opencode(&OpenCodeScan {
            db_path,
            run_session_ids: &no_runs(),
            repos,
            since,
        })
    }

    #[test]
    fn opencode_maps_tokens_ignoring_cost_and_reasoning() {
        let tmp = tempfile::tempdir().unwrap();
        let data = r#"{"role":"assistant","cost":999.0,"modelID":"k2p6","tokens":{"input":2168,"output":100,"reasoning":40,"cache":{"write":0,"read":11264}}}"#;
        let db = seed_db(tmp.path(), &[("ses_1", data)], &[]);
        let records = scan(&db, &[], None);
        assert_eq!(records.len(), 1);
        let r = &records[0];
        assert_eq!(r.tokens.as_ref().unwrap().input, 2168);
        assert_eq!(
            r.tokens.as_ref().unwrap().output,
            100,
            "reasoning NOT folded (would be 140)"
        );
        assert_eq!(r.tokens.as_ref().unwrap().cache_read, 11264);
        assert_eq!(r.tokens.as_ref().unwrap().cache_creation, 0);
        assert_eq!(r.model, "k2p6");
        assert_eq!(r.agent, "opencode");
        assert!(
            !r.lower_bound,
            "OpenCode writes every token to disk — this is a total, not a floor"
        );
    }

    #[test]
    fn opencode_attributes_directory_to_registered_repo() {
        let tmp = tempfile::tempdir().unwrap();
        let data = r#"{"role":"assistant","modelID":"k2p6","tokens":{"input":10,"output":5}}"#;
        let db = seed_db(
            tmp.path(),
            &[("ses_1", data)],
            &[("ses_1", "C:\\Dev\\ralphy")],
        );
        let repos = vec![RegisteredRepo {
            slug: "o/ralphy".into(),
            path: "C:\\Dev\\ralphy".into(),
        }];
        let records = scan(&db, &repos, None);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].project.as_deref(), Some("o/ralphy"));
    }

    #[test]
    fn opencode_unmatched_directory_yields_null_project() {
        let tmp = tempfile::tempdir().unwrap();
        let data = r#"{"role":"assistant","modelID":"k2p6","tokens":{"input":10,"output":5}}"#;
        let db = seed_db(
            tmp.path(),
            &[("ses_1", data)],
            &[("ses_1", "C:\\Dev\\elsewhere")],
        );
        let repos = vec![RegisteredRepo {
            slug: "o/ralphy".into(),
            path: "C:\\Dev\\ralphy".into(),
        }];
        let records = scan(&db, &repos, None);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].project, None);
        assert_eq!(records[0].actor_email, None);
    }

    #[test]
    fn opencode_excludes_run_owned_sessions() {
        let tmp = tempfile::tempdir().unwrap();
        let data = r#"{"role":"assistant","modelID":"m","tokens":{"input":10,"output":5}}"#;
        let db = seed_db(tmp.path(), &[("ses_run", data), ("ses_int", data)], &[]);
        let mut runs = HashSet::new();
        runs.insert("ses_run".to_string());
        let records = scan_opencode(&OpenCodeScan {
            db_path: &db,
            run_session_ids: &runs,
            repos: &[],
            since: None,
        });
        assert!(records.iter().any(|r| r.session_id == "ses_int"));
        assert!(!records.iter().any(|r| r.session_id == "ses_run"));
    }

    #[test]
    fn opencode_missing_db_is_zero() {
        let records = scan(Path::new("does-not-exist-anywhere.db"), &[], None);
        assert!(records.is_empty());
    }

    #[test]
    fn opencode_corrupt_db_degrades_to_zero() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("opencode.db");
        fs::write(&path, b"this is not a sqlite database at all").unwrap();
        let records = scan(&path, &[], None);
        assert!(records.is_empty());
    }

    #[test]
    fn opencode_since_filters_by_last_ts() {
        let tmp = tempfile::tempdir().unwrap();
        // created ms: old = 2026-07-01, new = 2026-07-10.
        let old = r#"{"role":"assistant","modelID":"m","tokens":{"input":10,"output":5},"time":{"created":1782000000000}}"#;
        let new = r#"{"role":"assistant","modelID":"m","tokens":{"input":20,"output":5},"time":{"created":1783000000000}}"#;
        let db = seed_db(tmp.path(), &[("ses_old", old), ("ses_new", new)], &[]);
        // since between the two created instants.
        let between = ms_to_rfc3339(Some(1782500000000));
        let records = scan(&db, &[], Some(&between));
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].session_id, "ses_new");
    }

    #[test]
    fn opencode_attributed_record_carries_git_actor_email() {
        let tmp = tempfile::tempdir().unwrap();
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

        let data = r#"{"role":"assistant","modelID":"m","tokens":{"input":10,"output":5}}"#;
        let db = seed_db(tmp.path(), &[("ses_1", data)], &[("ses_1", &repo_path)]);
        let repos = vec![RegisteredRepo {
            slug: "o/repo".into(),
            path: repo_path,
        }];
        let records = scan(&db, &repos, None);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].project.as_deref(), Some("o/repo"));
        assert_eq!(records[0].actor_email.as_deref(), Some("t@example.com"));
    }
}
