//! The peer tunnel (ADR-0067 §2) against `command_test_child` standing in for
//! `ssh`: it records its argv to `RALPHY_TEST_ARGV_FILE`, then lives for
//! `RALPHY_TEST_SLEEP_MS`. Never a real SSH connection.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use ralphy_daemon::peer::client::SelfRef;
use ralphy_daemon::peer::tunnel::Tunnels;
use ralphy_daemon::peer::{PeerDescriptor, TunnelSpec, PEER_PROTOCOL_VERSION};

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
    std::env::set_var(
        "RALPHY_DAEMON_SSH_OVERRIDE",
        env!("CARGO_BIN_EXE_command_test_child"),
    );
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
    let fx = setup(10_000);
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
