use super::*;

#[tokio::test]
async fn a_peer_request_is_dismissed_on_its_peer() {
    let dir = tempfile::tempdir().expect("a temp dir");
    prepare_environment(dir.path());
    let peer_home = dir.path().join("peer");
    let local_home = dir.path().join("local");
    std::fs::create_dir_all(&peer_home).expect("create the peer repo dir");
    std::fs::create_dir_all(&local_home).expect("create the local repo dir");
    finished_run(&peer_home);
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
    let local_port = serve(
        &local_home,
        Some(identity(LOCAL_ID, "local")),
        AuthState::localhost(),
    )
    .await;
    let on_peer = |n| format!("{PEER_ID}/{}", issue(n));
    wait_for(local_port, "/api/attention", |r| {
        ids(r) == [on_peer(71), on_peer(72)]
    })
    .await;

    let (status, body) = dismiss(local_port, &on_peer(71)).await;
    assert_eq!(status, 200, "{body}");
    let file = std::fs::read_to_string(peer_home.join("attention-dismissed.jsonl"))
        .expect("the peer stored the dismissal");
    assert!(file.contains(&issue(71)), "{file}");
    assert!(
        !local_home.join("attention-dismissed.jsonl").exists(),
        "the local daemon stores no dismissal of a peer's request"
    );
    wait_for(local_port, "/api/attention", |r| ids(r) == [on_peer(72)]).await;
}
