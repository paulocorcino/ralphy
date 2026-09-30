//! A daemon with the `daemon-require-token` marker asks for its access token
//! also on 127.0.0.1, because an SSH tunnel from another computer arrives there
//! (docs/adr/0067 §5). Driven through `AuthState::boot` — the path `serve.rs`
//! takes — so the marker is read from the store exactly as at start-up.
//!
//! SOLE env-setter in its file: `RALPHY_DAEMON_DIR` is process-global, so this
//! env-setting test must be alone in its file (no intra-process race).

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use http_body_util::BodyExt;
use ralphy_daemon::auth::{self, AuthState};
use ralphy_daemon::epoch::SessionEpoch;
use ralphy_daemon::identity::Identity;
use ralphy_daemon::peer::{self, PeerDescriptor, PEER_PROTOCOL_VERSION};
use ralphy_daemon::{registry, router};
use tower::ServiceExt;

const TOKEN: &str = "peer-tok";
const LOOPBACK: &str = "127.0.0.1:7257";
const LOCAL_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
const PEER_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAW";

fn boot() -> anyhow::Result<Arc<AuthState>> {
    let bound: SocketAddr = LOOPBACK.parse().expect("a valid loopback address");
    AuthState::boot(bound, None, SessionEpoch::in_memory_detached(), &[])
}

fn app(id: &str, registry_path: PathBuf, auth: Arc<AuthState>) -> axum::Router {
    let (tx, rx) = tokio::sync::watch::channel(false);
    std::mem::forget(tx);
    router(
        Some(Identity {
            id: id.parse().expect("a valid ULID"),
            name: id.to_string(),
            avatar: "🐙".to_string(),
        }),
        registry_path,
        PathBuf::from("does-not-exist"),
        ralphy_daemon::StorePaths::default(),
        Instant::now(),
        rx,
        auth,
    )
}

async fn get(
    app: axum::Router,
    uri: &str,
    bearer: Option<&str>,
) -> (StatusCode, serde_json::Value) {
    let mut req = Request::builder().uri(uri);
    if let Some(token) = bearer {
        req = req.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    let resp = app
        .oneshot(req.body(Body::empty()).expect("a well-formed test request"))
        .await
        .expect("the router must answer");
    let status = resp.status();
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json = serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null);
    (status, json)
}

#[tokio::test]
async fn the_marker_requires_the_token_on_loopback_and_the_fleet_still_reaches_the_peer() {
    let peer_store = tempfile::tempdir().expect("a temp peer store");
    let peer_dir = peer_store.path();
    // Sole test in this file → no intra-process env race.
    std::env::set_var("RALPHY_DAEMON_DIR", peer_dir);

    // (1) Marker set, no token anywhere: the daemon refuses to start.
    auth::set_require_token_in(peer_dir, true).unwrap();
    let Err(err) = boot() else {
        panic!("a marked loopback daemon with no token must refuse to start");
    };
    assert!(
        format!("{err:#}").contains("ralphy daemon require-token on"),
        "the refusal names the fix: {err:#}"
    );

    // (2) With a token on disk: loopback asks for it.
    auth::save_token_to(TOKEN, &auth::token_path_in(peer_dir)).unwrap();
    let peer_auth = boot().expect("a marked daemon with a token boots");
    assert_eq!(peer_auth.policy().name(), "bearer");

    let registry_path = peer_dir.join("repos.toml");
    let mut store = registry::RegistryStore::default();
    store.upsert("owner/theirs", &peer_dir.to_string_lossy());
    registry::save_to(&store, &registry_path).unwrap();
    let peer_app = app(PEER_ID, registry_path, peer_auth.clone());

    let (status, _) = get(peer_app.clone(), "/api/repos", None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "no token on loopback");
    let (status, _) = get(peer_app.clone(), "/api/repos", Some(TOKEN)).await;
    assert_eq!(status, StatusCode::OK, "the right token on loopback");
    let (_, session) = get(peer_app.clone(), "/api/session", Some(TOKEN)).await;
    assert_eq!(session["policy"], "bearer", "got: {session}");

    // (3) The same daemon, served, is a reachable peer of a local daemon.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let serving = tokio::spawn(async move {
        axum::serve(listener, peer_app).await.unwrap();
    });

    let local_store = tempfile::tempdir().expect("a temp local store");
    let local_dir = local_store.path();
    let local_registry = local_dir.join("repos.toml");
    let mut store = registry::RegistryStore::default();
    store.upsert("owner/local", &local_dir.to_string_lossy());
    registry::save_to(&store, &local_registry).unwrap();
    peer::write_descriptor(
        local_dir,
        &PeerDescriptor {
            daemon_id: PEER_ID.to_string(),
            name: "peer".to_string(),
            avatar: "🐙".to_string(),
            address: "127.0.0.1".to_string(),
            port,
            environment: "vps".to_string(),
            token: TOKEN.to_string(),
            protocol_version: PEER_PROTOCOL_VERSION,
            tunnel: None,
            nudge: None,
        },
    )
    .unwrap();
    let local_app = app(LOCAL_ID, local_registry, AuthState::localhost());
    let (status, fleet) = get(local_app, "/api/fleet", None).await;
    assert_eq!(status, StatusCode::OK, "got: {fleet}");
    assert_eq!(fleet["peers"][0]["state"], "reachable", "got: {fleet}");
    let theirs: Vec<&serde_json::Value> = fleet["repos"]
        .as_array()
        .expect("the fleet lists repos")
        .iter()
        .filter(|r| r["slug"] == "owner/theirs" && r["local"] == false)
        .collect();
    assert_eq!(theirs.len(), 1, "the peer's repo is listed: {fleet}");
    serving.abort();

    // (4) Marker cleared: loopback is open again.
    auth::set_require_token_in(peer_dir, false).unwrap();
    let open = boot().expect("an unmarked loopback daemon boots");
    assert_eq!(open.policy().name(), "localhost");
}
