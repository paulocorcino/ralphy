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

fn scan(db_path: &Path, repos: &[RegisteredRepo], since: Option<&str>) -> Vec<InteractiveRecord> {
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
    let data = concat!(
        r#"{"role":"assistant","cost":999.0,"modelID":"k2p6","tokens":{"input":2168,"#,
        r#""output":100,"reasoning":40,"cache":{"write":0,"read":11264}}}"#
    );
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
fn opencode_attributes_a_linked_worktree_directory_to_its_repo() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    let repo_fwd = repo.to_string_lossy().replace('\\', "/");
    let wt = tmp.path().join("wt");
    fs::create_dir_all(&wt).unwrap();
    fs::write(
        wt.join(".git"),
        format!("gitdir: {repo_fwd}/.git/worktrees/wt\n"),
    )
    .unwrap();
    let wt_native = wt.to_string_lossy().to_string();

    let data = r#"{"role":"assistant","modelID":"k2p6","tokens":{"input":10,"output":5}}"#;
    let db = seed_db(tmp.path(), &[("ses_wt", data)], &[("ses_wt", &wt_native)]);
    let repos = vec![RegisteredRepo {
        slug: "o/repo".into(),
        path: repo.to_string_lossy().to_string(),
    }];
    let records = scan(&db, &repos, None);
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].project.as_deref(), Some("o/repo"));
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
    runs.insert("ses_run".to_owned());
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
    assert_eq!(records, Vec::<InteractiveRecord>::new());
}

#[test]
fn opencode_corrupt_db_degrades_to_zero() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("opencode.db");
    fs::write(&path, b"this is not a sqlite database at all").unwrap();
    let records = scan(&path, &[], None);
    assert_eq!(records, Vec::<InteractiveRecord>::new());
}

#[test]
fn opencode_since_filters_by_last_ts() {
    let tmp = tempfile::tempdir().unwrap();
    // created ms: old = 2026-07-01, new = 2026-07-10.
    let old = concat!(
        r#"{"role":"assistant","modelID":"m","tokens":{"input":10,"output":5},"#,
        r#""time":{"created":1782000000000}}"#
    );
    let new = concat!(
        r#"{"role":"assistant","modelID":"m","tokens":{"input":20,"output":5},"#,
        r#""time":{"created":1783000000000}}"#
    );
    let db = seed_db(tmp.path(), &[("ses_old", old), ("ses_new", new)], &[]);
    // since between the two created instants.
    let between = ms_to_rfc3339(Some(1_782_500_000_000));
    let records = scan(&db, &[], Some(&between));
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].session_id, "ses_new");
    // The bound is inclusive: a session whose last message is AT it stays.
    let at = ms_to_rfc3339(Some(1_783_000_000_000));
    let records = scan(&db, &[], Some(&at));
    assert_eq!(records.len(), 1, "the bound is inclusive");
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

#[test]
fn opencode_db_without_a_session_table_still_yields_the_record() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("opencode.db");
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute(
        "CREATE TABLE message (id TEXT, session_id TEXT, data TEXT)",
        [],
    )
    .unwrap();
    let data = r#"{"role":"assistant","modelID":"k2p6","tokens":{"input":7,"output":3}}"#;
    conn.execute(
        "INSERT INTO message (id, session_id, data) VALUES ('m0', 'ses_1', ?1)",
        [data],
    )
    .unwrap();
    drop(conn);

    let records = scan(&path, &[], None);

    assert_eq!(records.len(), 1);
    assert_eq!(records[0].session_id, "ses_1");
    assert_eq!(records[0].project, None);
    assert_eq!(records[0].tokens.as_ref().unwrap().input, 7);
}

#[test]
fn opencode_rows_of_one_session_and_model_sum_and_span_their_times() {
    let tmp = tempfile::tempdir().unwrap();
    // Written out of time order, so the span is not the first or last row.
    let mid = concat!(
        r#"{"role":"assistant","modelID":"m","tokens":{"input":10,"output":1,"#,
        r#""cache":{"read":2,"write":3}},"time":{"created":1782500000000}}"#
    );
    let late = concat!(
        r#"{"role":"assistant","modelID":"m","tokens":{"input":20,"output":4,"#,
        r#""cache":{"read":5,"write":6}},"time":{"created":1783000000000}}"#
    );
    let early = concat!(
        r#"{"role":"assistant","modelID":"m","tokens":{"input":30,"output":9,"#,
        r#""cache":{"read":7,"write":8}},"time":{"created":1782000000000}}"#
    );
    let db = seed_db(
        tmp.path(),
        &[("ses_1", mid), ("ses_1", late), ("ses_1", early)],
        &[],
    );

    let records = scan(&db, &[], None);

    assert_eq!(records.len(), 1);
    let r = &records[0];
    let t = r.tokens.as_ref().unwrap();
    assert_eq!(
        (t.input, t.output, t.cache_read, t.cache_creation),
        (60, 14, 14, 17)
    );
    assert_eq!(r.first_ts, ms_to_rfc3339(Some(1_782_000_000_000)));
    assert_eq!(r.last_ts, ms_to_rfc3339(Some(1_783_000_000_000)));
}
