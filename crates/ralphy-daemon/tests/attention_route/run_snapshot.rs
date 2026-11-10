use super::*;

#[tokio::test]
async fn a_run_snapshot_in_a_registered_repo_is_listed() {
    let dir = tempfile::tempdir().expect("a temp dir");
    prepare_environment(dir.path());
    let home = dir.path().join("home");
    std::fs::create_dir_all(&home).expect("create the repo dir");
    let port = serve(&home, None, AuthState::localhost()).await;
    let mut presence = page::connect(port).await;

    // The run is this test: the reader keeps a document whose pid lives.
    let mut snapshot = ralphy_run_snapshot::RunSnapshot {
        runid: "01RUNWAITS".to_string(),
        pid: std::process::id(),
        exec_agent: "codex".to_string(),
        ..ralphy_run_snapshot::RunSnapshot::default()
    };
    snapshot.phase.state = "executing".to_string();
    snapshot.phase.active = Some(71);
    snapshot.phase.agent = Some(ralphy_run_snapshot::AgentBlock {
        state: "waiting".to_string(),
        since: "2026-10-28T11:00:00-03:00".to_string(),
        detail: Some("permission: Bash".to_string()),
    });
    ralphy_run_snapshot::write_atomic(&home, &snapshot).expect("write the run snapshot");
    let pushed = next_push(&mut presence, "attention.dirty").await;
    golden::check("attention.dirty", pushed, &[]);

    let (_, reply) = wait_for(port, "/api/attention", |r| {
        r.as_array().is_some_and(|a| !a.is_empty())
    })
    .await;
    let requests = reply["requests"].as_array().expect("an array");
    assert_eq!(requests.len(), 1, "{reply}");
    let request = &requests[0];
    assert_eq!(request["id"], "run:01RUNWAITS", "{reply}");
    assert_eq!(request["kind"], "run-waiting", "{reply}");
    assert_eq!(request["run"], "01RUNWAITS", "{reply}");
    assert_eq!(request["repo"], SLUG, "{reply}");
    assert_eq!(request["issue"], "71", "{reply}");
    assert_eq!(request["detail"], "permission: Bash", "{reply}");

    std::fs::remove_file(ralphy_run_snapshot::snapshot_path(&home, "01RUNWAITS"))
        .expect("remove the run snapshot");
    let (_, reply) = wait_for(port, "/api/attention", |r| {
        r.as_array().is_some_and(Vec::is_empty)
    })
    .await;
    assert_eq!(reply["requests"], serde_json::json!([]), "{reply}");
}
