//! The `OpenCode` module of the usage scan (ADR-0033 §2/§6, ADR-0008 D5). Reads the
//! `opencode.db` SQLite `message` store read-only into per-session × model
//! interactive records.
//!
//! Two table sets are read: the 1.x `message` and `session`, and the 2.x
//! `session_message` (`type = 'assistant'`) and `session_v2`. The 2.x upgrade
//! copies each 1.x session into the 2.x tables with the same tokens, so a session
//! id found in the 2.x set is not read again from the 1.x set. A database is 2.x
//! when it has `session_v2`: 1.19.0 creates an empty `session_message`.
//!
//! Two OpenCode-specific rules, both proven live by the adapter
//! (`ralphy-agent-opencode/src/usage.rs`, ADR-0008 D5): (1) `tokens.reasoning`
//! is added to `output`: `OpenCode` 1.19.0 on Windows 11 reported a stream total of
//! 24227 = 22336 input + 86 output + 1792 cache.read + 13 reasoning. (2) The CLI
//! `cost` field is never read (0 for the operator's un-priced custom providers).
//!
//! Aggregation is a plain per-session x model SUM over assistant rows with no
//! row-id dedup: forks land under a distinct `session_id`, and a session never
//! replays its own rows (ADR-0033 §2). Any `rusqlite` error (lock
//! `SQLITE_BUSY`, schema drift, corrupt file) is logged at `warn` and yields
//! an empty vec, never failing the verb; a missing file is silent.

use std::collections::{BTreeMap, HashMap, HashSet};

use crate::{InteractiveRecord, OpenCodeScan, Tokens};

/// Scan the `OpenCode` SQLite store into interactive records (one per session ×
/// model). Fully best-effort: a missing db is silent, and any `rusqlite` error
/// (lock, schema drift, corrupt file) is logged and yields an empty vec.
/// `since` drops records whose `last_ts` is strictly before it (§6: an
/// unparseable bound or record keeps the record).
pub fn scan_opencode(input: &OpenCodeScan<'_>) -> Vec<InteractiveRecord> {
    if !input.db_path.is_file() {
        return Vec::new();
    }
    read_opencode(input).unwrap_or_else(|e| {
        tracing::warn!(
            "could not read the OpenCode token usage from {}: {e}",
            input.db_path.display()
        );
        Vec::new()
    })
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

/// Which table set a row came from.
#[derive(Clone, Copy)]
enum Layout {
    V1,
    V2,
}

/// One message row: `session_id`, its `data` JSON, the session's directory and the
/// table set it came from.
struct RawRow {
    session_id: String,
    data: String,
    directory: Option<String>,
    layout: Layout,
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

    for RawRow {
        session_id,
        data,
        directory,
        layout,
    } in collect_rows(&conn)?
    {
        // Run-owned sessions are Ralphy runs', never interactive (ADR-0033 §5).
        if input.run_session_ids.contains(&session_id) {
            continue;
        }
        let Some(row) = assistant_row(&data, layout) else {
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

fn table_exists(conn: &rusqlite::Connection, name: &str) -> rusqlite::Result<bool> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)",
        [name],
        |row| row.get(0),
    )
}

const V1_ROWS: &str = "SELECT m.session_id, m.data, NULLIF(s.directory,'') \
                       FROM message m LEFT JOIN session s ON s.id = m.session_id";
const V1_ROWS_NO_DIRECTORY: &str = "SELECT session_id, data, NULL FROM message";
const V2_ROWS: &str = "SELECT m.session_id, m.data, NULLIF(s.directory,'') \
                       FROM session_message m LEFT JOIN session_v2 s ON s.id = m.session_id \
                       WHERE m.type = 'assistant'";

/// The rows of both table sets. A 1.x row whose session is also in the 2.x set is
/// dropped: the upgrade copied it. A database with neither set is an error.
fn collect_rows(conn: &rusqlite::Connection) -> rusqlite::Result<Vec<RawRow>> {
    let has_v2 = table_exists(conn, "session_v2")?;
    let mut rows = Vec::new();
    if has_v2 {
        read_rows(conn, V2_ROWS, Layout::V2, &mut rows)?;
    }
    if has_v2 && !table_exists(conn, "message")? {
        return Ok(rows);
    }
    let v2_sessions: HashSet<String> = rows.iter().map(|r| r.session_id.clone()).collect();
    let mut v1 = Vec::new();
    // Older databases have no `session` table (ADR-0033 §6): no directory then.
    let v1_sql = if table_exists(conn, "session")? {
        V1_ROWS
    } else {
        V1_ROWS_NO_DIRECTORY
    };
    read_rows(conn, v1_sql, Layout::V1, &mut v1)?;
    rows.extend(
        v1.into_iter()
            .filter(|r| !v2_sessions.contains(&r.session_id)),
    );
    Ok(rows)
}

fn read_rows(
    conn: &rusqlite::Connection,
    sql: &str,
    layout: Layout,
    out: &mut Vec<RawRow>,
) -> rusqlite::Result<()> {
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_map([], |row| {
        Ok(RawRow {
            session_id: row.get(0)?,
            data: row.get(1)?,
            directory: row.get(2)?,
            layout,
        })
    })?;
    let mut skipped = 0usize;
    for row in rows {
        match row {
            Ok(row) => out.push(row),
            Err(e) if is_undecodable_row(&e) => skipped += 1,
            Err(e) => return Err(e),
        }
    }
    if skipped > 0 {
        tracing::warn!(
            skipped,
            "skipped OpenCode message rows whose columns cannot be read"
        );
    }
    Ok(())
}

/// A row whose values have the wrong type: the row is skipped, the scan goes on.
fn is_undecodable_row(e: &rusqlite::Error) -> bool {
    matches!(
        e,
        rusqlite::Error::InvalidColumnType(..) | rusqlite::Error::FromSqlConversionFailure(..)
    )
}

/// The assistant row in a message's JSON `data`; `None` for any other role,
/// for a row with no `tokens` object, and for text that is not JSON. The 2.x SQL
/// keeps only assistant rows and the model is `model.id`; in 1.x it is `modelID`.
fn assistant_row(data: &str, layout: Layout) -> Option<AssistantRow> {
    let val = serde_json::from_str::<serde_json::Value>(data).ok()?;
    if matches!(layout, Layout::V1) && val.get("role").and_then(|v| v.as_str()) != Some("assistant")
    {
        return None;
    }
    let model = match layout {
        Layout::V1 => val.get("modelID"),
        Layout::V2 => val.get("model").and_then(|m| m.get("id")),
    };
    let tokens = val.get("tokens").filter(|t| t.is_object())?;
    let field = |obj: &serde_json::Value, k: &str| {
        obj.get(k).and_then(serde_json::Value::as_u64).unwrap_or(0)
    };
    let cache = tokens.get("cache");
    Some(AssistantRow {
        model: model
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_owned(),
        // reasoning is counted in `output` (module doc).
        tokens: Tokens {
            input: field(tokens, "input"),
            output: field(tokens, "output") + field(tokens, "reasoning"),
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
