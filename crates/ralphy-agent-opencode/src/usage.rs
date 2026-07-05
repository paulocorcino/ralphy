//! OpenCode token-usage capture, correlating a phase's `--format json` stream
//! `sessionID` to the rows OpenCode wrote into its SQLite message store
//! (ADR-0008 D5).

use std::path::{Path, PathBuf};

use ralphy_core::Usage;

/// Map an OpenCode assistant message's `data` JSON to a normalized [`Usage`]
/// (ADR-0008 D5). Requires `role == "assistant"` with a `tokens` object; maps
/// `input = tokens.input`, `output = tokens.output`,
/// `cache_read = tokens.cache.read`, `cache_creation = tokens.cache.write`, and
/// `model = data.modelID`. `tokens.reasoning` is NOT added — OpenCode's own
/// `total = input + output + cache.read` reconciles without it (verified live on
/// 8 rows), so reasoning already sits inside `output`. The CLI `cost` field is
/// never read: it is `0` for the operator's un-priced custom providers.
fn usage_from_opencode_message(data: &serde_json::Value) -> Option<Usage> {
    if data.get("role").and_then(|v| v.as_str()) != Some("assistant") {
        return None;
    }
    let tokens = data.get("tokens").filter(|t| t.is_object())?;
    let field = |obj: &serde_json::Value, k: &str| obj.get(k).and_then(|v| v.as_u64()).unwrap_or(0);
    let cache = tokens.get("cache");
    Some(Usage {
        input: field(tokens, "input"),
        output: field(tokens, "output"),
        cache_read: cache.map(|c| field(c, "read")).unwrap_or(0),
        cache_creation: cache.map(|c| field(c, "write")).unwrap_or(0),
        model: data
            .get("modelID")
            .and_then(|v| v.as_str())
            .map(str::to_string),
    })
}

/// The first non-empty `sessionID` across the parsed JSON envelope lines of an
/// `opencode run --format json` stream — the session this run created. Each
/// headless `opencode run` opens a fresh session (no continuation flag is
/// passed, ADR-0005), so this value uniquely selects this run's messages.
/// `None` when the stream carries none.
fn session_id_from_stream(stdout: &str) -> Option<String> {
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
                return Some(sid.to_string());
            }
        }
    }
    None
}

/// `<home>/.local/share/opencode/opencode.db` — the SQLite store OpenCode writes
/// message records into (`USERPROFILE` on Windows, `HOME` elsewhere). `None` when
/// no home is known.
fn opencode_db_path() -> Option<PathBuf> {
    ralphy_adapter_support::home_scoped_path(
        None,
        Path::new(".local/share/opencode"),
        Path::new("opencode.db"),
    )
}

/// Sum the token usage of every assistant message belonging to `session_id` in
/// the OpenCode SQLite store at `db` (ADR-0008 D5). Opens read-only and is fully
/// best-effort: any error (missing DB, lock, schema drift) yields
/// `Usage::default()` so token capture never fails the run. The last seen
/// per-record `modelID` is carried as the usage model (D8).
fn sum_opencode_session_usage(db: &Path, session_id: &str) -> Usage {
    read_opencode_session_usage(db, session_id).unwrap_or_default()
}

/// The fallible core of [`sum_opencode_session_usage`], separated so every
/// `rusqlite` error funnels through one `unwrap_or_default` rather than failing
/// the run.
fn read_opencode_session_usage(db: &Path, session_id: &str) -> rusqlite::Result<Usage> {
    use rusqlite::{Connection, OpenFlags};
    // READ-ONLY: this run is the reader, OpenCode the writer. The writer process
    // has already exited by the time we read (see `opencode_usage` call sites),
    // so the store is quiescent. Caveat: if OpenCode keeps the DB in WAL mode, a
    // read-only handle cannot checkpoint, so rows committed but not yet
    // checkpointed are invisible — token capture then under-counts rather than
    // failing. Acceptable for a best-effort measurement path.
    let conn = Connection::open_with_flags(db, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    // ORDER BY time_created so "last record's model" below is deterministic
    // rather than relying on implementation-defined row order.
    let mut stmt =
        conn.prepare("SELECT data FROM message WHERE session_id = ?1 ORDER BY time_created")?;
    let rows = stmt.query_map([session_id], |row| row.get::<_, String>(0))?;
    let mut total = Usage::default();
    for data in rows.flatten() {
        let Ok(val) = serde_json::from_str::<serde_json::Value>(&data) else {
            continue;
        };
        if let Some(u) = usage_from_opencode_message(&val) {
            total.add_tokens(&u);
            // add_tokens leaves `model` untouched; carry the last record's model
            // (rows are ordered by time_created, so this is the chronologically
            // last assistant message's model).
            if u.model.is_some() {
                total.model = u.model;
            }
        }
    }
    Ok(total)
}

/// Capture a phase's token usage by correlating the stream's `sessionID` to the
/// rows OpenCode wrote into `opencode.db`. Best-effort: `Usage::default()` when
/// no session id is on the stream or the DB is unavailable.
pub(crate) fn opencode_usage(stdout: &str) -> Usage {
    match (session_id_from_stream(stdout), opencode_db_path()) {
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
    use std::fs;

    // ── resolved_model_label ────────────────────────────────────────────────

    #[test]
    fn resolved_model_label_returns_model_or_unknown() {
        assert_eq!(
            resolved_model_label(&Usage {
                model: Some("k2p6".into()),
                ..Default::default()
            }),
            "k2p6"
        );
        assert_eq!(resolved_model_label(&Usage::default()), "<unknown>");
    }

    // ── usage_from_opencode_message ──────────────────────────────────────────

    #[test]
    fn usage_from_opencode_message_maps_live_row() {
        // The exact assistant `data` shape captured live (2026-06-15): cost is
        // present-and-zero (must be ignored) and reasoning is a separate field
        // that must NOT be added (total reconciles as input+output+cache.read).
        let data: serde_json::Value = serde_json::from_str(
            r#"{"role":"assistant","cost":0,"modelID":"k2p6","tokens":{"total":13532,"input":2168,"output":100,"reasoning":0,"cache":{"write":0,"read":11264}}}"#,
        )
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
        let _ = fs::remove_file(&db_path);

        let data = r#"{"role":"assistant","cost":0,"modelID":"k2p6","tokens":{"total":13532,"input":2168,"output":100,"reasoning":0,"cache":{"write":0,"read":11264}}}"#;
        {
            let conn = Connection::open(&db_path).expect("open temp db");
            // The real `message` table schema captured from opencode.db.
            conn.execute(
                "CREATE TABLE message (id TEXT, session_id TEXT, time_created INTEGER, time_updated INTEGER, data TEXT)",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO message (id, session_id, time_created, time_updated, data) VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params!["msg_1", "ses_test", 1, 2, data],
            )
            .unwrap();
            // A row for a DIFFERENT session must not bleed into the sum.
            conn.execute(
                "INSERT INTO message (id, session_id, time_created, time_updated, data) VALUES (?1, ?2, ?3, ?4, ?5)",
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

        let _ = fs::remove_file(&db_path);
    }

    #[test]
    fn sum_opencode_session_usage_missing_db_is_zero() {
        // A missing DB is best-effort: zeroed usage, never an error.
        let usage = sum_opencode_session_usage(Path::new("/no/such/opencode.db"), "ses_x");
        assert_eq!(usage, Usage::default());
    }
}
