//! The peer command route and its body cap.

use super::*;

/// POST `body` to `/api/peer/command` on a daemon with one registered repo,
/// `owner/cap`; return the status.
async fn peer_command_status(body: Vec<u8>) -> StatusCode {
    let dir = tempfile::tempdir().unwrap();
    let registry_path = dir.path().join("repos.toml");
    let mut store = registry::RegistryStore::default();
    store.upsert("owner/cap", &dir.path().to_string_lossy());
    registry::save_to(&store, &registry_path).unwrap();
    let resp = fleet_router(registry_path)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/peer/command")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    resp.status()
}

/// A paste forwarded to a peer carries a whole image: a 4 MiB `image.write`
/// is under the transport cap, so the peer route takes it.
#[tokio::test]
async fn peer_command_takes_a_4_mib_image() {
    let mut png = b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR".to_vec();
    png.resize(tree::MAX_IMAGE_BYTES as usize, 0);
    let body = serde_json::json!({
        "id": 1,
        "verb": "image.write",
        "payload": {"repo": "owner/cap", "base64": data_encoding::BASE64.encode(&png)},
    });
    let status = peer_command_status(body.to_string().into_bytes()).await;
    assert_ne!(status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(status, StatusCode::OK);
}

/// A body over the transport cap is refused before it is parsed.
#[tokio::test]
async fn peer_command_refuses_a_body_over_the_cap() {
    let body = vec![b' '; tree::MAX_COMMAND_BYTES + 1];
    assert_eq!(
        peer_command_status(body).await,
        StatusCode::PAYLOAD_TOO_LARGE
    );
}
