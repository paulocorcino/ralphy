//! `/api/fleet` and the nudge that wakes a peer.

use super::*;
use axum::Json;

/// Seed a scratch store with `repos.toml` plus a `peers/` descriptor pointing
/// at a closed loopback port, and return the registry path `router` takes.
fn seed_fleet_store(dir: &Path, peer_port: u16) -> PathBuf {
    let registry_path = dir.join("repos.toml");
    let mut store = registry::RegistryStore::default();
    store.upsert("owner/local", &dir.to_string_lossy());
    registry::save_to(&store, &registry_path).unwrap();
    peer::write_descriptor(
        dir,
        &peer::PeerDescriptor {
            daemon_id: "01PEERFAKE".into(),
            name: "wsl-box".into(),
            avatar: "🐺".into(),
            address: "127.0.0.1".into(),
            port: peer_port,
            environment: "WSL: Ubuntu-22.04".into(),
            os: String::new(),
            token: "tok".into(),
            protocol_version: peer::PEER_PROTOCOL_VERSION,
            tunnel: None,
            // Deliberately NOT a WSL peer: this descriptor points at a closed
            // loopback port, and announcing a distro it does not have would
            // make a failed dial ask the HOST whether that distro is running —
            // which is a real question with a real answer, so the state these
            // tests assert would then vary with the machine running them.
            nudge: None,
        },
    )
    .unwrap();
    registry_path
}

/// ADR-0070 D4: the fleet view shows a peer store it cannot read as a row,
/// in the existing `malformed` state, never as an empty fleet.
#[tokio::test]
async fn api_fleet_shows_an_unreadable_peer_store_as_a_row() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("peers"), "a file, not a directory").unwrap();
    let resp = fleet_router(dir.path().join("repos.toml"))
        .oneshot(
            Request::builder()
                .uri("/api/fleet")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let peers = body["peers"].as_array().unwrap();
    assert_eq!(peers.len(), 1, "the failure is a row: {peers:?}");
    assert_eq!(peers[0]["state"], "malformed");
}

#[tokio::test]
async fn api_fleet_marks_an_unreachable_peer_and_keeps_the_local_repos() {
    let dir = tempfile::tempdir().unwrap();
    // A port nothing listens on: bound to learn a free one, then dropped.
    let closed = {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        l.local_addr().unwrap().port()
    };
    let registry_path = seed_fleet_store(dir.path(), closed);
    // A second peer, this one announcing a distro, so the view's `nudgeable`
    // has something to be true about.
    peer::write_descriptor(
        dir.path(),
        &peer::PeerDescriptor {
            daemon_id: "01PEERWSL".into(),
            name: "wsl-box".into(),
            avatar: "🐺".into(),
            address: "127.0.0.1".into(),
            port: closed,
            environment: "WSL: Ubuntu-22.04".into(),
            os: "linux".into(),
            token: "tok".into(),
            protocol_version: peer::PEER_PROTOCOL_VERSION,
            tunnel: None,
            nudge: Some(peer::NudgeSpec {
                distro: "Ubuntu-22.04".into(),
                unit: "ralphy-daemon.service".into(),
            }),
        },
    )
    .unwrap();
    // A file that is not a descriptor at all — it must degrade to one entry,
    // never fail the route.
    std::fs::write(
        dir.path().join("peers").join("junk.toml"),
        "not toml at all",
    )
    .unwrap();

    let resp = fleet_router(registry_path)
        .oneshot(
            Request::builder()
                .uri("/api/fleet")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let body: serde_json::Value = serde_json::from_slice(&body).unwrap();

    let peers = body["peers"].as_array().unwrap();
    assert_eq!(peers.len(), 3, "both peers AND the bad record: {peers:?}");
    let wsl = peers
        .iter()
        .find(|p| p["daemon_id"] == "01PEERWSL")
        .expect("the WSL peer must be listed");
    assert_eq!(
        wsl["os"], "linux",
        "the OS family rides through for the icon: {wsl}"
    );
    let live = peers
        .iter()
        .find(|p| p["daemon_id"] == "01PEERFAKE")
        .expect("the announced peer must be listed");
    assert_eq!(live["state"], "unreachable", "got: {live}");
    assert_eq!(live["environment"], "WSL: Ubuntu-22.04");
    assert!(
        live["diagnosis"]
            .as_str()
            .unwrap()
            .contains("WSL: Ubuntu-22.04"),
        "the diagnosis must name the environment; got: {live}"
    );
    assert!(
        !live["nudgeable"].as_bool().unwrap(),
        "a peer that announced no distro cannot be woken: {live}"
    );
    // The nudgeable surface, on the one peer that does advertise a distro. Its
    // STATE is host-dependent by design — a Windows host with WSL reads the
    // distro as stopped and says `asleep`, a host that cannot be asked degrades
    // to `unreachable` — so only the flag is asserted here; which unreachable
    // is which is pinned deterministically in `peer::tests`.
    let wsl = peers
        .iter()
        .find(|p| p["daemon_id"] == "01PEERWSL")
        .expect("the WSL peer must be listed");
    assert!(
        wsl["nudgeable"].as_bool().unwrap(),
        "an announced distro is what makes a peer nudgeable: {wsl}"
    );
    assert_ne!(wsl["state"], "reachable", "got: {wsl}");
    let bad = peers
        .iter()
        .find(|p| p["state"] == "malformed")
        .expect("a fold rejection is degraded, never dropped");
    assert_eq!(bad["name"], "junk.toml");

    // Federation must never blank the local sidebar.
    let repos = body["repos"].as_array().unwrap();
    assert_eq!(repos.len(), 1, "the local row survives: {repos:?}");
    assert_eq!(repos[0]["key"], "00000000000000000000000000/owner/local");
    assert_eq!(repos[0]["peer_state"], "local");
    assert_eq!(repos[0]["local"], true);
}

/// The route-level proof of "marked, never removed": a peer that answered
/// once keeps its rows listed after it stops answering, with its state
/// changed rather than its rows dropped. Liveness is still fresh — only the
/// repo list is remembered. The environment label is the one the peer's
/// handshake gives while it answers, and the last one it gave after it stops.
#[tokio::test]
async fn api_fleet_keeps_a_peers_last_known_repos_after_it_stops_answering() {
    let dir = tempfile::tempdir().unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let stub = Router::new()
        .route(
            "/api/peer/hello",
            axum::routing::get(|| async {
                Json(serde_json::json!({
                    "protocol_version": peer::PEER_PROTOCOL_VERSION,
                    "environment": "WSL: Debian-12",
                }))
            }),
        )
        .route(
            "/api/repos",
            axum::routing::get(|| async {
                // The shape a real peer serves: its own verdict on its own
                // path travels with the row, because this daemon may not
                // stat it (ADR-0052 §1).
                Json(serde_json::json!([{
                    "slug": "owner/theirs",
                    "path": "/home/p/theirs",
                    "reachable": true,
                    "branch": "main",
                }]))
            }),
        );
    let serving = tokio::spawn(async move {
        let _ = axum::serve(listener, stub).await;
    });

    let registry_path = seed_fleet_store(dir.path(), port);
    let app = fleet_router(registry_path);

    let fleet = |app: Router| async move {
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/fleet")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        serde_json::from_slice::<serde_json::Value>(&body).unwrap()
    };

    let up = fleet(app.clone()).await;
    assert_eq!(up["peers"][0]["state"], "reachable", "got: {up}");
    let up_rows: Vec<&serde_json::Value> = up["repos"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["local"] == false)
        .collect();
    assert_eq!(up_rows.len(), 1, "the peer's repo is federated: {up}");
    assert_eq!(up_rows[0]["slug"], "owner/theirs");
    assert_eq!(
        up["peers"][0]["environment"], "WSL: Debian-12",
        "a reachable peer is labelled by its handshake, not its descriptor: {up}"
    );
    assert_eq!(up_rows[0]["environment"], "WSL: Debian-12");

    serving.abort();
    let _ = serving.await;

    let down = fleet(app.clone()).await;
    assert_eq!(
        down["peers"][0]["state"], "unreachable",
        "liveness is re-computed, never cached: {down}"
    );
    let down_rows: Vec<&serde_json::Value> = down["repos"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["local"] == false)
        .collect();
    assert_eq!(
        down_rows.len(),
        1,
        "the peer is MARKED, not removed — its last-known repos stay listed: {down}"
    );
    assert_eq!(down_rows[0]["peer_state"], "unreachable");
    assert_eq!(down_rows[0]["reachable"], false);
    assert_eq!(
        down["peers"][0]["environment"], "WSL: Debian-12",
        "a peer that stops answering keeps the label its last handshake gave: {down}"
    );
    assert_eq!(down_rows[0]["environment"], "WSL: Debian-12");
}

fn nudge_target(port: u16, address: &str) -> peer::PeerDescriptor {
    peer::PeerDescriptor {
        daemon_id: "01PEERWSL".into(),
        name: "wsl-box".into(),
        avatar: "🐺".into(),
        address: address.into(),
        port,
        environment: "WSL: Ubuntu-22.04".into(),
        os: String::new(),
        token: "tok".into(),
        protocol_version: peer::PEER_PROTOCOL_VERSION,
        tunnel: None,
        nudge: None,
    }
}

/// `nudged` used to mean "spawned `wsl.exe`", which is true a beat after the
/// request and useless: the peer is still booting, and the caller's next act
/// gets a 502. It must mean "answering".
#[tokio::test]
async fn a_nudge_reports_ready_only_once_the_peer_answers() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    // Bound but unserved for now: the peer exists and refuses, exactly as a
    // distro that has started while its daemon has not.
    let late = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(300)).await;
        let stub = Router::new().route(
            "/api/peer/hello",
            axum::routing::get(|| async {
                Json(serde_json::json!({
                    "daemon_id": "01PEERWSL",
                    "protocol_version": peer::PEER_PROTOCOL_VERSION,
                }))
            }),
        );
        let _ = axum::serve(listener, stub).await;
    });

    let d = nudge_target(port, "127.0.0.1");
    let (ready, waited, status) = nudge_await_ready(
        &d,
        Some("00000000000000000000000000"),
        1,
        Duration::from_secs(10),
    )
    .await;
    late.abort();
    assert!(
        ready,
        "the peer came up and must be reported ready: {status:?}"
    );
    assert!(
        waited >= Duration::from_millis(250),
        "it answered in {waited:?} — too fast to prove anything was waited for"
    );
}

/// The give-up path: a peer that never comes back must end the wait with a
/// verdict and the reason, not hang on the operator.
#[tokio::test]
async fn a_nudge_that_never_answers_gives_up_with_a_diagnosis() {
    let closed = {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        l.local_addr().unwrap().port()
    };
    let d = nudge_target(closed, "127.0.0.1");
    let (ready, _, status) = nudge_await_ready(
        &d,
        Some("00000000000000000000000000"),
        1,
        Duration::from_millis(200),
    )
    .await;
    assert!(!ready);
    assert_eq!(status.state(), "unreachable");
    assert!(
        status
            .diagnosis(&d.environment)
            .contains("WSL: Ubuntu-22.04"),
        "the give-up must still name the environment"
    );
}

/// A refusal is a verdict about the DESCRIPTOR, not a boot in progress. Waiting
/// on one burns the whole deadline to reach the answer it already had.
#[tokio::test]
async fn a_nudge_does_not_wait_out_a_refusal() {
    let d = nudge_target(7257, "10.0.0.5");
    let started = Instant::now();
    let (ready, _, status) = nudge_await_ready(
        &d,
        Some("00000000000000000000000000"),
        1,
        Duration::from_secs(30),
    )
    .await;
    assert!(!ready);
    assert_eq!(status.state(), "refused");
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "a refusal was waited on for {:?} — it can never become reachable",
        started.elapsed()
    );
}

#[tokio::test]
async fn api_fleet_nudge_404s_an_unknown_daemon() {
    let dir = tempfile::tempdir().unwrap();
    let registry_path = seed_fleet_store(dir.path(), 7257);
    let resp = fleet_router(registry_path)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/fleet/nudge?daemon_id=01NOSUCHPEER")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn api_fleet_nudge_refuses_a_peer_that_announced_no_way_to_wake_it() {
    let dir = tempfile::tempdir().unwrap();
    let registry_path = seed_fleet_store(dir.path(), 7257);
    // Re-announce the same peer WITHOUT a nudge spec.
    peer::write_descriptor(
        dir.path(),
        &peer::PeerDescriptor {
            daemon_id: "01PEERFAKE".into(),
            name: "wsl-box".into(),
            avatar: "🐺".into(),
            address: "127.0.0.1".into(),
            port: 7257,
            environment: "WSL: Ubuntu-22.04".into(),
            os: String::new(),
            token: "tok".into(),
            protocol_version: peer::PEER_PROTOCOL_VERSION,
            tunnel: None,
            nudge: None,
        },
    )
    .unwrap();

    let resp = fleet_router(registry_path)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/fleet/nudge?daemon_id=01PEERFAKE")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let body = String::from_utf8_lossy(&body);
    assert!(
        body.contains("loginctl enable-linger"),
        "the refusal must name the prerequisite no nudge can substitute for; got: {body}"
    );
}
