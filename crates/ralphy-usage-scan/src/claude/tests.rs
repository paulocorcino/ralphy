use super::*;

/// The adapter's fixture: this copy and the adapter's are held to the same
/// rows, so the two cannot drift.
#[test]
fn cache_creation_tokens_matches_the_shared_fixture() {
    let rows: Vec<serde_json::Value> = serde_json::from_str(include_str!(
        "../../../ralphy-agent-claude/tests/fixtures/cache_creation_tokens.json"
    ))
    .expect("the fixture is JSON");
    assert!(rows.len() >= 5, "the fixture lost rows");
    for row in &rows {
        assert_eq!(
            cache_creation_tokens(&row["usage"]),
            row["tokens"].as_u64().expect("each row has tokens"),
            "{}",
            row["case"]
        );
    }
}
use std::collections::HashSet;

fn write(dir: &Path, rel: &str, content: &str) {
    let path = dir.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

fn no_runs() -> HashSet<String> {
    HashSet::new()
}

/// One usage line: an assistant `message` with model, usage, and a timestamp.
fn usage_line(model: &str, id: &str, req: Option<&str>, input: u64, ts: &str) -> String {
    let req_field = req
        .map(|r| format!("\"requestId\":\"{r}\","))
        .unwrap_or_default();
    format!(
        "{{{req_field}\"timestamp\":\"{ts}\",\"message\":{{\"id\":\"{id}\",\"model\":\"{model}\",\
         \"usage\":{{\"input_tokens\":{input},\"output_tokens\":0,\"cache_read_input_tokens\":0,\
         \"cache_creation_input_tokens\":0}}}}}}"
    )
}

#[test]
fn dedups_message_id_request_id_by_max_merge() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    // Three lines share id=m1,requestId=r1 (input 100→300→250: MAX is 300, not
    // 650 sum, not 100 first-wins); a distinct m2 line adds 50 → session 350.
    let body = [
        usage_line(
            "claude-opus-4-8",
            "m1",
            Some("r1"),
            100,
            "2026-07-10T10:00:00Z",
        ),
        usage_line(
            "claude-opus-4-8",
            "m1",
            Some("r1"),
            300,
            "2026-07-10T10:00:01Z",
        ),
        usage_line(
            "claude-opus-4-8",
            "m1",
            Some("r1"),
            250,
            "2026-07-10T10:00:02Z",
        ),
        usage_line(
            "claude-opus-4-8",
            "m2",
            Some("r2"),
            50,
            "2026-07-10T10:00:03Z",
        ),
    ]
    .join("\n");
    write(root, "ws-key/sess1.jsonl", &body);

    let records = scan_claude(&ClaudeScan {
        projects_dir: root,
        run_session_ids: &no_runs(),
        repos: &[],
        since: None,
    });
    assert_eq!(records.len(), 1);
    assert_eq!(
        records[0].tokens.as_ref().unwrap().input,
        350,
        "m1 max 300 + m2 50, not 650+50 sum"
    );
}

#[test]
fn excludes_run_owned_sessions() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write(
        root,
        "ws-key/run-sess.jsonl",
        &usage_line("m", "a", Some("r"), 10, "2026-07-10T10:00:00Z"),
    );
    write(
        root,
        "ws-key/int-sess.jsonl",
        &usage_line("m", "b", Some("r"), 20, "2026-07-10T10:00:00Z"),
    );
    let mut runs = HashSet::new();
    runs.insert("run-sess".to_owned());

    let records = scan_claude(&ClaudeScan {
        projects_dir: root,
        run_session_ids: &runs,
        repos: &[],
        since: None,
    });
    assert!(records.iter().any(|r| r.session_id == "int-sess"));
    assert!(!records.iter().any(|r| r.session_id == "run-sess"));
}

#[test]
fn attributes_dashed_cwd_dotted_and_drive_case() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write(
        root,
        "c--Dev-ralphy/s1.jsonl",
        &usage_line("m", "a", Some("r"), 10, "2026-07-10T10:00:00Z"),
    );
    write(
        root,
        "C--Dev--ralph-worktrees-issue-10/s2.jsonl",
        &usage_line("m", "b", Some("r"), 20, "2026-07-10T10:00:00Z"),
    );
    let repos = vec![
        crate::RegisteredRepo {
            slug: "o/ralphy".into(),
            path: "c:\\Dev\\ralphy".into(),
        },
        crate::RegisteredRepo {
            slug: "o/wt".into(),
            path: "C:\\Dev\\.ralph-worktrees\\issue-10".into(),
        },
    ];
    let records = scan_claude(&ClaudeScan {
        projects_dir: root,
        run_session_ids: &no_runs(),
        repos: &repos,
        since: None,
    });
    let s1 = records.iter().find(|r| r.session_id == "s1").unwrap();
    let s2 = records.iter().find(|r| r.session_id == "s2").unwrap();
    assert_eq!(
        s1.project.as_deref(),
        Some("o/ralphy"),
        "drive-case preserved"
    );
    assert_eq!(
        s2.project.as_deref(),
        Some("o/wt"),
        "dotted path → double dash"
    );
}

#[test]
fn attributes_a_checkouts_dashed_key_to_its_repo() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    let repo_fwd = repo.to_string_lossy().replace('\\', "/");
    let other = tmp.path().join("other");
    let other_fwd = other.to_string_lossy().replace('\\', "/");
    let worktrees = repo.join(".ralphy").join("worktrees");

    let wt_x = worktrees.join("x");
    fs::create_dir_all(&wt_x).unwrap();
    fs::write(
        wt_x.join(".git"),
        format!("gitdir: {repo_fwd}/.git/worktrees/x\n"),
    )
    .unwrap();
    let wt_y = worktrees.join("y");
    fs::create_dir_all(&wt_y).unwrap();
    fs::write(
        wt_y.join(".git"),
        format!("gitdir: {other_fwd}/.git/worktrees/y\n"),
    )
    .unwrap();

    let root = tmp.path().join("projects");
    write(
        &root,
        &format!("{}/s1.jsonl", dashed_cwd(&wt_x.to_string_lossy())),
        &usage_line("m", "a", Some("r"), 10, "2026-07-10T10:00:00Z"),
    );
    write(
        &root,
        &format!("{}/s2.jsonl", dashed_cwd(&wt_y.to_string_lossy())),
        &usage_line("m", "b", Some("r"), 10, "2026-07-10T10:00:00Z"),
    );

    let repos = vec![crate::RegisteredRepo {
        slug: "o/repo".into(),
        path: repo.to_string_lossy().to_string(),
    }];
    let records = scan_claude(&ClaudeScan {
        projects_dir: &root,
        run_session_ids: &no_runs(),
        repos: &repos,
        since: None,
    });
    let s1 = records.iter().find(|r| r.session_id == "s1").unwrap();
    let s2 = records.iter().find(|r| r.session_id == "s2").unwrap();
    assert_eq!(s1.project.as_deref(), Some("o/repo"));
    assert_eq!(s2.project, None, "y points at a different primary repo");
}

#[test]
fn unmatched_workspace_yields_null_project() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write(
        root,
        "some-unknown-ws/s1.jsonl",
        &usage_line("m", "a", Some("r"), 10, "2026-07-10T10:00:00Z"),
    );
    // A registered repo whose key is not this workspace: it must not claim it.
    let elsewhere = tmp.path().join("elsewhere");
    let repos = vec![crate::RegisteredRepo {
        slug: "o/elsewhere".into(),
        path: elsewhere.to_string_lossy().to_string(),
    }];
    let records = scan_claude(&ClaudeScan {
        projects_dir: root,
        run_session_ids: &no_runs(),
        repos: &repos,
        since: None,
    });
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].session_id, "s1");
    assert_eq!(records[0].project, None);
    assert_eq!(records[0].actor_email, None);
}

#[test]
fn zero_usage_session_scans_to_empty() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    // A summary line and a user line: neither carries `message.usage`.
    let body = "{\"type\":\"summary\"}\n{\"type\":\"user\",\"message\":{\"role\":\"user\"}}\n";
    write(root, "ws-key/empty.jsonl", body);
    let records = scan_claude(&ClaudeScan {
        projects_dir: root,
        run_session_ids: &no_runs(),
        repos: &[],
        since: None,
    });
    assert_eq!(records, Vec::<InteractiveRecord>::new());
}

#[test]
fn attributed_record_carries_git_actor_email() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("projects");
    // A real git repo so `git config user.email` resolves.
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
    let ws_key = dashed_cwd(&repo_path);
    write(
        &root,
        &format!("{ws_key}/s1.jsonl"),
        &usage_line("m", "a", Some("r"), 10, "2026-07-10T10:00:00Z"),
    );
    let repos = vec![crate::RegisteredRepo {
        slug: "o/repo".into(),
        path: repo_path,
    }];
    let records = scan_claude(&ClaudeScan {
        projects_dir: &root,
        run_session_ids: &no_runs(),
        repos: &repos,
        since: None,
    });
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].project.as_deref(), Some("o/repo"));
    assert_eq!(records[0].actor_email.as_deref(), Some("t@example.com"));
}

#[test]
fn since_filters_interactive_by_last_ts() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write(
        root,
        "ws-key/old.jsonl",
        &usage_line("m", "a", Some("r"), 10, "2026-07-01T10:00:00Z"),
    );
    write(
        root,
        "ws-key/new.jsonl",
        &usage_line("m", "b", Some("r"), 20, "2026-07-10T10:00:00Z"),
    );
    // `+00:00` offset bound vs the transcripts' `Z` form: parsed compare works.
    let records = scan_claude(&ClaudeScan {
        projects_dir: root,
        run_session_ids: &no_runs(),
        repos: &[],
        since: Some("2026-07-05T00:00:00+00:00"),
    });
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].session_id, "new");
    // The bound is inclusive: a session whose last line is AT it stays.
    let records = scan_claude(&ClaudeScan {
        projects_dir: root,
        run_session_ids: &no_runs(),
        repos: &[],
        since: Some("2026-07-10T10:00:00+00:00"),
    });
    assert_eq!(records.len(), 1, "the bound is inclusive");
    assert_eq!(records[0].session_id, "new");
}

#[test]
fn a_message_id_without_request_id_dedups_by_max_merge() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let body = [
        usage_line("m", "m1", None, 100, "2026-07-10T10:00:00Z"),
        usage_line("m", "m1", None, 300, "2026-07-10T10:00:01Z"),
    ]
    .join("\n");
    write(root, "ws-key/sess1.jsonl", &body);
    let records = scan_claude(&ClaudeScan {
        projects_dir: root,
        run_session_ids: &no_runs(),
        repos: &[],
        since: None,
    });
    assert_eq!(records.len(), 1);
    assert_eq!(
        records[0].tokens.as_ref().unwrap().input,
        300,
        "one id, no requestId: the max 300, not the sum 400"
    );
}

#[test]
fn timestamps_widen_to_the_first_and_last_line_in_any_order() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let body = [
        usage_line("m", "a", Some("r"), 1, "2026-07-10T10:00:05Z"),
        usage_line("m", "b", Some("r"), 1, "2026-07-10T10:00:01Z"),
        usage_line("m", "c", Some("r"), 1, "2026-07-10T10:00:09Z"),
        usage_line("m", "d", Some("r"), 1, "2026-07-10T10:00:03Z"),
    ]
    .join("\n");
    write(root, "ws-key/sess1.jsonl", &body);
    let records = scan_claude(&ClaudeScan {
        projects_dir: root,
        run_session_ids: &no_runs(),
        repos: &[],
        since: None,
    });
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].first_ts, "2026-07-10T10:00:01Z");
    assert_eq!(records[0].last_ts, "2026-07-10T10:00:09Z");
}
