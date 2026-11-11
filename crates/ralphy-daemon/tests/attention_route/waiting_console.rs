use super::*;

#[tokio::test]
async fn a_waiting_console_enters_and_leaves_the_route() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let store = prepare_environment(dir.path());
    let home = dir.path().join("home");
    std::fs::create_dir_all(&home).expect("create the repo dir");
    let port = serve(&home, None, AuthState::localhost()).await;
    let _console = claude_console(port, None).await;

    let (_, reply) = wait_for(port, "/api/attention", serde_json::Value::is_array).await;
    assert_eq!(reply["requests"], serde_json::json!([]), "{reply}");

    let status = store.join("sessions").join("1.agent-status.jsonl");
    append(&status, PERMISSION);
    let (_, reply) = wait_for(port, "/api/attention", |r| {
        r.as_array().is_some_and(|a| !a.is_empty())
    })
    .await;
    let requests = reply["requests"].as_array().expect("an array");
    assert_eq!(requests.len(), 1, "{reply}");
    let request = &requests[0];
    assert_eq!(request["id"], "console:1", "{reply}");
    assert_eq!(request["kind"], "console-waiting", "{reply}");
    assert_eq!(request["vendor"], "claude", "{reply}");
    assert_eq!(request["console"], "1", "{reply}");
    assert_eq!(request["repo"], SLUG, "{reply}");
    assert_eq!(request["detail"], "permission: Bash", "{reply}");
    assert_eq!(request["since"], "2026-10-28T10:00:00-03:00", "{reply}");
    assert_eq!(request["dismissable"], false, "{reply}");
    golden::check("api-attention", reply.clone(), &[]);

    append(&status, ANSWERED);
    let (_, reply) = wait_for(port, "/api/attention", |r| {
        r.as_array().is_some_and(Vec::is_empty)
    })
    .await;
    assert_eq!(reply["requests"], serde_json::json!([]), "{reply}");
}
