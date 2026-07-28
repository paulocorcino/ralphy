//! The peer handshake over a real loopback socket (docs/adr/0052 §3, issue
//! #349): a second daemon router is stood up in-process on `127.0.0.1:0` and
//! dialled through `peer::client`, so the transport, the bearer credential and
//! the version gate are all exercised as real code paths.
//!
//! No WSL, no `wsl.exe`, no real distro — every listener is a loopback socket
//! inside one OS, so this runs unchanged on both CI platforms. The environment
//! label `WSL: Ubuntu-22.04` is DATA in the descriptor, which is exactly the
//! point: a diagnosis must name the environment it is about even when the reader
//! has never seen that machine.

use std::path::PathBuf;
use std::time::Instant;

use axum::routing::get;
use axum::{Json, Router};
use ralphy_daemon::auth::{AuthPolicy, AuthState};
use ralphy_daemon::epoch::SessionEpoch;
use ralphy_daemon::identity;
use ralphy_daemon::peer::client::{probe, PeerStatus};
use ralphy_daemon::peer::{PeerDescriptor, PEER_PROTOCOL_VERSION};

/// The environment label every descriptor here announces. Asserted verbatim in
/// each rejection diagnosis.
const ENV: &str = "WSL: Ubuntu-22.04";

fn anvil() -> identity::Identity {
    identity::Identity {
        id: ulid::Ulid::nil(),
        name: "anvil".into(),
        avatar: "🐙".into(),
    }
}

fn descriptor(port: u16, token: &str) -> PeerDescriptor {
    PeerDescriptor {
        daemon_id: "01TESTPEER".into(),
        name: "anvil".into(),
        avatar: "🐙".into(),
        address: "127.0.0.1".into(),
        port,
        environment: ENV.into(),
        token: token.into(),
        protocol_version: PEER_PROTOCOL_VERSION,
        nudge: None,
    }
}

/// Stand up a real daemon router under `policy` on an OS-assigned loopback port,
/// and return a descriptor pointing at it that announces `announced_token`.
/// Announcing a token OTHER than the policy's is how a rotation is reproduced.
async fn spawn_daemon(policy: AuthPolicy, announced_token: &str) -> PeerDescriptor {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let (_tx, rx) = tokio::sync::watch::channel(false);
    let app = ralphy_daemon::router(
        Some(anvil()),
        PathBuf::from("does-not-exist"),
        PathBuf::from("does-not-exist"),
        ralphy_daemon::StorePaths::default(),
        Instant::now(),
        rx,
        AuthState::fixed(policy, SessionEpoch::in_memory_detached()),
    );
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    descriptor(port, announced_token)
}

#[tokio::test]
async fn handshake_succeeds_with_the_announced_token() {
    let d = spawn_daemon(AuthPolicy::Bearer("tok".into()), "tok").await;
    assert_eq!(probe(&d).await, PeerStatus::Reachable);
}

#[tokio::test]
async fn a_wrong_bearer_is_a_legible_rejection() {
    let d = spawn_daemon(AuthPolicy::Bearer("tok".into()), "not-the-token").await;
    let status = probe(&d).await;
    assert_eq!(status, PeerStatus::Unauthorized, "got: {status:?}");
    let diagnosis = status.diagnosis(&d.environment);
    assert!(
        diagnosis.contains(ENV),
        "the rejection must name the environment; got: {diagnosis}"
    );
    assert!(
        diagnosis.contains("--peer-store"),
        "the rejection must name the fix; got: {diagnosis}"
    );
}

#[tokio::test]
async fn a_version_mismatch_is_a_legible_rejection() {
    // A bare stub, not the daemon router: the daemon can only ever serve its OWN
    // version, so a future peer has to be stood up as one.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let app = Router::new().route(
        "/api/peer/hello",
        get(|| async { Json(serde_json::json!({"protocol_version": 999})) }),
    );
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    let d = descriptor(port, "tok");
    let status = probe(&d).await;
    assert_eq!(
        status,
        PeerStatus::VersionMismatch {
            theirs: 999,
            ours: 1
        },
        "got: {status:?}"
    );
    let diagnosis = status.diagnosis(&d.environment);
    assert!(
        diagnosis.contains(ENV),
        "the mismatch must name the environment; got: {diagnosis}"
    );
}

#[tokio::test]
async fn an_unreachable_peer_is_marked_not_removed() {
    // Bind then DROP, so the port is one nothing is listening on.
    let port = {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        listener.local_addr().unwrap().port()
    };
    let d = descriptor(port, "tok");
    let status = probe(&d).await;
    assert!(
        matches!(status, PeerStatus::Unreachable { .. }),
        "a closed port must be Unreachable, got: {status:?}"
    );
    // The descriptor itself is untouched: the peer is MARKED, never removed.
    assert_eq!(d.port, port);
    assert!(status.diagnosis(&d.environment).contains(ENV));
}

#[tokio::test]
async fn a_non_loopback_descriptor_is_refused_without_dialling() {
    let mut d = descriptor(7257, "tok");
    d.address = "10.0.0.5".into();
    let status = probe(&d).await;
    assert!(
        matches!(status, PeerStatus::Refused { .. }),
        "a routable address must be refused, got: {status:?}"
    );
}

#[tokio::test]
async fn revoking_one_peer_leaves_the_other_working() {
    // Two daemons, two DIFFERENT tokens — there is no shared secret. Peer A has
    // rotated its token since it announced (its descriptor still carries the old
    // one); peer B has not.
    let mut a = spawn_daemon(AuthPolicy::Bearer("a-rotated".into()), "a-announced").await;
    a.daemon_id = "01PEERA".into();
    let mut b = spawn_daemon(AuthPolicy::Bearer("b-token".into()), "b-token").await;
    b.daemon_id = "01PEERB".into();

    assert_eq!(
        probe(&a).await,
        PeerStatus::Unauthorized,
        "the rotated peer is revoked"
    );
    assert_eq!(
        probe(&b).await,
        PeerStatus::Reachable,
        "revoking one peer must leave the other working"
    );
}
