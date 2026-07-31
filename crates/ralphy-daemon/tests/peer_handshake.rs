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
use ralphy_daemon::peer::client::{probe, PeerStatus, SelfRef};
use ralphy_daemon::peer::{PeerDescriptor, PEER_PROTOCOL_VERSION};

/// The environment label every descriptor here announces. Asserted verbatim in
/// each rejection diagnosis.
const ENV: &str = "WSL: Ubuntu-22.04";

/// The dialling daemon, as these tests model it: a DIFFERENT daemon from the one
/// being probed. Port 0 is never a bound listener, so the self-dial gate stays
/// out of the way, and an empty id disables the identity echo check — both are
/// exercised deliberately in `a_peer_on_this_daemons_own_port_is_refused`.
fn me() -> SelfRef<'static> {
    SelfRef {
        port: 0,
        daemon_id: "",
    }
}

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
    assert_eq!(probe(&d, me()).await, PeerStatus::Reachable);
}

#[tokio::test]
async fn a_wrong_bearer_is_a_legible_rejection() {
    let d = spawn_daemon(AuthPolicy::Bearer("tok".into()), "not-the-token").await;
    let status = probe(&d, me()).await;
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
        get(|| async { Json(serde_json::json!({"protocol_version": 1})) }),
    );
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    let d = descriptor(port, "tok");
    let status = probe(&d, me()).await;
    assert_eq!(
        status,
        PeerStatus::VersionMismatch {
            theirs: 1,
            ours: PEER_PROTOCOL_VERSION
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
    let status = probe(&d, me()).await;
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
    let t0 = std::time::Instant::now();
    let status = probe(&d, me()).await;
    let elapsed = t0.elapsed();
    assert!(
        matches!(status, PeerStatus::Refused { .. }),
        "a routable address must be refused, got: {status:?}"
    );
    // The verdict alone does not prove the socket was never opened: an
    // implementation that dialled FIRST and classified afterwards returns the
    // same `Refused`. A real dial to a routable, unrouted address costs the full
    // 2 s `PEER_TIMEOUT`; refusing without dialling is instant.
    assert!(
        elapsed < std::time::Duration::from_millis(300),
        "the refusal took {elapsed:?} — that is long enough to have dialled, and \
         the loopback gate must refuse BEFORE a socket is opened"
    );

    // Same gate on the raw `get`, which is what fetches a peer's repo list.
    let err = ralphy_daemon::peer::client::get(&d, "/api/repos")
        .await
        .expect_err("a routable address must never be dialled");
    assert!(
        err.to_string().contains("loopback"),
        "the refusal must say why; got: {err}"
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
        probe(&a, me()).await,
        PeerStatus::Unauthorized,
        "the rotated peer is revoked"
    );
    assert_eq!(
        probe(&b, me()).await,
        PeerStatus::Reachable,
        "revoking one peer must leave the other working"
    );
}

/// A descriptor announcing this daemon's own loopback port is refused, unread.
///
/// Both daemons default to the same port, and WSL2's `localhostForwarding` — the
/// relay the whole transport rests on — publishes the peer's listener on that
/// port on the Windows side. If the local daemon started first the relay loses
/// silently, and dialling the "peer" arrives back here.
///
/// Measured in the #353 capstone before this gate existed: the local daemon
/// presented the peer's token to its own auth, was correctly rejected, and
/// reported `its token was rotated` — pointing the operator at a credential that
/// was never broken. The refusal must name the collision instead, and must not
/// depend on what the far end answers, since a self-dial can answer anything.
#[tokio::test]
async fn a_peer_on_this_daemons_own_port_is_refused() {
    let d = spawn_daemon(AuthPolicy::Bearer("tok".into()), "tok").await;
    // Same port as the descriptor announces: this daemon IS what is listening.
    let status = probe(
        &d,
        SelfRef {
            port: d.port,
            daemon_id: "",
        },
    )
    .await;

    let PeerStatus::Refused { .. } = status else {
        panic!("a self-dial must be refused, not dialled; got: {status:?}");
    };
    let diagnosis = status.diagnosis(&d.environment);
    assert!(
        diagnosis.contains(ENV),
        "the refusal must name the environment; got: {diagnosis}"
    );
    assert!(
        diagnosis.contains(&d.port.to_string()),
        "the refusal must name the colliding port; got: {diagnosis}"
    );
    assert!(
        diagnosis.contains("--port"),
        "the refusal must name the fix; got: {diagnosis}"
    );
    assert!(
        !diagnosis.contains("rotated"),
        "a port collision must not be reported as a credential problem; got: {diagnosis}"
    );
}

/// A handshake answering with our OWN id is refused even when the ports differ.
#[tokio::test]
async fn a_handshake_echoing_our_own_identity_is_refused() {
    let d = spawn_daemon(AuthPolicy::Bearer("tok".into()), "tok").await;
    // `spawn_daemon` gives the served daemon the `anvil()` identity, whose id is
    // the nil ULID; claim that same id as ours.
    let status = probe(
        &d,
        SelfRef {
            port: 0,
            daemon_id: &ulid::Ulid::nil().to_string(),
        },
    )
    .await;

    let PeerStatus::Refused { why } = &status else {
        panic!("a loop back to ourselves must be refused; got: {status:?}");
    };
    assert!(
        why.contains("own identity"),
        "the refusal must say the connection looped back; got: {why}"
    );
}
