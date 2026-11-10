use super::*;

#[tokio::test]
async fn a_dismissal_survives_a_daemon_restart() {
    let dir = tempfile::tempdir().expect("a temp dir");
    prepare_environment(dir.path());
    let home = dir.path().join("home");
    std::fs::create_dir_all(&home).expect("create the repo dir");
    finished_run(&home);
    let port = serve(&home, None, AuthState::localhost()).await;
    let (_, reply) = wait_for(port, "/api/attention", |r| ids(r) == [issue(71), issue(72)]).await;
    // `since` is when the final snapshot was written.
    golden::check(
        "api-attention--finished",
        reply,
        &["/requests/0/since", "/requests/1/since"],
    );

    let (status, body) = dismiss(port, &issue(71)).await;
    assert_eq!(status, 200, "{body}");
    let reply: serde_json::Value = serde_json::from_str(&body).expect("a JSON reply");
    golden::check("api-attention-dismiss", reply, &[]);
    wait_for(port, "/api/attention", |r| ids(r) == [issue(72)]).await;

    // A second daemon on the same store reads the dismissal back.
    let again = serve(&home, None, AuthState::localhost()).await;
    let (_, reply) = wait_for(again, "/api/attention", |r| !ids(r).is_empty()).await;
    assert_eq!(ids(&reply["requests"]), [issue(72)], "{reply}");
}

#[tokio::test]
async fn a_second_dismissal_is_a_no_op() {
    let dir = tempfile::tempdir().expect("a temp dir");
    prepare_environment(dir.path());
    let home = dir.path().join("home");
    std::fs::create_dir_all(&home).expect("create the repo dir");
    finished_run(&home);
    let port = serve(&home, None, AuthState::localhost()).await;
    wait_for(port, "/api/attention", |r| ids(r).contains(&issue(71))).await;

    for round in ["first", "second"] {
        let (status, body) = dismiss(port, &issue(71)).await;
        assert_eq!(status, 200, "{round}: {body}");
    }
    let file = std::fs::read_to_string(home.join("attention-dismissed.jsonl"))
        .expect("the dismissals file");
    assert_eq!(file.lines().count(), 1, "{file}");
    let line: serde_json::Value =
        serde_json::from_str(file.lines().next().unwrap_or_default()).expect("a JSON line");
    assert_eq!(line["id"], issue(71), "{line}");
    assert!(line["ts"].is_string(), "{line}");
}

#[tokio::test]
async fn a_live_request_cannot_be_dismissed() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let store = prepare_environment(dir.path());
    let home = dir.path().join("home");
    std::fs::create_dir_all(&home).expect("create the repo dir");
    let port = serve(&home, None, AuthState::localhost()).await;
    let _console = claude_console(port, None).await;
    let status_file = store.join("sessions").join("1.agent-status.jsonl");
    let deadline = Instant::now() + Duration::from_secs(10);
    while !status_file.is_file() {
        assert!(Instant::now() < deadline, "the console never started");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    append(&status_file, PERMISSION);
    let mut snapshot = ralphy_run_snapshot::RunSnapshot {
        runid: "01RUNWAITS".to_string(),
        pid: std::process::id(),
        exec_agent: "codex".to_string(),
        ..ralphy_run_snapshot::RunSnapshot::default()
    };
    snapshot.phase.state = "executing".to_string();
    snapshot.phase.agent = Some(ralphy_run_snapshot::AgentBlock {
        state: "waiting".to_string(),
        since: "2026-10-28T11:00:00-03:00".to_string(),
        detail: None,
    });
    ralphy_run_snapshot::write_atomic(&home, &snapshot).expect("write the run snapshot");
    let both = ["console:1".to_string(), "run:01RUNWAITS".to_string()];
    wait_for(port, "/api/attention", |r| ids(r) == both).await;

    for id in &both {
        let (status, body) = dismiss(port, id).await;
        assert_eq!(status, 409, "{id}: {body}");
        assert_eq!(
            body, "the request is still open and cannot be hidden",
            "{id}"
        );
    }
    let (status, body) = dismiss(port, &issue(99)).await;
    assert_eq!(status, 404, "an unknown id: {body}");
    let (_, reply) = wait_for(port, "/api/attention", serde_json::Value::is_array).await;
    assert_eq!(ids(&reply["requests"]), both, "{reply}");
    assert!(!home.join("attention-dismissed.jsonl").exists());
}
