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

/// The reasoning effort Copilot RECORDED for `session_id` — the chronologically
/// last non-NULL `assistant_usage_events.reasoning_effort` (`ORDER BY id`, the
/// same key the model carry uses).
///
/// This is the post-hoc oracle for the effort clamp (ADR-0041 D5a): what the
/// adapter requested is not the truth, what the vendor wrote is. Fully
/// best-effort like [`session_tokens`] — a missing store, a corrupt file, or a
/// renamed column all yield `None` rather than failing a run.
pub fn session_reasoning_effort(db_path: &Path, session_id: &str) -> Option<String> {
    copy_store(db_path)
        .ok()
        .and_then(|c| read_session_effort(&c.db, session_id).ok())
        .flatten()
}

/// The non-copying reader core of [`session_reasoning_effort`].
fn read_session_effort(db: &Path, session_id: &str) -> rusqlite::Result<Option<String>> {
    use rusqlite::Connection;

    let conn = Connection::open(db)?;
    let mut stmt = conn.prepare(
        "SELECT reasoning_effort FROM assistant_usage_events WHERE session_id = ?1 ORDER BY id",
    )?;
    let rows = stmt.query_map([session_id], |row| row.get::<_, Option<String>>(0))?;
    let mut last = None;
    for level in rows.flatten().flatten() {
        last = Some(level);
    }
    Ok(last)
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
                .and_then(|d| crate::attribution::find_repo(input.repos, d));
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
                tokens: Some(agg.tokens),
                first_ts: agg.first_ts.map(|d| d.to_rfc3339()).unwrap_or_default(),
                last_ts: agg.last_ts.map(|d| d.to_rfc3339()).unwrap_or_default(),
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
mod tests;
