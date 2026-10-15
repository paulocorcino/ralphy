//! `fleet_route` when a peer answers its handshake but not with a repo list
//! this daemon can use.

use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;

use axum::http::StatusCode;
use axum::response::IntoResponse;
use http_body_util::BodyExt;

use super::*;

/// What the stub peer serves on `/api/repos`.
const GOOD: u8 = 0;
const REFUSED: u8 = 1;
const UNREADABLE: u8 = 2;

/// A peer that always answers its handshake and serves `/api/repos` as
/// `mode` says. The refusal carries a valid list of another repo, so a
/// daemon that reads a refused body would show that repo.
async fn serve_peer(mode: Arc<AtomicU8>) -> (u16, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let hello = || async {
        Json(serde_json::json!({
            "protocol_version": peer::PEER_PROTOCOL_VERSION,
            "environment": "WSL: Debian-12",
        }))
    };
    let repos = move || {
        let mode = mode.load(Ordering::SeqCst);
        async move {
            let row = |slug: &str| serde_json::json!([{ "slug": slug, "path": "/home/p/x", "reachable": true }]);
            match mode {
                GOOD => Json(row("owner/theirs")).into_response(),
                REFUSED => {
                    (StatusCode::SERVICE_UNAVAILABLE, Json(row("owner/refused"))).into_response()
                }
                _ => "not a repo list".into_response(),
            }
        }
    };
    let stub = Router::new()
        .route("/api/peer/hello", get(hello))
        .route("/api/repos", get(repos));
    let serving = tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, stub).await {
            panic!("the stub peer stopped: {e}");
        }
    });
    (port, serving)
}

fn descriptor(port: u16) -> peer::PeerDescriptor {
    peer::PeerDescriptor {
        daemon_id: "01PEERFAKE".into(),
        name: "wsl-box".into(),
        avatar: "🐺".into(),
        address: "127.0.0.1".into(),
        port,
        environment: "WSL: Ubuntu-22.04".into(),
        os: String::new(),
        token: "tok".into(),
        protocol_version: peer::PEER_PROTOCOL_VERSION,
        tunnel: None,
        nudge: None,
    }
}

/// The fleet body, read from `fleet_route` with `cache` as its memory.
async fn fleet(dir: &std::path::Path, cache: &PeerRepoCache) -> serde_json::Value {
    let resp = fleet_route(
        dir.join("repos.toml"),
        dir.join("peers"),
        Some(identity::Identity {
            id: ulid::Ulid::nil(),
            name: "anvil".into(),
            avatar: "🐙".into(),
        }),
        "Windows".into(),
        cache.clone(),
        1,
    )
    .await;
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&body).unwrap()
}

/// The slugs of the peer rows in a fleet body.
fn peer_slugs(body: &serde_json::Value) -> Vec<String> {
    body["repos"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["local"] == false)
        .map(|r| r["slug"].as_str().unwrap().to_string())
        .collect()
}

/// A reachable peer that answers `/api/repos` with `mode` after one good
/// answer: the fleet keeps the good answer and the peer stays reachable.
async fn keeps_the_last_good_repos_when_the_peer_serves(mode: u8) {
    let dir = tempfile::tempdir().unwrap();
    let served = Arc::new(AtomicU8::new(GOOD));
    let (port, serving) = serve_peer(served.clone()).await;
    peer::write_descriptor(dir.path(), &descriptor(port)).unwrap();
    let cache = PeerRepoCache::default();

    let first = fleet(dir.path(), &cache).await;
    assert_eq!(peer_slugs(&first), ["owner/theirs"], "got: {first}");

    served.store(mode, Ordering::SeqCst);
    let after = fleet(dir.path(), &cache).await;
    serving.abort();
    assert_eq!(after["peers"][0]["state"], "reachable", "got: {after}");
    assert_eq!(
        peer_slugs(&after),
        ["owner/theirs"],
        "the last good repo list stays, and nothing from this answer is read: {after}"
    );
}

#[tokio::test]
async fn a_peer_that_refuses_its_repos_keeps_its_last_known_repos() {
    keeps_the_last_good_repos_when_the_peer_serves(REFUSED).await;
}

#[tokio::test]
async fn a_peer_that_serves_an_unreadable_repo_list_keeps_its_last_known_repos() {
    keeps_the_last_good_repos_when_the_peer_serves(UNREADABLE).await;
}
