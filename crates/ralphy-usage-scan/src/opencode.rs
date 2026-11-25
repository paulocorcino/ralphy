//! The `OpenCode` module of the usage scan (ADR-0033 §2/§6, ADR-0008 D5). Reads the
//! `opencode.db` SQLite `message` store read-only into per-session × model
//! interactive records.
//!
//! Two OpenCode-specific rules, both proven live by the adapter
//! (`ralphy-agent-opencode/src/usage.rs`, ADR-0008 D5): (1) `tokens.reasoning`
//! is NOT added to `output` — `OpenCode`'s own `total = input + output +
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

/// Scan the `OpenCode` SQLite store into interactive records (one per session ×
/// model). Fully best-effort: any `rusqlite` error (missing db, lock, schema
/// drift, corrupt file) yields an empty vec via the single `read_opencode`
/// error funnel. `since` drops records whose `last_ts` is strictly before it
/// (§6: an unparseable bound or record keeps the record).
pub fn scan_opencode(input: &OpenCodeScan<'_>) -> Vec<InteractiveRecord> {
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

impl ModelAgg {
    fn add(&mut self, row: &AssistantRow) {
        self.tokens.input += row.tokens.input;
        self.tokens.output += row.tokens.output;
        self.tokens.cache_read += row.tokens.cache_read;
        self.tokens.cache_creation += row.tokens.cache_creation;
        if let Some(ms) = row.ts_ms {
            self.first_ms = Some(self.first_ms.map_or(ms, |cur| cur.min(ms)));
            self.last_ms = Some(self.last_ms.map_or(ms, |cur| cur.max(ms)));
        }
    }
}

/// What one assistant `message` row says: its model, its tokens and when it
/// was created (unix ms).
struct AssistantRow {
    model: String,
    tokens: Tokens,
    ts_ms: Option<i64>,
}

/// A session's `(project, actor_email)`.
type Attribution = (Option<String>, Option<String>);

/// The fallible core of [`scan_opencode`], separated so every `rusqlite` error
/// funnels through one `unwrap_or_default`. Opens read-only, joins the
/// `message` rows to their `session.directory`, and aggregates per session ×
/// model. Falls back to a directory-less query when the `session` table / its
/// `directory` column is absent (older DBs, ADR-0033 §6).
fn read_opencode(input: &OpenCodeScan<'_>) -> rusqlite::Result<Vec<InteractiveRecord>> {
    use rusqlite::{Connection, OpenFlags};

    // READ-ONLY: this scan is the reader, OpenCode the writer. WAL rows committed
    // but not yet checkpointed can be invisible to a read-only handle — under-count
    // rather than fail, acceptable for a best-effort measurement path.
    let conn = Connection::open_with_flags(input.db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;

    // slug → resolved git actor email, computed at most once per attributed repo.
    let mut email_cache: HashMap<String, Option<String>> = HashMap::new();
    // (session_id, model) → aggregate.
    let mut groups: BTreeMap<(String, String), ModelAgg> = BTreeMap::new();
    // session_id → its attribution, resolved once.
    let mut attribution: HashMap<String, Attribution> = HashMap::new();

    let mut stmt = prepare_messages(&conn)?;

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
        let Some(row) = assistant_row(&data) else {
            continue;
        };

        // Resolve this session's project/actor once, keyed on session_id.
        attribution
            .entry(session_id.clone())
            .or_insert_with(|| attribute(input, directory.as_deref(), &mut email_cache));

        groups
            .entry((session_id, row.model.clone()))
            .or_default()
            .add(&row);
    }

    let mut records: Vec<InteractiveRecord> = groups
        .into_iter()
        .map(|((session_id, model), agg)| {
            let (project, actor_email) = attribution
                .get(&session_id)
                .cloned()
                .unwrap_or((None, None));
            InteractiveRecord {
                agent: "opencode".to_owned(),
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

    crate::since::retain_since(&mut records, input.since);
    Ok(records)
}

/// The `message` rows as `(session_id, data, directory)`, joined to
/// `session.directory`; without that join when the table or column is absent.
fn prepare_messages(conn: &rusqlite::Connection) -> rusqlite::Result<rusqlite::Statement<'_>> {
    conn.prepare(
        "SELECT m.session_id, m.data, NULLIF(s.directory,'') AS directory \
         FROM message m LEFT JOIN session s ON s.id = m.session_id",
    )
    .or_else(|_| conn.prepare("SELECT session_id, data, NULL AS directory FROM message"))
}

/// The assistant row in a message's JSON `data`; `None` for any other role,
/// for a row with no `tokens` object, and for text that is not JSON.
fn assistant_row(data: &str) -> Option<AssistantRow> {
    let val = serde_json::from_str::<serde_json::Value>(data).ok()?;
    if val.get("role").and_then(|v| v.as_str()) != Some("assistant") {
        return None;
    }
    let tokens = val.get("tokens").filter(|t| t.is_object())?;
    let field = |obj: &serde_json::Value, k: &str| {
        obj.get(k).and_then(serde_json::Value::as_u64).unwrap_or(0)
    };
    let cache = tokens.get("cache");
    Some(AssistantRow {
        model: val
            .get("modelID")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_owned(),
        // reasoning NOT added — `output` already includes it (module doc / ADR-0008 D5).
        tokens: Tokens {
            input: field(tokens, "input"),
            output: field(tokens, "output"),
            cache_read: cache.map_or(0, |c| field(c, "read")),
            cache_creation: cache.map_or(0, |c| field(c, "write")),
        },
        ts_ms: val
            .get("time")
            .and_then(|t| t.get("created"))
            .and_then(serde_json::Value::as_i64),
    })
}

/// The repo a session's directory sits in and that repo's git actor email,
/// looked up once per slug through `email_cache`.
fn attribute(
    input: &OpenCodeScan<'_>,
    directory: Option<&str>,
    email_cache: &mut HashMap<String, Option<String>>,
) -> Attribution {
    let matched = directory.and_then(|d| crate::attribution::find_repo(input.repos, d));
    let project = matched.map(|r| r.slug.clone());
    let actor_email = matched.and_then(|r| {
        email_cache
            .entry(r.slug.clone())
            .or_insert_with(|| ralphy_git_read::user_email(std::path::Path::new(&r.path)))
            .clone()
    });
    (project, actor_email)
}

/// A unix-ms instant → RFC3339 UTC; empty string when absent or out of range.
fn ms_to_rfc3339(ms: Option<i64>) -> String {
    ms.and_then(chrono::DateTime::from_timestamp_millis)
        .map(|d| d.to_rfc3339())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests;
