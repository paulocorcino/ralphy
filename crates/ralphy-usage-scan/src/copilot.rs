//! The Copilot module of the usage scan (ADR-0033 §2/§6, ADR-0041 D10). Reads the
//! GitHub Copilot CLI's `session-store.db` SQLite store — its
//! `assistant_usage_events` rows joined to `sessions.cwd` — into per-session ×
//! model interactive records, and (for the adapter) into a single session's
//! summed [`Tokens`].
//!
//! Three Copilot-specific rules, all verified live against
//! `~/.copilot/session-store.db`:
//!
//! 1. **Rows are summed, not keep-last.** `assistant_usage_events` carries one row
//!    per model call with that call's own counts. `turn_index` is NOT a per-call
//!    key (two distinct calls both carried `turn_index: 0`); the key is the
//!    `INTEGER PRIMARY KEY AUTOINCREMENT` `id`, which also fixes the model carry
//!    (`ORDER BY id`, last row wins).
//! 2. **`reasoning_tokens` is never selected.** It has no [`Tokens`] slot and
//!    appears to be a subset of `output_tokens`; folding it in would double-count.
//! 3. **WAL-safe reading: the store is COPIED before it is opened.** The live
//!    database is in WAL mode with `-wal`/`-shm` sidecars on disk, and a read-only
//!    handle cannot replay an uncheckpointed WAL (the under-count trap
//!    `opencode.rs` documents). So the `.db` plus both sidecars are `fs::copy`'d
//!    into a private temp dir and the COPY is opened read-write; the live store is
//!    never opened at all, which makes "never writes the live database" structural
//!    rather than flag-dependent.
//!
//! Not consumed here: `assistant_usage_events.token_details_json`, the per-call
//! rate card Copilot itself records — an array of `{ tokenType, costPerBatch,
//! batchSize, … }` entries whose `costPerBatch` is in **nano-AIU per 1M tokens**,
//! per `tokenType` (`input`/`output`/`cacheRead`/`cacheWrite`). That is the
//! read-time price source ADR-0034 specifies; ADR-0034 is `Status: proposed` and
//! unimplemented, so the column is pinned here and parsed by nobody. Copilot bills
//! in AI credits, not tokens, and no documented nano-AIU→USD rate exists — Ralphy
//! prices these rows in USD at the underlying vendor's list price, the ADR-0034
//! "what would this have cost on metered API" counterfactual.
//!
//! Any `rusqlite` or IO error — missing db, corrupt file, schema drift — funnels
//! through one `unwrap_or_default`, never failing the verb.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::{CopilotScan, InteractiveRecord, Tokens};

/// A private copy of the store (the `.db` plus whatever sidecars existed), owned
/// by its temp directory. Dropping it removes the whole directory — which is why
/// [`copy_store`] returns the guard rather than a bare path: a later `?` cannot
/// leak the copy.
struct StoreCopy {
    dir: PathBuf,
    db: PathBuf,
}

impl Drop for StoreCopy {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Distinguishes concurrent copies within one process (test threads scan in
/// parallel), so two copies never share a temp directory.
static COPY_SEQ: AtomicU64 = AtomicU64::new(0);

/// Copy `db` and its `-wal`/`-shm` sidecars into a fresh temp directory. The `.db`
/// is a hard error (no store, no read); the sidecars are best-effort — a
/// checkpointed store has none. Never opens the live database.
fn copy_store(db: &Path) -> std::io::Result<StoreCopy> {
    let seq = COPY_SEQ.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "ralphy-copilot-store-{}-{}",
        std::process::id(),
        seq
    ));
    // A process killed before `Drop` ran leaves a directory this pid+seq can name
    // again; a stale `-wal` there would be replayed over the fresh `.db` snapshot.
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir)?;
    let name = db
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "session-store.db".to_string());
    // The guard exists BEFORE the first fallible copy, so the `?` below removes
    // the temp dir on its way out instead of leaking it.
    let copy = StoreCopy {
        db: dir.join(&name),
        dir,
    };
    std::fs::copy(db, &copy.db)?;
    for suffix in ["-wal", "-shm"] {
        let side = db.with_file_name(format!("{name}{suffix}"));
        if side.exists() {
            let _ = std::fs::copy(&side, copy.dir.join(format!("{name}{suffix}")));
        }
    }
    Ok(copy)
}

/// The summed tokens of `session_id` in the Copilot store at `db_path`, plus the
/// last row's model. Fully best-effort: any error (missing db, corrupt file,
/// schema drift) yields `(Tokens::default(), None)` — token capture never fails a
/// run. The store is copied first (module doc §3).
pub fn session_tokens(db_path: &Path, session_id: &str) -> (Tokens, Option<String>) {
    copy_store(db_path)
        .ok()
        .and_then(|c| read_session_tokens(&c.db, session_id).ok())
        .unwrap_or_default()
}

/// The non-copying reader core of [`session_tokens`]: sums one session's rows in
/// an already-local database. `ORDER BY id` makes the carried model the
/// chronologically last call's rather than implementation-defined row order.
fn read_session_tokens(db: &Path, session_id: &str) -> rusqlite::Result<(Tokens, Option<String>)> {
    use rusqlite::Connection;

    // The COPY is opened read-write on purpose: a read-only handle cannot replay
    // the `-wal`, so its rows would be invisible (module doc §3).
    let conn = Connection::open(db)?;
    let mut stmt = conn.prepare(
        "SELECT model, input_tokens, output_tokens, cache_read_tokens, cache_write_tokens \
         FROM assistant_usage_events WHERE session_id = ?1 ORDER BY id",
    )?;
    let rows = stmt.query_map([session_id], |row| {
        Ok((
            row.get::<_, Option<String>>(0)?,
            row.get::<_, Option<i64>>(1)?,
            row.get::<_, Option<i64>>(2)?,
            row.get::<_, Option<i64>>(3)?,
            row.get::<_, Option<i64>>(4)?,
        ))
    })?;
    let mut total = Tokens::default();
    let mut model = None;
    for (m, input, output, cache_read, cache_write) in rows.flatten() {
        total.input += input.unwrap_or(0).max(0) as u64;
        total.output += output.unwrap_or(0).max(0) as u64;
        total.cache_read += cache_read.unwrap_or(0).max(0) as u64;
        total.cache_creation += cache_write.unwrap_or(0).max(0) as u64;
        if let Some(m) = m {
            model = Some(m);
        }
    }
    Ok((total, model))
}

/// Scan the Copilot SQLite store into interactive records (one per session ×
/// model). Fully best-effort: any error (missing db, corrupt file, schema drift)
/// yields an empty vec via the single [`read_copilot`] error funnel. `since` drops
/// records whose `last_ts` is strictly before it (§6: an unparseable bound or
/// record keeps the record).
pub fn scan_copilot(input: &CopilotScan) -> Vec<InteractiveRecord> {
    read_copilot(input).unwrap_or_default()
}

/// Per-model accumulator: the summed per-field tokens plus the RFC3339 ts span
/// (`assistant_usage_events.created_at`) of the rows that contributed them.
#[derive(Default)]
struct ModelAgg {
    tokens: Tokens,
    first_ts: Option<chrono::DateTime<chrono::FixedOffset>>,
    last_ts: Option<chrono::DateTime<chrono::FixedOffset>>,
}

/// The fallible core of [`scan_copilot`], separated so every error funnels through
/// one `unwrap_or_default`. Reads the private copy, joins the usage rows to their
/// `sessions.cwd`, and aggregates per session × model. Falls back to a cwd-less
/// query when the `sessions` table / its `cwd` column is absent (ADR-0033 §6).
fn read_copilot(input: &CopilotScan) -> rusqlite::Result<Vec<InteractiveRecord>> {
    use rusqlite::Connection;

    let copy = copy_store(input.db_path)
        .map_err(|e| rusqlite::Error::InvalidPath(PathBuf::from(e.to_string())))?;
    let conn = Connection::open(&copy.db)?;

    // slug → resolved git actor email, computed at most once per attributed repo.
    let mut email_cache: HashMap<String, Option<String>> = HashMap::new();
    // (session_id, model) → aggregate.
    let mut groups: BTreeMap<(String, String), ModelAgg> = BTreeMap::new();
    // session_id → its (project, actor_email) attribution, resolved once.
    let mut attribution: HashMap<String, (Option<String>, Option<String>)> = HashMap::new();

    let mut stmt = conn
        .prepare(
            "SELECT u.session_id, u.model, u.input_tokens, u.output_tokens, \
             u.cache_read_tokens, u.cache_write_tokens, u.created_at, \
             NULLIF(s.cwd,'') AS cwd \
             FROM assistant_usage_events u LEFT JOIN sessions s ON s.id = u.session_id",
        )
        .or_else(|_| {
            conn.prepare(
                "SELECT session_id, model, input_tokens, output_tokens, cache_read_tokens, \
                 cache_write_tokens, created_at, NULL AS cwd FROM assistant_usage_events",
            )
        })?;

    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, Option<String>>(1)?,
            row.get::<_, Option<i64>>(2)?,
            row.get::<_, Option<i64>>(3)?,
            row.get::<_, Option<i64>>(4)?,
            row.get::<_, Option<i64>>(5)?,
            row.get::<_, Option<String>>(6)?,
            row.get::<_, Option<String>>(7)?,
        ))
    })?;

    for (
        session_id,
        model,
        input_tokens,
        output_tokens,
        cache_read,
        cache_write,
        created_at,
        cwd,
    ) in rows.flatten()
    {
        // Run-owned sessions are Ralphy runs', never interactive (ADR-0033 §5).
        if input.run_session_ids.contains(&session_id) {
            continue;
        }
        let model = model.unwrap_or_else(|| "unknown".to_string());

        attribution.entry(session_id.clone()).or_insert_with(|| {
            let matched = cwd
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
        agg.tokens.input += input_tokens.unwrap_or(0).max(0) as u64;
        agg.tokens.output += output_tokens.unwrap_or(0).max(0) as u64;
        agg.tokens.cache_read += cache_read.unwrap_or(0).max(0) as u64;
        agg.tokens.cache_creation += cache_write.unwrap_or(0).max(0) as u64;
        // `created_at` is TEXT, RFC3339-with-`Z` (verified live).
        if let Some(ts) = created_at
            .as_deref()
            .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
        {
            agg.first_ts = Some(agg.first_ts.map_or(ts, |cur| cur.min(ts)));
            agg.last_ts = Some(agg.last_ts.map_or(ts, |cur| cur.max(ts)));
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
                agent: "copilot".to_string(),
                model,
                session_id,
                project,
                actor_email,
                tokens: agg.tokens,
                first_ts: agg.first_ts.map(|d| d.to_rfc3339()).unwrap_or_default(),
                last_ts: agg.last_ts.map(|d| d.to_rfc3339()).unwrap_or_default(),
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

/// Normalize a filesystem path for a case-insensitive compare: `\` → `/`, trailing
/// `/` trimmed. Duplicated from `opencode.rs` (ADR-0033 §7 accepts per-vendor
/// duplication).
fn normalize_path(p: &str) -> String {
    p.replace('\\', "/").trim_end_matches('/').to_string()
}

/// True when two paths name the same directory. Duplicated from `opencode.rs`.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::RegisteredRepo;
    use rusqlite::Connection;
    use std::collections::HashSet;
    use std::fs;

    const CREATE_USAGE: &str = "CREATE TABLE assistant_usage_events (\
         id INTEGER PRIMARY KEY AUTOINCREMENT, session_id TEXT, turn_index INTEGER, \
         model TEXT, input_tokens INTEGER, output_tokens INTEGER, \
         cache_read_tokens INTEGER, cache_write_tokens INTEGER, \
         reasoning_tokens INTEGER, token_details_json TEXT, created_at TEXT)";
    const CREATE_SESSIONS: &str = "CREATE TABLE sessions (id TEXT PRIMARY KEY, cwd TEXT)";

    /// A usage row as the live store shapes it.
    struct Row<'a> {
        session_id: &'a str,
        turn_index: i64,
        model: &'a str,
        input: i64,
        output: i64,
        cache_read: i64,
        cache_write: i64,
        reasoning: i64,
        created_at: &'a str,
    }

    fn insert(conn: &Connection, r: &Row) {
        conn.execute(
            "INSERT INTO assistant_usage_events (session_id, turn_index, model, input_tokens, \
             output_tokens, cache_read_tokens, cache_write_tokens, reasoning_tokens, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            rusqlite::params![
                r.session_id,
                r.turn_index,
                r.model,
                r.input,
                r.output,
                r.cache_read,
                r.cache_write,
                r.reasoning,
                r.created_at
            ],
        )
        .unwrap();
    }

    /// The live P2 pair: two distinct calls of one session, both `turn_index 0`.
    fn seed_p2(dir: &Path, session_id: &str) -> PathBuf {
        let path = dir.join("session-store.db");
        let conn = Connection::open(&path).unwrap();
        conn.execute(CREATE_USAGE, []).unwrap();
        conn.execute(CREATE_SESSIONS, []).unwrap();
        insert(
            &conn,
            &Row {
                session_id,
                turn_index: 0,
                model: "claude-sonnet-5",
                input: 22913,
                output: 350,
                cache_read: 0,
                cache_write: 22903,
                reasoning: 159,
                created_at: "2026-07-20T11:54:33.066Z",
            },
        );
        insert(
            &conn,
            &Row {
                session_id,
                turn_index: 0,
                model: "claude-sonnet-5",
                input: 23345,
                output: 23,
                cache_read: 22903,
                cache_write: 437,
                reasoning: 0,
                created_at: "2026-07-20T11:55:14.161Z",
            },
        );
        path
    }

    #[test]
    fn copilot_sums_rows_never_keeps_last() {
        let tmp = tempfile::tempdir().unwrap();
        let db = seed_p2(tmp.path(), "ses_p2");
        let (tokens, model) = session_tokens(&db, "ses_p2");
        assert_eq!(
            // 22913 + 23345 — the plan's "36258" was an arithmetic slip.
            tokens.input,
            46258,
            "summed, not keep-last (would be 23345)"
        );
        assert_eq!(tokens.output, 373, "reasoning NOT folded (would be 532)");
        assert_eq!(tokens.cache_read, 22903);
        assert_eq!(tokens.cache_creation, 23340);
        assert_eq!(model.as_deref(), Some("claude-sonnet-5"));
    }

    /// `ORDER BY id`, LAST row wins — not first, and not `turn_index` order. The
    /// two rows carry different models in an order where id and alphabet disagree.
    #[test]
    fn copilot_model_carry_is_the_highest_id_row() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("session-store.db");
        let conn = Connection::open(&path).unwrap();
        conn.execute(CREATE_USAGE, []).unwrap();
        for (model, turn) in [("claude-sonnet-5", 1), ("a-later-model", 0)] {
            insert(
                &conn,
                &Row {
                    session_id: "ses_m",
                    turn_index: turn,
                    model,
                    input: 1,
                    output: 1,
                    cache_read: 0,
                    cache_write: 0,
                    reasoning: 0,
                    created_at: "2026-07-20T11:54:33.066Z",
                },
            );
        }
        drop(conn);
        let (_, model) = session_tokens(&path, "ses_m");
        assert_eq!(
            model.as_deref(),
            Some("a-later-model"),
            "the highest `id` wins — keep-first or `ORDER BY turn_index` would give claude-sonnet-5"
        );
    }

    /// The cwd-less fallback query (ADR-0033 §6): a store with no `sessions` table
    /// still reports its rows, unattributed, instead of degrading to zero.
    #[test]
    fn copilot_store_without_a_sessions_table_still_reports_rows() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("session-store.db");
        let conn = Connection::open(&path).unwrap();
        conn.execute(CREATE_USAGE, []).unwrap();
        insert(
            &conn,
            &Row {
                session_id: "ses_nofk",
                turn_index: 0,
                model: "claude-sonnet-5",
                input: 10,
                output: 5,
                cache_read: 0,
                cache_write: 0,
                reasoning: 0,
                created_at: "2026-07-20T11:54:33.066Z",
            },
        );
        drop(conn);
        let records = scan_copilot(&CopilotScan {
            db_path: &path,
            run_session_ids: &HashSet::new(),
            repos: &[RegisteredRepo {
                slug: "o/ralphy".into(),
                path: "C:\\Dev\\ralphy".into(),
            }],
            since: None,
        });
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].tokens.input, 10);
        assert_eq!(records[0].project, None, "no cwd column, no attribution");
        assert_eq!(records[0].actor_email, None);
    }

    /// A cwd that matches no registered repo is REPORTED, never dropped (§6), and
    /// a matched one carries the repo's git actor email.
    #[test]
    fn copilot_attribution_covers_matched_and_unmatched_cwd() {
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

        let path = tmp.path().join("session-store.db");
        let conn = Connection::open(&path).unwrap();
        conn.execute(CREATE_USAGE, []).unwrap();
        conn.execute(CREATE_SESSIONS, []).unwrap();
        for sid in ["ses_in", "ses_out"] {
            insert(
                &conn,
                &Row {
                    session_id: sid,
                    turn_index: 0,
                    model: "claude-sonnet-5",
                    input: 10,
                    output: 5,
                    cache_read: 0,
                    cache_write: 0,
                    reasoning: 0,
                    created_at: "2026-07-20T11:54:33.066Z",
                },
            );
        }
        conn.execute(
            "INSERT INTO sessions (id, cwd) VALUES ('ses_in', ?1), ('ses_out', ?2)",
            rusqlite::params![repo_path, tmp.path().join("elsewhere").to_string_lossy()],
        )
        .unwrap();
        drop(conn);

        let records = scan_copilot(&CopilotScan {
            db_path: &path,
            run_session_ids: &HashSet::new(),
            repos: &[RegisteredRepo {
                slug: "o/repo".into(),
                path: repo_path,
            }],
            since: None,
        });
        assert_eq!(
            records.len(),
            2,
            "an unmatched cwd is reported, not dropped"
        );
        let matched = records.iter().find(|r| r.session_id == "ses_in").unwrap();
        assert_eq!(matched.project.as_deref(), Some("o/repo"));
        assert_eq!(matched.actor_email.as_deref(), Some("t@example.com"));
        let unmatched = records.iter().find(|r| r.session_id == "ses_out").unwrap();
        assert_eq!(unmatched.project, None);
        assert_eq!(unmatched.actor_email, None);
    }

    #[test]
    fn copilot_wal_rows_need_the_sidecars() {
        let tmp = tempfile::tempdir().unwrap();
        let live = tmp.path().join("live");
        fs::create_dir_all(&live).unwrap();
        let db = live.join("session-store.db");
        // The writer connection stays alive for the whole test: dropping it
        // checkpoints the WAL into the `.db` and destroys the evidence.
        let conn = Connection::open(&db).unwrap();
        // The table is created BEFORE WAL is switched on, so the `.db`-only copy
        // has the schema and misses only the row — isolating the WAL invisibility
        // from a trivial "no such table".
        conn.execute(CREATE_USAGE, []).unwrap();
        conn.pragma_update(None, "journal_mode", "WAL").unwrap();
        insert(
            &conn,
            &Row {
                session_id: "ses_wal",
                turn_index: 0,
                model: "claude-sonnet-5",
                input: 100,
                output: 0,
                cache_read: 0,
                cache_write: 0,
                reasoning: 0,
                created_at: "2026-07-20T11:54:33.066Z",
            },
        );

        let db_only = tmp.path().join("a");
        fs::create_dir_all(&db_only).unwrap();
        fs::copy(&db, db_only.join("session-store.db")).unwrap();

        let all_three = tmp.path().join("b");
        fs::create_dir_all(&all_three).unwrap();
        for suffix in ["", "-wal", "-shm"] {
            let src = live.join(format!("session-store.db{suffix}"));
            fs::copy(&src, all_three.join(format!("session-store.db{suffix}"))).unwrap();
        }

        let (a, _) = read_session_tokens(&db_only.join("session-store.db"), "ses_wal").unwrap();
        assert_eq!(a.input, 0, "the `.db` alone cannot see uncheckpointed rows");
        let (b, _) = read_session_tokens(&all_three.join("session-store.db"), "ses_wal").unwrap();
        assert_eq!(b.input, 100, "the `.db` + sidecars replays the WAL");

        // The PRODUCTION path over the same live store, writer still open: this is
        // the leg that reds if `copy_store`'s sidecar loop is deleted — the two
        // hand-copied legs above only establish the SQLite premise.
        let (live_tokens, _) = session_tokens(&db, "ses_wal");
        assert_eq!(
            live_tokens.input, 100,
            "session_tokens must copy the sidecars, not just the `.db`"
        );
        let records = scan_copilot(&CopilotScan {
            db_path: &db,
            run_session_ids: &HashSet::new(),
            repos: &[],
            since: None,
        });
        assert_eq!(records.len(), 1, "scan_copilot sees the uncheckpointed row");
        assert_eq!(records[0].tokens.input, 100);
    }

    /// The store under test is a LIVE WAL store with its writer still open — the
    /// shape the daemon actually meets. A reader that opened it in place would
    /// checkpoint or truncate the `-wal` (or leave a journal behind); asserting the
    /// `.db` AND `-wal` bytes plus the directory listing is what makes "never
    /// writes the live database" a falsifiable claim rather than a property any
    /// pure-SELECT implementation satisfies on a quiescent DELETE-mode file.
    #[test]
    fn copilot_never_writes_the_live_store() {
        let tmp = tempfile::tempdir().unwrap();
        let live = tmp.path().join("live");
        fs::create_dir_all(&live).unwrap();
        let db = live.join("session-store.db");
        let conn = Connection::open(&db).unwrap();
        conn.execute(CREATE_USAGE, []).unwrap();
        conn.execute(CREATE_SESSIONS, []).unwrap();
        conn.pragma_update(None, "journal_mode", "WAL").unwrap();
        insert(
            &conn,
            &Row {
                session_id: "ses_p2",
                turn_index: 0,
                model: "claude-sonnet-5",
                input: 22913,
                output: 350,
                cache_read: 0,
                cache_write: 22903,
                reasoning: 159,
                created_at: "2026-07-20T11:54:33.066Z",
            },
        );

        let names = |dir: &Path| {
            let mut n: Vec<String> = fs::read_dir(dir)
                .unwrap()
                .flatten()
                .map(|e| e.file_name().to_string_lossy().to_string())
                .collect();
            n.sort();
            n
        };
        let wal = live.join("session-store.db-wal");
        let before_db = fs::read(&db).unwrap();
        let before_wal = fs::read(&wal).unwrap();
        let before_names = names(&live);
        assert!(
            before_names.iter().any(|n| n.ends_with("-wal")),
            "the fixture must be a live WAL store, got {before_names:?}"
        );

        let (tokens, _) = session_tokens(&db, "ses_p2");
        assert_eq!(tokens.input, 22913, "the scan actually read the live row");
        let records = scan_copilot(&CopilotScan {
            db_path: &db,
            run_session_ids: &HashSet::new(),
            repos: &[],
            since: None,
        });
        assert_eq!(records.len(), 1);

        assert_eq!(
            fs::read(&db).unwrap(),
            before_db,
            "live `.db` bytes unchanged"
        );
        assert_eq!(
            fs::read(&wal).unwrap(),
            before_wal,
            "the `-wal` was neither checkpointed nor truncated"
        );
        assert_eq!(names(&live), before_names, "no file added or removed");
    }

    #[test]
    fn copilot_excludes_run_owned_sessions() {
        let tmp = tempfile::tempdir().unwrap();
        let db = seed_p2(tmp.path(), "ses_run");
        let conn = Connection::open(&db).unwrap();
        insert(
            &conn,
            &Row {
                session_id: "ses_int",
                turn_index: 0,
                model: "claude-sonnet-5",
                input: 10,
                output: 5,
                cache_read: 0,
                cache_write: 0,
                reasoning: 0,
                created_at: "2026-07-20T11:54:33.066Z",
            },
        );
        drop(conn);
        let mut runs = HashSet::new();
        runs.insert("ses_run".to_string());
        let records = scan_copilot(&CopilotScan {
            db_path: &db,
            run_session_ids: &runs,
            repos: &[],
            since: None,
        });
        assert!(records.iter().any(|r| r.session_id == "ses_int"));
        assert!(!records.iter().any(|r| r.session_id == "ses_run"));
        assert_eq!(records[0].agent, "copilot");
    }

    #[test]
    fn copilot_attributes_cwd_to_registered_repo() {
        let tmp = tempfile::tempdir().unwrap();
        let db = seed_p2(tmp.path(), "ses_p2");
        let conn = Connection::open(&db).unwrap();
        conn.execute(
            "INSERT INTO sessions (id, cwd) VALUES (?1, ?2)",
            rusqlite::params!["ses_p2", "C:\\Dev\\ralphy"],
        )
        .unwrap();
        drop(conn);
        let repos = vec![RegisteredRepo {
            slug: "o/ralphy".into(),
            path: "C:\\Dev\\ralphy".into(),
        }];
        let records = scan_copilot(&CopilotScan {
            db_path: &db,
            run_session_ids: &HashSet::new(),
            repos: &repos,
            since: None,
        });
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].project.as_deref(), Some("o/ralphy"));
        assert_eq!(records[0].tokens.input, 46258);
    }

    #[test]
    fn copilot_since_filters_by_last_ts() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("session-store.db");
        let conn = Connection::open(&path).unwrap();
        conn.execute(CREATE_USAGE, []).unwrap();
        conn.execute(CREATE_SESSIONS, []).unwrap();
        for (sid, created_at) in [
            ("ses_old", "2026-07-20T11:54:33.066Z"),
            ("ses_new", "2026-07-20T11:55:14.161Z"),
        ] {
            insert(
                &conn,
                &Row {
                    session_id: sid,
                    turn_index: 0,
                    model: "claude-sonnet-5",
                    input: 10,
                    output: 5,
                    cache_read: 0,
                    cache_write: 0,
                    reasoning: 0,
                    created_at,
                },
            );
        }
        drop(conn);
        let records = scan_copilot(&CopilotScan {
            db_path: &path,
            run_session_ids: &HashSet::new(),
            repos: &[],
            since: Some("2026-07-20T11:55:00Z"),
        });
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].session_id, "ses_new");
    }

    #[test]
    fn copilot_missing_db_is_zero() {
        let records = scan_copilot(&CopilotScan {
            db_path: Path::new("does-not-exist-anywhere-session-store.db"),
            run_session_ids: &HashSet::new(),
            repos: &[],
            since: None,
        });
        assert!(records.is_empty());
        assert_eq!(
            session_tokens(
                Path::new("does-not-exist-anywhere-session-store.db"),
                "ses_x"
            ),
            (Tokens::default(), None)
        );
    }

    #[test]
    fn copilot_corrupt_db_degrades_to_zero() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("session-store.db");
        fs::write(&path, b"this is not a sqlite database at all").unwrap();
        let records = scan_copilot(&CopilotScan {
            db_path: &path,
            run_session_ids: &HashSet::new(),
            repos: &[],
            since: None,
        });
        assert!(records.is_empty());
        assert_eq!(session_tokens(&path, "ses_x"), (Tokens::default(), None));
    }
}
