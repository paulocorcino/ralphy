use super::*;

#[tokio::test]
async fn unreachable_peer_is_an_environment_diagnosis() {
    let local_store = tempfile::tempdir().unwrap();
    let local_repo = tempfile::tempdir().unwrap();
    let registry = local_store.path().join("repos.toml");
    save_registry(&registry, local_repo.path());
    let closed_port = {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        listener.local_addr().unwrap().port()
    };
    peer::write_descriptor(local_store.path(), &descriptor(DEAD_ID, closed_port)).unwrap();
    let local = serve(
        identity(LOCAL_ID, "local"),
        registry,
        AuthState::localhost(),
    )
    .await;

    let repo = format!("{DEAD_ID}/{SLUG}");
    let refusal = page::refused(
        local.port,
        serde_json::json!({ "repo": repo, "agent": "claude" }),
    )
    .await;
    let body = refusal["message"].as_str().unwrap_or_default();
    assert!(body.contains(ENVIRONMENT), "got {body}");
    assert!(!body.contains("serde"), "got {body}");
    assert!(!body.contains("decod"), "got {body}");

    // A reattach is refused too: the channel ends as refused, with no frame.
    let mut reattach =
        page::console(local.port, serde_json::json!({ "id": 7, "repo": repo })).await;
    let first = reattach.next().await;
    assert!(
        matches!(first, Some(Ok(Message::Close(_)))),
        "a refused reattach sends no frame, got {first:?}"
    );
    assert_eq!(reattach.closed_reason(), Some("refused"));
}

#[tokio::test]
async fn incompatible_peer_is_an_environment_diagnosis() {
    let local_store = tempfile::tempdir().unwrap();
    let local_repo = tempfile::tempdir().unwrap();
    let registry = local_store.path().join("repos.toml");
    save_registry(&registry, local_repo.path());
    let mut incompatible = descriptor(DEAD_ID, 1);
    incompatible.protocol_version = PEER_PROTOCOL_VERSION - 1;
    peer::write_descriptor(local_store.path(), &incompatible).unwrap();
    let local = serve(
        identity(LOCAL_ID, "local"),
        registry,
        AuthState::localhost(),
    )
    .await;

    let repo = format!("{DEAD_ID}/{SLUG}");
    let refusal = page::refused(
        local.port,
        serde_json::json!({ "repo": repo, "agent": "claude" }),
    )
    .await;
    let body = refusal["message"].as_str().unwrap_or_default();
    assert!(body.contains(ENVIRONMENT), "got {body}");
    assert!(body.contains("Upgrade the older Ralphy."), "got {body}");
    assert!(
        body.contains(&format!("protocol {}", PEER_PROTOCOL_VERSION - 1)),
        "got {body}"
    );
    assert!(
        body.contains(&format!("not {PEER_PROTOCOL_VERSION}.")),
        "got {body}"
    );
}

#[tokio::test]
async fn an_old_peer_that_refuses_over_http_is_shown_as_a_refusal() {
    // A peer that refuses at the upgrade answers with an HTTP status and a
    // body. This stand-in passes the handshake and refuses its page socket
    // that way.
    const OLD_REFUSAL: &str = "gemini: no owned configuration root in this repo";
    let old_peer = axum::Router::new()
        .route(
            "/api/peer/hello",
            axum::routing::get(|| async {
                axum::Json(serde_json::json!({
                    "daemon_id": DEAD_ID,
                    "protocol_version": PEER_PROTOCOL_VERSION,
                }))
            }),
        )
        .route(
            "/ws",
            axum::routing::get(|| async { (axum::http::StatusCode::BAD_REQUEST, OLD_REFUSAL) }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let old_port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        axum::serve(listener, old_peer).await.unwrap();
    });

    let local_store = tempfile::tempdir().unwrap();
    let local_repo = tempfile::tempdir().unwrap();
    let registry = local_store.path().join("repos.toml");
    save_registry(&registry, local_repo.path());
    peer::write_descriptor(local_store.path(), &descriptor(DEAD_ID, old_port)).unwrap();
    let local = serve(
        identity(LOCAL_ID, "local"),
        registry,
        AuthState::localhost(),
    )
    .await;

    let repo = format!("{DEAD_ID}/{SLUG}");
    let refusal = page::refused(
        local.port,
        serde_json::json!({ "repo": repo, "agent": "gemini" }),
    )
    .await;
    assert_eq!(
        refusal["message"], OLD_REFUSAL,
        "the peer's own words reach the console"
    );
}
