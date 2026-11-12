use super::*;

#[tokio::test]
async fn a_peer_that_ends_a_spawn_before_its_last_frame_is_one_error_reply() {
    // A stand-in peer: its handshake passes, and its page socket answers the
    // open with `channel.closed {reason: refused}` and no frame of the run.
    let opens = Arc::new(std::sync::Mutex::new(Vec::new()));
    let seen = Arc::clone(&opens);
    let stand_in = axum::Router::new()
        .route(
            "/api/peer/hello",
            axum::routing::get(|| async {
                axum::Json(serde_json::json!({
                    "daemon_id": B_ID,
                    "protocol_version": PEER_PROTOCOL_VERSION,
                }))
            }),
        )
        .route(
            "/ws",
            axum::routing::get(move |ws: axum::extract::ws::WebSocketUpgrade| {
                let seen = Arc::clone(&seen);
                async move { ws.on_upgrade(move |socket| refuse_the_open(socket, seen)) }
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let peer_port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        axum::serve(listener, stand_in).await.unwrap();
    });

    let local_store = tempfile::tempdir().unwrap();
    let local_repo = tempfile::tempdir().unwrap();
    let local_registry = local_store.path().join("repos.toml");
    save_registry(&local_registry, local_repo.path());
    peer::write_descriptor(local_store.path(), &descriptor(B_ID, peer_port)).unwrap();
    let (local_port, _local_task) = serve(
        identity(A_ID, "local"),
        local_registry,
        AuthState::localhost(),
    )
    .await;

    let payload = serde_json::json!({ "repo": format!("{B_ID}/{SLUG}") });
    let replies = ask_until_the_end(local_port, 30, "triage", payload).await;
    assert_eq!(replies.len(), 1, "exactly one reply: {replies:?}");
    assert_eq!(replies[0].payload["status"], "error", "got {replies:?}");
    let message = replies[0].payload["message"].as_str().unwrap_or_default();
    assert!(message.contains(ENV), "the error names the peer: {message}");

    let opens = opens.lock().unwrap().clone();
    assert_eq!(opens.len(), 1, "the peer got one open: {opens:?}");
    assert_eq!(opens[0]["kind"], "spawn", "got {opens:?}");
    assert_eq!(opens[0]["command"]["verb"], "triage", "got {opens:?}");
    assert_eq!(
        opens[0]["command"]["payload"]["repo"], SLUG,
        "the peer gets its own slug: {opens:?}"
    );
}

/// Read the first frame of `socket` as a `channel.open`, keep its payload in
/// `seen`, and end that channel as refused.
async fn refuse_the_open(
    mut socket: axum::extract::ws::WebSocket,
    seen: Arc<std::sync::Mutex<Vec<serde_json::Value>>>,
) {
    use axum::extract::ws::Message as AxumMessage;
    let Some(Ok(AxumMessage::Binary(bytes))) = socket.recv().await else {
        return;
    };
    let Ok(Frame::Command(open)) = protocol::decode(&bytes) else {
        return;
    };
    assert_eq!(open.verb, "channel.open", "the first frame opens a channel");
    let channel = open.payload["channel"].clone();
    seen.lock()
        .expect("the stand-in peer's record of opens")
        .push(open.payload);
    let closed = Frame::Command(Command {
        id: 0,
        verb: "channel.closed".to_owned(),
        payload: serde_json::json!({ "channel": channel, "reason": "refused" }),
    });
    socket
        .send(AxumMessage::Binary(protocol::encode(&closed).into()))
        .await
        .expect("the stand-in peer sends channel.closed");
    // Hold the socket until the local daemon closes it.
    while let Some(Ok(_)) = socket.recv().await {}
}
