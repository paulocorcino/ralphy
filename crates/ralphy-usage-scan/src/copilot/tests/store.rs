use super::*;

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
