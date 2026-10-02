//! The peer tunnel (ADR-0067 §2) against `command_test_child` standing in for
//! `ssh`: it records its argv to `RALPHY_TEST_ARGV_FILE`, then lives for
//! `RALPHY_TEST_SLEEP_MS`. Never a real SSH connection.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use ralphy_daemon::auth::{AuthPolicy, AuthState};
use ralphy_daemon::epoch::SessionEpoch;
use ralphy_daemon::identity::Identity;
use ralphy_daemon::peer::client::SelfRef;
use ralphy_daemon::peer::tunnel::{tunnels, Tunnels};
use ralphy_daemon::peer::{self, PeerDescriptor, TunnelSpec, PEER_PROTOCOL_VERSION};
use ralphy_daemon::{registry, router};
use tower::ServiceExt;

/// The env seams below are process-wide; this keeps the file correct under
/// `cargo test` threads, not only under nextest's process per test.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

struct Fixture {
    _dir: tempfile::TempDir,
    argv_log: PathBuf,
}

fn setup(sleep_ms: u64) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let argv_log = dir.path().join("argv.log");
    // The holder never kills, so a stand-in outlives its test. Run a copy: a
    // live image of `target/debug/command_test_child.exe` makes the next
    // cargo build that relinks it fail on Windows (os error 5).
    let built = Path::new(env!("CARGO_BIN_EXE_command_test_child"));
    let stand_in = dir.path().join(built.file_name().expect("a binary path"));
    std::fs::copy(built, &stand_in).unwrap();
    std::env::set_var("RALPHY_DAEMON_SSH_OVERRIDE", &stand_in);
    std::env::set_var("RALPHY_TEST_ARGV_FILE", &argv_log);
    std::env::set_var("RALPHY_TEST_EXIT_CODE", "0");
    set_sleep(sleep_ms);
    Fixture {
        _dir: dir,
        argv_log,
    }
}

fn set_sleep(ms: u64) {
    std::env::set_var("RALPHY_TEST_SLEEP_MS", ms.to_string());
}

fn spec(local_port: u16) -> TunnelSpec {
    TunnelSpec {
        destination: "svrapp".into(),
        peer_port: 7257,
        peer_socket: None,
        local_port,
        identity_file: None,
    }
}

fn lines(log: &Path) -> Vec<String> {
    std::fs::read_to_string(log)
        .unwrap_or_default()
        .lines()
        .map(str::to_string)
        .collect()
}

/// Poll `log` until it has `n` lines, or 5 s passed; returns what it has.
fn wait_lines(log: &Path, n: usize) -> Vec<String> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let got = lines(log);
        if got.len() >= n || Instant::now() > deadline {
            return got;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn wait_dead(tunnels: &Tunnels, id: &str) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while tunnels.is_alive(id) {
        assert!(Instant::now() < deadline, "the child for {id} never exited");
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[tokio::test]
async fn a_tunnel_is_held_once_and_replaced_once_dead() {
    let _serial = SERIAL.lock().await;
    let fx = setup(60_000);
    let tunnels = Tunnels::new();

    assert!(tunnels.ensure("peer-a", &spec(7401)).unwrap());
    let got = wait_lines(&fx.argv_log, 1);
    assert_eq!(
        got,
        vec![
            "-N -L 127.0.0.1:7401:127.0.0.1:7257 -o BatchMode=yes -o ExitOnForwardFailure=yes \
             -o ServerAliveInterval=15 -o ServerAliveCountMax=3 -- svrapp"
        ]
    );
    assert!(
        !tunnels.ensure("peer-a", &spec(7401)).unwrap(),
        "a running tunnel is kept"
    );
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(lines(&fx.argv_log).len(), 1, "no second ssh was started");

    set_sleep(0);
    assert!(tunnels.ensure("peer-b", &spec(7402)).unwrap());
    wait_dead(&tunnels, "peer-b");
    assert!(
        tunnels.ensure("peer-b", &spec(7402)).unwrap(),
        "an exited tunnel is replaced"
    );
    let got = wait_lines(&fx.argv_log, 3);
    let b: Vec<_> = got
        .iter()
        .filter(|l| l.contains("127.0.0.1:7402:"))
        .collect();
    assert_eq!(b.len(), 2, "got: {got:?}");
}

fn descriptor(id: &str, port: u16, tunnel: Option<TunnelSpec>) -> PeerDescriptor {
    PeerDescriptor {
        daemon_id: id.to_string(),
        name: "svrapp".to_string(),
        avatar: "🐙".to_string(),
        address: "127.0.0.1".to_string(),
        port,
        environment: "Linux".to_string(),
        os: String::new(),
        token: "peer-token".to_string(),
        protocol_version: PEER_PROTOCOL_VERSION,
        nudge: None,
        tunnel,
    }
}

#[tokio::test]
async fn ensure_all_skips_a_peer_without_a_tunnel_and_one_on_our_port() {
    let _serial = SERIAL.lock().await;
    let fx = setup(0);
    let tunnels = Tunnels::new();
    let me = SelfRef {
        port: 7399,
        daemon_id: "01LOCAL",
    };
    tunnels.ensure_all(
        &[
            descriptor("01PLAIN", 7400, None),
            descriptor("01SELF", 7399, Some(spec(7399))),
            descriptor("01TUN", 7403, Some(spec(7403))),
        ],
        me,
    );
    wait_lines(&fx.argv_log, 1);
    std::thread::sleep(Duration::from_millis(500));
    let got = lines(&fx.argv_log);
    assert_eq!(got.len(), 1, "got: {got:?}");
    assert!(got[0].contains("127.0.0.1:7403:"), "got: {got:?}");
}

const LOCAL_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FB0";
const TUNNEL_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FB1";
const COLLIDING_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FB2";
const NUDGE_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FB3";

fn app(id: &str, store: &Path, auth: Arc<AuthState>) -> axum::Router {
    let registry_path = store.join("repos.toml");
    let mut repos = registry::RegistryStore::default();
    repos.upsert("owner/repo", &store.to_string_lossy());
    registry::save_to(&repos, &registry_path).unwrap();
    let (tx, rx) = tokio::sync::watch::channel(false);
    std::mem::forget(tx);
    router(
        Some(Identity {
            id: id.parse().unwrap(),
            name: "svrapp".to_string(),
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

async fn call(app: &axum::Router, method: &str, uri: &str) -> (StatusCode, serde_json::Value) {
    let req = Request::builder()
        .method(method)
        .uri(uri)
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&body).unwrap_or_default())
}

fn peer_row(fleet: &serde_json::Value, id: &str) -> serde_json::Value {
    fleet["peers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["daemon_id"] == id)
        .cloned()
        .unwrap_or_else(|| panic!("no peer {id} in {fleet}"))
}

/// A loopback port with nothing listening on it.
fn closed_port() -> u16 {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    l.local_addr().unwrap().port()
}

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

#[tokio::test]
async fn a_nudge_on_a_tunnel_peer_opens_its_tunnel() {
    let _serial = SERIAL.lock().await;
    let fx = setup(60_000);
    let peer_store = tempfile::tempdir().unwrap();
    let peer_app = app(
        NUDGE_ID,
        peer_store.path(),
        AuthState::fixed(
            AuthPolicy::Bearer("peer-token".to_string()),
            SessionEpoch::in_memory_detached(),
        ),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let serving = tokio::spawn(async move {
        axum::serve(listener, peer_app).await.unwrap();
    });

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

const PAIRED_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FB4";

fn description(token: &str) -> peer::DaemonDescription {
    peer::DaemonDescription {
        daemon_id: Some(PAIRED_ID.to_string()),
        name: Some("svrapp".to_string()),
        avatar: Some("🐙".to_string()),
        environment: "Linux".to_string(),
        os: "linux".to_string(),
        port: 7257,
        protocol_version: PEER_PROTOCOL_VERSION,
        require_token: true,
        autostart: true,
        running: true,
        socket: None,
        token: Some(token.to_string()),
    }
}

#[tokio::test]
async fn a_paired_descriptor_is_reachable_through_its_local_port() {
    let _serial = SERIAL.lock().await;
    let _fx = setup(60_000);
    let peer_store = tempfile::tempdir().unwrap();
    let peer_app = app(
        PAIRED_ID,
        peer_store.path(),
        AuthState::fixed(
            AuthPolicy::Bearer("peer-tok".to_string()),
            SessionEpoch::in_memory_detached(),
        ),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let serving = tokio::spawn(async move {
        axum::serve(listener, peer_app).await.unwrap();
    });

    let store = tempfile::tempdir().unwrap();
    let paired = peer::paired_descriptor(&description("peer-tok"), "svrapp", port, None).unwrap();
    assert_eq!(paired.tunnel.as_ref().unwrap().peer_port, 7257);
    peer::write_descriptor(store.path(), &paired).unwrap();
    let local = app(LOCAL_ID, store.path(), AuthState::localhost());
    let (status, fleet) = call(&local, "GET", "/api/fleet").await;
    assert_eq!(status, StatusCode::OK);
    let row = peer_row(&fleet, PAIRED_ID);
    assert_eq!(row["state"], "reachable", "got: {row}");

    let wrong = peer::paired_descriptor(&description("wrong"), "svrapp", port, None).unwrap();
    peer::write_descriptor(store.path(), &wrong).unwrap();
    let (_, fleet) = call(&local, "GET", "/api/fleet").await;
    let row = peer_row(&fleet, PAIRED_ID);
    assert_ne!(row["state"], "reachable", "a wrong token: {row}");
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
