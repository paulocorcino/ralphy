use super::*;

#[tokio::test]
async fn peer_launch_stream_resize_and_interrupt_are_native() {
    let (_peer_store, peer_repo, _local_store, local_repo, peer, local) = fixture().await;
    let mut ws = launch(local.port).await;
    ws.send(terminal(b"peer-input\r")).await.unwrap();
    let seen = read_until(&mut ws, "GOT:peer-input", true).await;
    let open = seen
        .open
        .expect("session-open must precede terminal output");
    assert_eq!(seen.first_frame.as_deref(), Some("session-open"));
    assert_eq!(open["daemon_id"], PEER_ID);
    assert_eq!(open["environment"], ENVIRONMENT);
    assert!(
        names_cwd(&seen.terminal, peer_repo.path()),
        "peer cwd missing: {}",
        seen.terminal
    );
    assert!(
        !names_cwd(&seen.terminal, local_repo.path()),
        "local collision spawned: {}",
        seen.terminal
    );

    ws.send(resize(40, 100)).await.unwrap();
    let resized = read_until(&mut ws, "SIZE 100x40", false).await;
    assert!(resized.terminal.replace("\r\n", "").contains("SIZE 100x40"));

    let mut interrupted = launch(local.port).await;
    let opened = read_until(&mut interrupted, "READY", true).await;
    let interrupted_id = opened.open.unwrap()["session"].as_u64().unwrap();
    interrupted.send(terminal(&[0x03])).await.unwrap();
    let ended = read_until(&mut interrupted, "session-end", false).await;
    let end = ended.end.unwrap();
    assert!(
        ended
            .terminal
            .replace("\r\n", "")
            .contains("INTERRUPTED:ETX"),
        "native child did not observe ETX before exit: {}",
        ended.terminal
    );
    assert_eq!(end["reason"], "child-exited");
    assert_eq!(end["daemon_id"], PEER_ID);
    assert_eq!(end["environment"], ENVIRONMENT);
    assert_eq!(ended.open, None);

    let (_, peer_body) = http_request(peer.port, "GET", "/api/sessions", Some("peer-token")).await;
    let peer_rows: serde_json::Value = serde_json::from_str(&peer_body).unwrap();
    assert!(
        peer_rows
            .as_array()
            .unwrap()
            .iter()
            .all(|row| row["id"] != interrupted_id),
        "interrupted session remained listed: {peer_rows}"
    );
}

#[tokio::test]
async fn browser_and_proxy_drops_reattach_to_peer_scrollback() {
    let (_peer_store, _peer_repo, local_store, _local_repo, peer, local) = fixture().await;
    let mut ws = launch(local.port).await;
    ws.send(terminal(b"marker-alpha\r")).await.unwrap();
    let first = read_until(&mut ws, "GOT:marker-alpha", true).await;
    let id = first.open.unwrap()["session"].as_u64().unwrap();
    ws.close(None).await.unwrap();
    drop(ws);

    let (_, listed) = http_request(local.port, "GET", "/api/sessions", None).await;
    let rows: serde_json::Value = serde_json::from_str(&listed).unwrap();
    assert_eq!(rows.as_array().unwrap().len(), 1);
    assert_eq!(rows[0]["id"], id);
    assert_eq!(rows[0]["repo"], peer_repo());
    assert_eq!(rows[0]["daemon_id"], PEER_ID);

    let mut ws = attach(local.port, id).await;
    assert!(read_until(&mut ws, "GOT:marker-alpha", false)
        .await
        .terminal
        .replace("\r\n", "")
        .contains("GOT:marker-alpha"));
    ws.send(terminal(b"marker-beta\r")).await.unwrap();
    assert!(read_until(&mut ws, "GOT:marker-beta", false)
        .await
        .terminal
        .replace("\r\n", "")
        .contains("GOT:marker-beta"));
    ws.send(terminal(b"proxy-survivor\r")).await.unwrap();
    read_until(&mut ws, "GOT:proxy-survivor", false).await;

    local.shutdown.send(true).unwrap();
    local.task.abort();
    let (_, peer_listed) =
        http_request(peer.port, "GET", "/api/sessions", Some("peer-token")).await;
    let peer_rows: serde_json::Value = serde_json::from_str(&peer_listed).unwrap();
    assert_eq!(peer_rows[0]["id"], id);

    let replacement = serve(
        identity(LOCAL_ID, "local"),
        local_store.path().join("repos.toml"),
        AuthState::localhost(),
    )
    .await;
    let mut restored = attach(replacement.port, id).await;
    assert!(read_until(&mut restored, "GOT:proxy-survivor", false)
        .await
        .terminal
        .replace("\r\n", "")
        .contains("GOT:proxy-survivor"));
}

#[tokio::test]
async fn federated_list_and_close_keep_colliding_ids_distinct() {
    let (_peer_store, _peer_repo, _local_store, _local_repo, _peer, local) = fixture().await;
    let mut local_ws = launch_local(local.port).await;
    let local_open = read_until(&mut local_ws, "READY", true).await;
    let local_id = local_open.open.unwrap()["session"].as_u64().unwrap();
    let mut peer_ws = launch(local.port).await;
    let peer_open = read_until(&mut peer_ws, "READY", true).await;
    let peer_id = peer_open.open.unwrap()["session"].as_u64().unwrap();
    assert_eq!(
        local_id, peer_id,
        "fixture must create the numeric collision"
    );

    let (_, body) = http_request(local.port, "GET", "/api/sessions", None).await;
    let rows: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(rows.as_array().unwrap().len(), 2, "got {rows}");
    assert!(rows
        .as_array()
        .unwrap()
        .iter()
        .any(|row| row["id"] == local_id && row["repo"] == SLUG && row["daemon_id"] == LOCAL_ID));
    assert!(rows
        .as_array()
        .unwrap()
        .iter()
        .any(|row| row["id"] == peer_id
            && row["repo"] == peer_repo()
            && row["daemon_id"] == PEER_ID));

    let repo = peer_repo().replace('/', "%2F");
    let (status, _) = http_request(
        local.port,
        "POST",
        &format!("/api/sessions/close?id={peer_id}&repo={repo}"),
        None,
    )
    .await;
    assert_eq!(status, 200);
    let (_, body) = http_request(local.port, "GET", "/api/sessions", None).await;
    let rows: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(rows.as_array().unwrap().len(), 1, "got {rows}");
    assert_eq!(rows[0]["id"], local_id);
    assert_eq!(rows[0]["repo"], SLUG);
    assert_eq!(rows[0]["daemon_id"], LOCAL_ID);
}

/// A peer that does not answer is NAMED by the list, not left out of it: the
/// page reads "no sessions on that peer" otherwise, and relaunches consoles
/// that still run there (a second PTY on a peer without the record join).
#[tokio::test]
async fn the_session_list_names_the_peers_that_did_not_answer() {
    let (_peer_store, _peer_repo, local_store, _local_repo, _peer, local) = fixture().await;
    let mut peer_ws = launch(local.port).await;
    let peer_open = read_until(&mut peer_ws, "READY", true).await;
    let peer_id = peer_open.open.unwrap()["session"].as_u64().unwrap();

    let (status, head, _) = http_exchange(local.port, "GET", "/api/sessions", None).await;
    assert_eq!(status, 200);
    assert_eq!(
        header_value(&head, "x-ralphy-unanswered"),
        None,
        "every peer answered: {head}"
    );

    let closed_port = {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        listener.local_addr().unwrap().port()
    };
    peer::write_descriptor(local_store.path(), &descriptor(DEAD_ID, closed_port)).unwrap();
    let (status, head, body) = http_exchange(local.port, "GET", "/api/sessions", None).await;
    assert_eq!(status, 200);
    assert_eq!(
        header_value(&head, "x-ralphy-unanswered"),
        Some(DEAD_ID),
        "{head}"
    );
    // The peer that answered is still listed.
    let rows: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert!(
        rows.as_array()
            .unwrap()
            .iter()
            .any(|row| row["id"] == peer_id && row["daemon_id"] == PEER_ID),
        "got {rows}"
    );
}

#[tokio::test]
async fn peer_takeover_and_shutdown_end_events_keep_owner_identity() {
    let (_peer_store, _peer_repo, _local_store, _local_repo, peer, local) = fixture().await;
    let mut incumbent = launch(local.port).await;
    let opened = read_until(&mut incumbent, "READY", true).await;
    let id = opened.open.unwrap()["session"].as_u64().unwrap();
    let mut taker = Ws::new(
        page::console(
            local.port,
            serde_json::json!({ "id": id, "repo": peer_repo(), "takeover": 1 }),
        )
        .await,
    );
    let takeover = read_until(&mut incumbent, "session-end", false)
        .await
        .end
        .unwrap();
    assert_eq!(takeover["reason"], "taken-over");
    assert_eq!(takeover["daemon_id"], PEER_ID);
    assert_eq!(takeover["environment"], ENVIRONMENT);
    let taker_open = read_until(&mut taker, "READY", false).await.open.unwrap();
    assert_eq!(taker_open["daemon_id"], PEER_ID);

    peer.shutdown.send(true).unwrap();
    let shutdown = read_until(&mut taker, "session-end", false)
        .await
        .end
        .unwrap();
    assert_eq!(shutdown["reason"], "daemon-shutdown");
    assert_eq!(shutdown["daemon_id"], PEER_ID);
    assert_eq!(shutdown["environment"], ENVIRONMENT);
}
