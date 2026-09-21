use super::*;
use crate::RegisteredRepo;
use rusqlite::Connection;
use std::collections::HashSet;
use std::fs;

const CREATE_USAGE: &str = "CREATE TABLE assistant_usage_events (\
         id INTEGER PRIMARY KEY AUTOINCREMENT, session_id TEXT, turn_index INTEGER, \
         model TEXT, input_tokens INTEGER, output_tokens INTEGER, \
         cache_read_tokens INTEGER, cache_write_tokens INTEGER, \
         reasoning_tokens INTEGER, reasoning_effort TEXT, token_details_json TEXT, \
         created_at TEXT)";
/// The same table as the live store MINUS `reasoning_effort` — the schema-drift
/// shape the effort reader has to survive.
const CREATE_USAGE_NO_EFFORT: &str = "CREATE TABLE assistant_usage_events (\
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
    assert_eq!(records[0].tokens.as_ref().unwrap().input, 10);
    assert!(
        !records[0].lower_bound,
        "Copilot writes every token to disk — this is a total, not a floor"
    );
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
    assert_eq!(records[0].tokens.as_ref().unwrap().input, 100);
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
    assert_eq!(records[0].tokens.as_ref().unwrap().input, 46258);
}

#[test]
fn copilot_attributes_a_linked_worktree_cwd_to_its_repo() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    let repo_fwd = repo.to_string_lossy().replace('\\', "/");
    let wt = tmp.path().join("wt");
    std::fs::create_dir_all(&wt).unwrap();
    std::fs::write(
        wt.join(".git"),
        format!("gitdir: {repo_fwd}/.git/worktrees/wt\n"),
    )
    .unwrap();
    let wt_native = wt.to_string_lossy().to_string();

    let db = seed_p2(tmp.path(), "ses_wt");
    let conn = Connection::open(&db).unwrap();
    conn.execute(
        "INSERT INTO sessions (id, cwd) VALUES (?1, ?2)",
        rusqlite::params!["ses_wt", wt_native],
    )
    .unwrap();
    drop(conn);
    let repos = vec![RegisteredRepo {
        slug: "o/repo".into(),
        path: repo.to_string_lossy().to_string(),
    }];
    let records = scan_copilot(&CopilotScan {
        db_path: &db,
        run_session_ids: &HashSet::new(),
        repos: &repos,
        since: None,
    });
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].project.as_deref(), Some("o/repo"));
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

/// The post-hoc effort oracle: the LAST recorded level wins, an unknown session
/// is `None`, and a store whose schema lost the column degrades to `None`
/// instead of panicking.
#[test]
fn session_reasoning_effort_reads_the_recorded_level() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("session-store.db");
    let conn = Connection::open(&path).unwrap();
    conn.execute(CREATE_USAGE, []).unwrap();
    for level in ["low", "high"] {
        conn.execute(
            "INSERT INTO assistant_usage_events (session_id, turn_index, model, \
                 input_tokens, output_tokens, reasoning_effort, created_at) \
                 VALUES ('ses_e', 0, 'm', 1, 1, ?1, '2026-07-20T11:54:33.066Z')",
            rusqlite::params![level],
        )
        .unwrap();
    }
    // The live store carries NULLs in this column: the LAST NON-NULL wins, so
    // a trailing NULL row must not erase the answer.
    conn.execute(
        "INSERT INTO assistant_usage_events (session_id, turn_index, model, \
             input_tokens, output_tokens, reasoning_effort, created_at) \
             VALUES ('ses_e', 0, 'm', 1, 1, NULL, '2026-07-20T11:54:33.066Z')",
        [],
    )
    .unwrap();
    drop(conn);
    assert_eq!(
        session_reasoning_effort(&path, "ses_e"),
        Some("high".to_string()),
        "the chronologically last NON-NULL row wins"
    );
    assert_eq!(session_reasoning_effort(&path, "ses_nobody"), None);

    let drifted = tmp.path().join("drifted");
    fs::create_dir_all(&drifted).unwrap();
    let dpath = drifted.join("session-store.db");
    let dconn = Connection::open(&dpath).unwrap();
    dconn.execute(CREATE_USAGE_NO_EFFORT, []).unwrap();
    insert(
        &dconn,
        &Row {
            session_id: "ses_e",
            turn_index: 0,
            model: "claude-sonnet-5",
            input: 1,
            output: 1,
            cache_read: 0,
            cache_write: 0,
            reasoning: 0,
            created_at: "2026-07-20T11:54:33.066Z",
        },
    );
    drop(dconn);
    assert_eq!(
        session_reasoning_effort(&dpath, "ses_e"),
        None,
        "a renamed/dropped column degrades to None, never a panic"
    );
    assert_eq!(
        session_reasoning_effort(Path::new("does-not-exist-session-store.db"), "ses_e"),
        None
    );
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
