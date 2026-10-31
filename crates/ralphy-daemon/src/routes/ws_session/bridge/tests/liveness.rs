use super::*;

/// A live console whose output stream is the test's own and stays empty, so
/// the bridge sends the client nothing but its opening frames and its pings.
/// The sender is returned so that the stream stays open.
fn quiet_session() -> (
    Arc<session::SessionManager>,
    session::SessionId,
    session::Attachment,
    tokio::sync::broadcast::Sender<Vec<u8>>,
) {
    let manager = Arc::new(session::SessionManager::new());
    let spec = session::console_spec(std::env::temp_dir(), 24, 80, None);
    let (id, mut attach) = manager
        .spawn_attached(
            "~".to_string(),
            "console".to_string(),
            "console".to_string(),
            None,
            None,
            None,
            spec,
        )
        .expect("the platform shell must spawn — the free console depends on it");
    let (tx, rx) = tokio::sync::broadcast::channel::<Vec<u8>>(4);
    attach.rx = rx;
    attach.snapshot.clear();
    (manager, id, attach, tx)
}

/// A client that sends nothing after the handshake, not even the pong a
/// browser sends for each ping, is released 45 s after it last spoke, not at
/// the first ping tick past 45 s (60 s with 20 s pings). The client never
/// reads, so it answers no ping. Time is paused, so the 45 s pass in test
/// time.
#[tokio::test(start_paused = true)]
async fn a_silent_client_is_released_45_s_after_it_last_spoke() {
    let (manager, id, attach, _output) = quiet_session();
    let slot = Arc::new(Mutex::new(Some(attach)));
    let (ended_tx, ended) = tokio::sync::oneshot::channel();
    let ended_tx = Arc::new(Mutex::new(Some(ended_tx)));
    let (_stop, shutdown) = tokio::sync::watch::channel(false);
    let app = axum::Router::new().route(
        "/",
        axum::routing::get(move |ws: WebSocketUpgrade| {
            let attach = slot.lock().expect("slot mutex").take();
            let ended_tx = ended_tx.lock().expect("end mutex").take();
            let shutdown = shutdown.clone();
            async move {
                let attach = attach.expect("one client connects");
                let ended_tx = ended_tx.expect("one client connects");
                ws.on_upgrade(move |socket| async move {
                    let labels = SessionLabels::default();
                    let env = "test".to_string();
                    let daemon = "daemon".to_string();
                    let tab = Tab::default();
                    session_ws(socket, attach, id, daemon, env, labels, tab, shutdown).await;
                    ended_tx
                        .send(tokio::time::Instant::now())
                        .expect("the test waits for the end");
                })
            }
        }),
    );
    let (server_io, client_io) = tokio::io::duplex(1 << 20);
    let server =
        tokio::spawn(async move { axum::serve(OneConnection(Some(server_io)), app).await });
    let (client, _) = tokio_tungstenite::client_async("ws://localhost/", client_io)
        .await
        .expect("the bridge upgrades the connection");
    // The handshake was the last thing the client sent.
    let spoke = tokio::time::Instant::now();

    let ended = tokio::time::timeout(Duration::from_secs(120), ended)
        .await
        .expect("the bridge ends within 120 s")
        .expect("the bridge reports its end");
    let silent = ended - spoke;
    server.abort();
    drop(client);
    assert!(manager.close(id), "the session was live");
    assert!(
        silent >= Duration::from_secs(45) && silent < Duration::from_secs(46),
        "the bridge released the client after {silent:?} of silence"
    );
}
