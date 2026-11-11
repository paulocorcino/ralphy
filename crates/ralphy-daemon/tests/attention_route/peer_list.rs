use super::*;

#[tokio::test]
async fn the_list_joins_a_peer_and_names_one_that_did_not_answer() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let store = prepare_environment(dir.path());
    let peer_home = dir.path().join("peer");
    let local_home = dir.path().join("local");
    std::fs::create_dir_all(&peer_home).expect("create the peer repo dir");
    std::fs::create_dir_all(&local_home).expect("create the local repo dir");

    let peer_port = serve(
        &peer_home,
        Some(identity(PEER_ID, "peer")),
        AuthState::fixed(
            AuthPolicy::Bearer(TOKEN.to_owned()),
            SessionEpoch::in_memory_detached(),
        ),
    )
    .await;
    peer::write_descriptor(&local_home, &descriptor(PEER_ID, peer_port))
        .expect("announce the peer");
    let dead_port = {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind a port");
        listener.local_addr().expect("a bound address").port()
    };
    peer::write_descriptor(&local_home, &descriptor(DEAD_ID, dead_port))
        .expect("announce the dead peer");
    let local_port = serve(
        &local_home,
        Some(identity(LOCAL_ID, "local")),
        AuthState::localhost(),
    )
    .await;

    // The only console runs on the peer, so its status file is the only one.
    let _console = claude_console(peer_port, Some(TOKEN)).await;
    let status = store.join("sessions").join("1.agent-status.jsonl");
    let deadline = Instant::now() + Duration::from_secs(10);
    while !status.is_file() {
        assert!(Instant::now() < deadline, "the peer console never started");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    append(&status, PERMISSION);

    let (head, reply) = wait_for(local_port, "/api/attention", |r| {
        r.as_array().is_some_and(|a| !a.is_empty())
    })
    .await;
    let requests = reply["requests"].as_array().expect("an array");
    assert_eq!(requests.len(), 1, "{reply}");
    let request = &requests[0];
    assert_eq!(request["id"], format!("{PEER_ID}/console:1"), "{reply}");
    assert_eq!(request["console"], format!("{PEER_ID}/1"), "{reply}");
    assert_eq!(request["repo"], format!("{PEER_ID}/{SLUG}"), "{reply}");
    assert_eq!(request["detail"], "permission: Bash", "{reply}");
    assert_eq!(
        header_value(&head, "x-ralphy-unanswered"),
        Some(DEAD_ID),
        "{head}"
    );
}
