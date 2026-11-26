//! `OpenCode` token-usage capture, correlating a phase's `--format json` stream
//! `sessionID` to the rows `OpenCode` wrote into its SQLite message store
//! (ADR-0008 D5).

use std::path::Path;

use ralphy_core::Usage;

/// The token fields of an `OpenCode` `tokens` object as a [`Usage`] with `model`:
/// `input = tokens.input`, `output = tokens.output + tokens.reasoning`,
/// `cache_read = tokens.cache.read`, `cache_creation = tokens.cache.write`. The CLI
/// `cost` field is never read: it is `0` for un-priced custom providers.
// Measured with OpenCode 1.19.0 on Windows 11: the stream `total` 24227 = 22336
// input + 86 output + 1792 cache.read + 13 reasoning, so reasoning is counted
// apart from `output` (#1040). An older reading, that `output` holds it, was wrong.
fn usage_from_tokens(tokens: &serde_json::Value, model: Option<&str>) -> Usage {
    let field = |obj: &serde_json::Value, k: &str| {
        obj.get(k).and_then(serde_json::Value::as_u64).unwrap_or(0)
    };
    let cache = tokens.get("cache");
    Usage {
        input: field(tokens, "input"),
        output: field(tokens, "output") + field(tokens, "reasoning"),
        cache_read: cache.map_or(0, |c| field(c, "read")),
        cache_creation: cache.map_or(0, |c| field(c, "write")),
        model: model.map(str::to_owned),
    }
}

/// Map an 1.x `message` row's `data` JSON to a [`Usage`]. Requires `role ==
/// "assistant"` with a `tokens` object; the model is `data.modelID`.
fn usage_from_opencode_message(data: &serde_json::Value) -> Option<Usage> {
    if data.get("role").and_then(|v| v.as_str()) != Some("assistant") {
        return None;
    }
    let tokens = data.get("tokens").filter(|t| t.is_object())?;
    let model = data.get("modelID").and_then(|v| v.as_str());
    Some(usage_from_tokens(tokens, model))
}

/// Map a 2.x `session_message` row's `data` JSON (the SQL keeps `type =
/// 'assistant'`) to a [`Usage`]; the model is `data.model.id`.
fn usage_from_v2_message(data: &serde_json::Value) -> Option<Usage> {
    let tokens = data.get("tokens").filter(|t| t.is_object())?;
    let model = data
        .get("model")
        .and_then(|m| m.get("id"))
        .and_then(|v| v.as_str());
    Some(usage_from_tokens(tokens, model))
}

/// The first non-empty `sessionID` across the parsed JSON envelope lines of an
/// `opencode run --format json` stream — the session this run created. Each
/// headless `opencode run` opens a fresh session (no continuation flag is
/// passed, ADR-0005), so this value uniquely selects this run's messages.
/// `None` when the stream carries none. Also the ledger session-id source
/// (ADR-0033 §5) — the identity the run-vs-scan dedup key is built on.
pub(crate) fn session_id_from_stream(stdout: &str) -> Option<String> {
    for line in stdout.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(val) = serde_json::from_str::<serde_json::Value>(trimmed) else {
            continue;
        };
        if let Some(sid) = val.get("sessionID").and_then(|v| v.as_str()) {
            if !sid.is_empty() {
                return Some(sid.to_owned());
            }
        }
    }
    None
}

/// Sum the token usage of every assistant message belonging to `session_id` in
/// the `OpenCode` SQLite store at `db`. Best-effort: the run never fails on it, so
/// an error yields `Usage::default()`. A missing file is silent; any other
/// failure (lock, schema drift) is logged, so zero usage is not shown for a
/// failed query.
fn sum_opencode_session_usage(db: &Path, session_id: &str) -> Usage {
    if !db.is_file() {
        return Usage::default();
    }
    read_opencode_session_usage(db, session_id).unwrap_or_else(|e| {
        tracing::warn!(
            "could not read the OpenCode token usage from {}: {e}",
            db.display()
        );
        Usage::default()
    })
}

const V1_ROWS: &str = "SELECT data FROM message WHERE session_id = ?1 ORDER BY time_created";
const V2_ROWS: &str = "SELECT data FROM session_message \
                       WHERE session_id = ?1 AND type = 'assistant' ORDER BY time_created";

fn table_exists(conn: &rusqlite::Connection, name: &str) -> rusqlite::Result<bool> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)",
        [name],
        |row| row.get(0),
    )
}

/// The summed usage of the rows `sql` selects for `session_id`; `None` when no
/// row carries tokens. The model is the last row's (rows come ordered by time).
fn sum_rows(
    conn: &rusqlite::Connection,
    sql: &str,
    session_id: &str,
    map: fn(&serde_json::Value) -> Option<Usage>,
) -> rusqlite::Result<Option<Usage>> {
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_map([session_id], |row| row.get::<_, String>(0))?;
    let mut total: Option<Usage> = None;
    for data in rows {
        let Ok(val) = serde_json::from_str::<serde_json::Value>(&data?) else {
            continue;
        };
        if let Some(u) = map(&val) {
            let sum = total.get_or_insert_with(Usage::default);
            sum.add_tokens(&u);
            if u.model.is_some() {
                sum.model = u.model;
            }
        }
    }
    Ok(total)
}

/// The fallible core of [`sum_opencode_session_usage`]. A database with
/// `session_v2` is a 2.x one: its sessions are in `session_message`, and a
/// session the 2.x upgrade did not copy is still in the 1.x `message` table.
/// `OpenCode` 1.19.0 creates an empty `session_message`, so only `session_v2` tells.
fn read_opencode_session_usage(db: &Path, session_id: &str) -> rusqlite::Result<Usage> {
    use rusqlite::{Connection, OpenFlags};
    // READ-ONLY: this run is the reader, OpenCode the writer. The writer process
    // has already exited by the time we read (see `opencode_usage` call sites),
    // so the store is quiescent. Caveat: if OpenCode keeps the DB in WAL mode, a
    // read-only handle cannot checkpoint, so rows committed but not yet
    // checkpointed are invisible — token capture then under-counts.
    let conn = Connection::open_with_flags(db, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    if table_exists(&conn, "session_v2")? {
        if let Some(usage) = sum_rows(&conn, V2_ROWS, session_id, usage_from_v2_message)? {
            return Ok(usage);
        }
        if !table_exists(&conn, "message")? {
            return Ok(Usage::default());
        }
    }
    let usage = sum_rows(&conn, V1_ROWS, session_id, usage_from_opencode_message)?;
    Ok(usage.unwrap_or_default())
}

/// Capture a phase's token usage by correlating the stream's `sessionID` to the
/// rows `OpenCode` wrote into `opencode.db`. Best-effort: `Usage::default()` when
/// no session id is on the stream or the DB is unavailable.
pub(crate) fn opencode_usage(stdout: &str) -> Usage {
    match (
        session_id_from_stream(stdout),
        ralphy_proc_util::opencode::opencode_db_path(),
    ) {
        (Some(sid), Some(db)) => sum_opencode_session_usage(&db, &sid),
        _ => Usage::default(),
    }
}

/// Return the model string from a usage record, or `"<unknown>"` when absent.
pub(crate) fn resolved_model_label(usage: &Usage) -> &str {
    usage.model.as_deref().unwrap_or("<unknown>")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cleanup::remove_scratch_file;
    use std::path::PathBuf;

    // ── usage_from_opencode_message ──────────────────────────────────────────

    #[test]
    fn usage_from_opencode_message_maps_live_row() {
        // The exact assistant `data` shape captured live (2026-06-15): cost is
        // present-and-zero and must be ignored; reasoning is 0 in this row.
        let data: serde_json::Value = serde_json::from_str(concat!(
            r#"{"role":"assistant","cost":0,"modelID":"k2p6","tokens":{"total":13532,"#,
            r#""input":2168,"output":100,"reasoning":0,"cache":{"write":0,"read":11264}}}"#,
        ))
        .unwrap();
        let usage = usage_from_opencode_message(&data).expect("assistant row maps");
        assert_eq!(usage.input, 2168);
        assert_eq!(usage.output, 100);
        assert_eq!(usage.cache_read, 11264);
        assert_eq!(usage.cache_creation, 0);
        assert_eq!(usage.model.as_deref(), Some("k2p6"));
        assert_eq!(usage.total(), 13532, "reconciles with OpenCode's own total");
    }

    #[test]
    fn reasoning_tokens_are_counted_in_output() {
        // The row of the OpenCode 1.19.0 run whose stream total was 24227 (#1040).
        let data = serde_json::json!({
            "role": "assistant", "modelID": "m", "cost": 0,
            "tokens": {"input": 22336, "output": 86, "reasoning": 13,
                       "cache": {"read": 1792, "write": 0}}
        });
        let usage = usage_from_opencode_message(&data).expect("assistant row maps");
        assert_eq!(usage.output, 99);
        assert_eq!(usage.total(), 24227);
    }

    #[test]
    fn usage_from_opencode_message_skips_non_assistant_and_tokenless() {
        // A user message (no tokens) and an assistant row without a tokens object
        // both yield None — only token-bearing assistant rows are summed.
        let user = serde_json::json!({"role": "user", "text": "hi"});
        assert!(usage_from_opencode_message(&user).is_none());
        let no_tokens = serde_json::json!({"role": "assistant", "modelID": "k2p6"});
        assert!(usage_from_opencode_message(&no_tokens).is_none());
    }

    // ── session_id_from_stream ───────────────────────────────────────────────

    #[test]
    fn session_id_from_stream_takes_first_non_empty() {
        let stream = concat!(
            r#"{"type":"step_start","sessionID":"ses_abc","part":{"type":"step-start"}}"#,
            "\n",
            r#"{"type":"text","sessionID":"ses_abc","part":{"type":"text","text":"hi"}}"#,
            "\n",
        );
        assert_eq!(session_id_from_stream(stream).as_deref(), Some("ses_abc"));
        // A stream with no sessionID yields None.
        assert_eq!(session_id_from_stream("{\"type\":\"text\"}\n"), None);
    }

    // ── sum_opencode_session_usage ───────────────────────────────────────────

    #[test]
    fn sum_opencode_session_usage_reads_row_by_session_id() {
        use rusqlite::Connection;

        let db_path = std::env::temp_dir().join(format!(
            "ralphy-opencode-usage-{}-{}.db",
            std::process::id(),
            // a stable-per-test discriminator (line number) avoids collisions with
            // sibling tests in the same process without needing a clock/random.
            line!()
        ));
        remove_scratch_file(&db_path);

        let data = concat!(
            r#"{"role":"assistant","cost":0,"modelID":"k2p6","tokens":{"total":13532,"#,
            r#""input":2168,"output":100,"reasoning":0,"cache":{"write":0,"read":11264}}}"#,
        );
        {
            let conn = Connection::open(&db_path).expect("open temp db");
            // The real `message` table schema captured from opencode.db.
            conn.execute(
                "CREATE TABLE message (id TEXT, session_id TEXT, time_created INTEGER, \
                 time_updated INTEGER, data TEXT)",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO message (id, session_id, time_created, time_updated, data) \
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params!["msg_1", "ses_test", 1, 2, data],
            )
            .unwrap();
            // A row for a DIFFERENT session must not bleed into the sum.
            conn.execute(
                "INSERT INTO message (id, session_id, time_created, time_updated, data) \
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params!["msg_2", "ses_other", 1, 2, data],
            )
            .unwrap();
        }

        let usage = sum_opencode_session_usage(&db_path, "ses_test");
        assert_eq!(usage.input, 2168);
        assert_eq!(usage.output, 100);
        assert_eq!(usage.cache_read, 11264);
        assert_eq!(usage.cache_creation, 0);
        assert_eq!(usage.model.as_deref(), Some("k2p6"));
        assert_eq!(usage.total(), 13532);

        remove_scratch_file(&db_path);
    }

    #[test]
    fn sum_opencode_session_usage_missing_db_is_zero() {
        // A missing DB is best-effort: zeroed usage, never an error.
        let usage = sum_opencode_session_usage(Path::new("/no/such/opencode.db"), "ses_x");
        assert_eq!(usage, Usage::default());
    }

    // ── both table sets, from the databases the real versions wrote ──────────

    const V1_SESSION: &str = "ses_dedb89c6affe2Bf0J714lbuYfg";
    const V2_FRESH_SESSION: &str = concat!("ses_dedb6c267ffe6qnJHwBc9o", "t2DX");
    const V2_UPGRADED_SESSION: &str = "ses_dedb7a25dffeoEMhN9M3UVV92v";

    fn load(dump: &str) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("opencode.db");
        let conn = rusqlite::Connection::open(&path).unwrap();
        // The dump lists a table before its parent table.
        conn.execute_batch("PRAGMA foreign_keys = OFF").unwrap();
        conn.execute_batch(dump).unwrap();
        (dir, path)
    }

    /// `(input, output, cache_read, cache_creation, model)` of a usage.
    fn parts(u: &Usage) -> (u64, u64, u64, u64, Option<&str>) {
        (
            u.input,
            u.output,
            u.cache_read,
            u.cache_creation,
            u.model.as_deref(),
        )
    }

    #[test]
    fn a_1x_session_is_read_from_the_message_table() {
        let (_dir, db) = load(include_str!("../tests/fixtures/db/v1.sql"));
        let usage = read_opencode_session_usage(&db, V1_SESSION).unwrap();
        // input 23840, output 3 + reasoning 14, no cache.
        assert_eq!(parts(&usage), (23840, 17, 0, 0, Some("big-pickle")));
    }

    #[test]
    fn a_2x_session_is_read_from_session_message() {
        let (_dir, db) = load(include_str!("../tests/fixtures/db/v2-fresh.sql"));
        let usage = read_opencode_session_usage(&db, V2_FRESH_SESSION).unwrap();
        assert_eq!(parts(&usage), (10217, 14, 4096, 0, Some("big-pickle")));
    }

    #[test]
    fn an_upgraded_database_reads_both_sessions() {
        let (_dir, db) = load(include_str!("../tests/fixtures/db/v2-upgraded.sql"));
        let old = read_opencode_session_usage(&db, V1_SESSION).unwrap();
        assert_eq!(parts(&old), (23840, 17, 0, 0, Some("big-pickle")));
        let new = read_opencode_session_usage(&db, V2_UPGRADED_SESSION).unwrap();
        assert_eq!(parts(&new), (13981, 16, 1536, 0, Some("big-pickle")));
    }

    #[test]
    fn a_failed_token_query_is_an_error_not_zero() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("opencode.db");
        rusqlite::Connection::open(&db)
            .unwrap()
            .execute("CREATE TABLE unrelated (id TEXT)", [])
            .unwrap();
        assert!(read_opencode_session_usage(&db, "ses_x").is_err());
    }

    /// The level and message of every event the closure emits.
    struct Lines(std::sync::Arc<std::sync::Mutex<Vec<String>>>);

    impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for Lines {
        fn on_event(
            &self,
            event: &tracing::Event<'_>,
            _ctx: tracing_subscriber::layer::Context<'_, S>,
        ) {
            struct Message(String);
            impl tracing::field::Visit for Message {
                fn record_debug(&mut self, _f: &tracing::field::Field, v: &dyn std::fmt::Debug) {
                    self.0 = format!("{v:?}");
                }
            }
            let mut message = Message(String::new());
            event.record(&mut message);
            let level = event.metadata().level();
            self.0
                .lock()
                .expect("lines")
                .push(format!("{level} {}", message.0));
        }
    }

    fn logged(f: impl FnOnce()) -> Vec<String> {
        use tracing_subscriber::layer::SubscriberExt;
        let sink = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let subscriber = tracing_subscriber::registry().with(Lines(std::sync::Arc::clone(&sink)));
        tracing::subscriber::with_default(subscriber, f);
        let lines = sink.lock().expect("lines").clone();
        lines
    }

    #[test]
    fn a_failed_token_query_is_logged() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("opencode.db");
        rusqlite::Connection::open(&db)
            .unwrap()
            .execute("CREATE TABLE unrelated (id TEXT)", [])
            .unwrap();
        let mut usage = Usage::default();
        let out = logged(|| usage = sum_opencode_session_usage(&db, "ses_x"));
        assert_eq!(usage, Usage::default());
        assert_eq!(out.len(), 1, "one line: {out:?}");
        assert!(out[0].starts_with("WARN "), "{out:?}");
        assert!(out[0].contains("no such table"), "{out:?}");
        let silent = logged(|| {
            sum_opencode_session_usage(&dir.path().join("missing.db"), "ses_x");
        });
        assert!(silent.is_empty(), "a missing file is silent: {silent:?}");
    }
}
