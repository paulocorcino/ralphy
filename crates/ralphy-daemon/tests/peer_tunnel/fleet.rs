//! The fleet row, the nudge and the wake of a tunnel peer, and a command relayed to one.

use super::*;

#[tokio::test]
async fn fleet_reports_the_two_tunnel_states() {
    let _serial = SERIAL.lock().await;
    let fx = setup(0);
    let store = tempfile::tempdir().unwrap();
    let port = closed_port();
    let mut tunnel = descriptor(TUNNEL_ID, port, Some(spec(port)));
    tunnel.name = "svrapp".to_string();
    peer::write_descriptor(store.path(), &tunnel).unwrap();
    let own = ralphy_daemon::DEFAULT_PORT;
    peer::write_descriptor(
        store.path(),
        &descriptor(COLLIDING_ID, own, Some(spec(own))),
    )
    .unwrap();
    let local = app(LOCAL_ID, store.path(), AuthState::localhost());

    let (status, fleet) = call(&local, "GET", "/api/fleet").await;
    assert_eq!(status, StatusCode::OK);
    let row = peer_row(&fleet, TUNNEL_ID);
    assert_eq!(row["state"], "tunnel-closed", "got: {row}");
    assert_eq!(row["tunnel"], true);
    assert_eq!(
        row["destination"], "svrapp",
        "the form of Edit needs it: {row}"
    );
    assert!(
        !row["diagnosis"].as_str().unwrap().contains("WSL"),
        "got: {row}"
    );
    let colliding = peer_row(&fleet, COLLIDING_ID);
    assert_eq!(colliding["state"], "refused", "got: {colliding}");
    assert!(
        colliding["diagnosis"]
            .as_str()
            .unwrap()
            .contains("the port of this daemon"),
        "got: {colliding}"
    );
    wait_lines(&fx.argv_log, 1);
    std::thread::sleep(Duration::from_millis(500));
    let own_forward = format!("127.0.0.1:{own}:");
    assert!(
        !lines(&fx.argv_log).iter().any(|l| l.contains(&own_forward)),
        "no ssh for the colliding peer"
    );

    wait_dead(tunnels(), TUNNEL_ID);
    set_sleep(60_000);
    let (_, fleet) = call(&local, "GET", "/api/fleet").await;
    let row = peer_row(&fleet, TUNNEL_ID);
    assert_eq!(
        row["state"], "tunnel-closed",
        "the dead ssh was replaced: {row}"
    );
    // The stand-in wrote this on its stderr before it exited.
    assert!(
        row["diagnosis"]
            .as_str()
            .unwrap()
            .contains("dispatch-stderr-marker"),
        "the closed tunnel says what ssh said: {row}"
    );
    let (_, fleet) = call(&local, "GET", "/api/fleet").await;
    let row = peer_row(&fleet, TUNNEL_ID);
    assert_eq!(row["state"], "tunnel-silent", "got: {row}");
    let diagnosis = row["diagnosis"].as_str().unwrap();
    assert!(diagnosis.contains("does not answer"), "got: {diagnosis}");
    assert!(!diagnosis.contains("WSL"), "got: {diagnosis}");
}

/// Serve the peer daemon `id` on a loopback port. The store must outlive the
/// server.
async fn serve_peer(id: &str) -> (u16, tokio::task::JoinHandle<()>, tempfile::TempDir) {
    let peer_store = tempfile::tempdir().expect("create the temp dir");
    let peer_app = app(
        id,
        peer_store.path(),
        AuthState::fixed(
            AuthPolicy::Bearer("peer-token".to_string()),
            SessionEpoch::in_memory_detached(),
        ),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind a local port");
    let port = listener
        .local_addr()
        .expect("read the bound address")
        .port();
    let serving = tokio::spawn(async move {
        axum::serve(listener, peer_app)
            .await
            .expect("serve the test app");
    });
    (port, serving, peer_store)
}

#[tokio::test]
async fn a_nudge_on_a_tunnel_peer_opens_its_tunnel() {
    let _serial = SERIAL.lock().await;
    let fx = setup(60_000);
    let (port, serving, _peer_store) = serve_peer(NUDGE_ID).await;
    let store = tempfile::tempdir().unwrap();
    peer::write_descriptor(store.path(), &descriptor(NUDGE_ID, port, Some(spec(port)))).unwrap();
    let local = app(LOCAL_ID, store.path(), AuthState::localhost());
    let (status, body) = call(
        &local,
        "POST",
        &format!("/api/fleet/nudge?daemon_id={NUDGE_ID}"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "got: {body}");
    assert_eq!(body["ready"], true, "got: {body}");
    wait_lines(&fx.argv_log, 1);
    std::thread::sleep(Duration::from_millis(500));
    let got = lines(&fx.argv_log);
    assert_eq!(got.len(), 1, "got: {got:?}");
    serving.abort();
}

const WAKE_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FB6";

/// A tunnel that waits after its exits: the operator's wake starts the `ssh`
/// at once, even when the wake is what sees the last exit, and the peer's
/// answer to the wake's probe resets the count.
#[tokio::test]
async fn a_wake_and_an_answer_reopen_a_waiting_tunnel() {
    let _serial = SERIAL.lock().await;
    let fx = setup(0);
    let (port, serving, _peer_store) = serve_peer(WAKE_ID).await;
    let store = tempfile::tempdir().unwrap();
    peer::write_descriptor(store.path(), &descriptor(WAKE_ID, port, Some(spec(port)))).unwrap();
    for _ in 0..2 {
        assert!(tunnels().ensure(WAKE_ID, &spec(port)).unwrap());
        wait_dead(tunnels(), WAKE_ID);
    }
    let local = app(LOCAL_ID, store.path(), AuthState::localhost());
    let uri = format!("/api/fleet/nudge?daemon_id={WAKE_ID}");
    let (status, body) = call(&local, "POST", &uri).await;
    assert_eq!(status, StatusCode::OK, "got: {body}");
    assert_eq!(
        wait_lines(&fx.argv_log, 3).len(),
        3,
        "the wake started an ssh"
    );
    wait_dead(tunnels(), WAKE_ID);
    assert!(
        tunnels().ensure(WAKE_ID, &spec(port)).unwrap(),
        "the answer to the wake's probe reset the count"
    );
    serving.abort();
}

const SILENT_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FB5";

/// A repo command relayed to a tunnel peer whose `ssh` runs but whose daemon
/// does not answer gets the sentence `/api/fleet` gives the same peer.
#[tokio::test]
async fn a_relayed_command_names_a_silent_tunnel_as_the_fleet_does() {
    use ralphy_daemon::protocol::{self, Command, Frame};
    use tokio_tungstenite::tungstenite::Message;

    let _serial = SERIAL.lock().await;
    let _fx = setup(60_000);
    let store = tempfile::tempdir().unwrap();
    let port = closed_port();
    peer::write_descriptor(store.path(), &descriptor(SILENT_ID, port, Some(spec(port)))).unwrap();
    assert!(tunnels().ensure(SILENT_ID, &spec(port)).unwrap());

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let local_port = listener.local_addr().unwrap().port();
    let local = app(LOCAL_ID, store.path(), AuthState::localhost());
    let serving = tokio::spawn(async move {
        axum::serve(listener, local).await.unwrap();
    });

    let mut ws = page::command(local_port).await;
    ws.send(Message::Binary(
        protocol::encode(&Frame::Command(Command {
            id: 1,
            verb: "tree.list".to_string(),
            payload: serde_json::json!({ "repo": format!("{SILENT_ID}/owner/repo"), "path": "" }),
        }))
        .into(),
    ))
    .await
    .unwrap();
    let reply = tokio::time::timeout(Duration::from_secs(10), async {
        while let Some(message) = ws.next().await {
            if let Message::Binary(bytes) = message.unwrap() {
                if let Ok(Frame::Command(reply)) = protocol::decode(&bytes) {
                    if reply.id == 1 {
                        return reply.payload;
                    }
                }
            }
        }
        panic!("command socket closed without a reply");
    })
    .await
    .expect("command reply timed out");

    assert_eq!(reply["status"], "error", "got: {reply}");
    let message = reply["message"].as_str().unwrap();
    assert!(
        message.starts_with("The tunnel to svrapp is open, but its daemon does not answer"),
        "got: {message}"
    );
    serving.abort();
}

#[tokio::test]
async fn a_nudge_never_opens_a_tunnel_on_our_own_port() {
    let _serial = SERIAL.lock().await;
    let fx = setup(0);
    let store = tempfile::tempdir().unwrap();
    let own = ralphy_daemon::DEFAULT_PORT;
    peer::write_descriptor(
        store.path(),
        &descriptor(COLLIDING_ID, own, Some(spec(own))),
    )
    .unwrap();
    let local = app(LOCAL_ID, store.path(), AuthState::localhost());
    let (status, body) = call(
        &local,
        "POST",
        &format!("/api/fleet/nudge?daemon_id={COLLIDING_ID}"),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "got: {body}");
    assert!(
        body["error"]
            .as_str()
            .unwrap_or_default()
            .contains("the port of this daemon"),
        "got: {body}"
    );
    std::thread::sleep(Duration::from_millis(500));
    assert!(lines(&fx.argv_log).is_empty(), "no ssh was started");
}
