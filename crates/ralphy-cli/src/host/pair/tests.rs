use super::*;
use crate::host::report::Report;
use ralphy_daemon::peer::key::ensure_peer_key;

mod add;
mod add_fix;
mod add_socket;
mod connect_key;
mod remove;

const KEYS: &str = "ssh-ed25519 OTHER me@laptop\n\
restrict,port-forwarding ssh-ed25519 BODY ralphy-peer@anvil\n\
ssh-ed25519 BODY2 ralphy-peer@forge\n";

#[test]
fn without_key_line_removes_exactly_ours() {
    assert_eq!(
        without_key_line(KEYS, "BODY").as_deref(),
        Some("ssh-ed25519 OTHER me@laptop\nssh-ed25519 BODY2 ralphy-peer@forge\n")
    );
}

#[test]
fn without_key_line_keeps_crlf() {
    let text =
        "ssh-ed25519 OTHER me@laptop\r\nssh-ed25519 BODY ralphy-peer@anvil\r\nssh-rsa LAST x";
    assert_eq!(
        without_key_line(text, "BODY").as_deref(),
        Some("ssh-ed25519 OTHER me@laptop\r\nssh-rsa LAST x")
    );
}

#[test]
fn without_key_line_none_when_absent() {
    assert_eq!(without_key_line(KEYS, "BOD"), None);
    assert_eq!(without_key_line("", "BODY"), None);
}

// ---- the add, check and remove flows, against `FakeHost` ----

use crate::host::checks::tests::description;
use crate::host::ssh::tests::{out, FakeHost};

const LOCAL_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FD0";
const HOST_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FC0";
const PUBLIC: &str = "ssh-ed25519 BODY ralphy-peer@anvil";

// format of `id -u`, `id -un` and `loginctl show-user`
pub(super) const LINUX_PROBE: &str =
    "--- host\nsvr.example.com\n--- uid\n1000\n--- user\npaulo\n--- arch\nx86_64\n--- linger\nLinger=yes\n";
const LINUX_NO_LINGER: &str =
    "--- host\nsvr.example.com\n--- uid\n1000\n--- user\npaulo\n--- arch\nx86_64\n--- linger\nLinger=no\n";

pub(super) fn local(store: &Path) -> Local<'_> {
    Local {
        store,
        daemon_id: Some(LOCAL_ID.to_string()),
        name: Some("anvil".to_string()),
        port: 7401,
        build: ralphy_release::Build::parse("v0.1.0-rc.30"),
        target: Some("linux-x64"),
        latest: None,
    }
}

fn json(d: &ralphy_daemon::peer::DaemonDescription) -> String {
    serde_json::to_string(d).expect("a description serializes")
}

fn fake_keygen(path: &Path) -> Result<()> {
    std::fs::write(path, "PRIVATE")?;
    std::fs::write(format!("{}.pub", path.display()), format!("{PUBLIC}\n"))?;
    Ok(())
}

pub(super) fn no_keygen(_: &Path) -> Result<()> {
    panic!("the peer key must not be generated here")
}

/// A Linux host with Ralphy ready except for `first`'s gaps; the second
/// describe carries the token.
pub(super) fn linux_host(probe: &str, first: ralphy_daemon::peer::DaemonDescription) -> FakeHost {
    let mut second = description("linux");
    second.token = Some("host-tok".to_string());
    FakeHost::before_doctor()
        .answer("uname -s", out(0, "Linux\n", ""))
        .answer("--- uid", out(0, probe, ""))
        .answer("describe --with-token", out(0, &json(&second), ""))
        .answer("describe", out(0, &json(&first), ""))
        .answer("daemon install", out(0, "", ""))
        .answer("require-token on", out(0, "", ""))
        .answer("daemon restart", out(0, "", ""))
}

/// The probe of a host whose tunnel reaches its own daemon.
fn as_the_host(_: &TunnelSpec, _: Option<&str>) -> std::result::Result<String, String> {
    Ok(HOST_ID.to_string())
}

fn peers_dir(store: &Path) -> PathBuf {
    store.join("peers")
}

fn denied() -> HostOutput {
    out(255, "", "paulo@svrapp: Permission denied (publickey).\r\n")
}

#[test]
fn check_applies_nothing() {
    let store = tempfile::tempdir().unwrap();
    let mut first = description("linux");
    first.require_token = false;
    first.autostart = false;
    let mut fake = linux_host(LINUX_NO_LINGER, first);
    let mut printed = Report::text(Vec::new());
    let checks = check(
        &mut fake,
        &local(store.path()),
        Destination {
            dest: "svrapp",
            key_file: None,
        },
        None,
        no_keygen,
        &mut printed,
    )
    .unwrap();
    assert!(checks
        .iter()
        .any(|c| matches!(c.status, CheckStatus::Fix(_))));
    for sent in fake.commands() {
        for word in [
            "enable-linger",
            "require-token",
            "install",
            "setup",
            "restart",
        ] {
            assert!(!sent.contains(word), "{sent}");
        }
    }
    let printed = String::from_utf8(printed.into_inner()).unwrap();
    assert!(printed.contains("Lingering"), "{printed}");
    assert!(!peers_dir(store.path()).exists());
}

#[test]
fn check_creates_the_peer_key_and_hands_over_its_line() {
    let store = tempfile::tempdir().unwrap();
    let mut fake = FakeHost::before_doctor().answer("uname -s", denied());
    let err = check(
        &mut fake,
        &local(store.path()),
        Destination {
            dest: "svrapp",
            key_file: None,
        },
        None,
        fake_keygen,
        &mut Report::text(Vec::new()),
    )
    .unwrap_err();
    let ssh = err
        .downcast_ref::<crate::host::ssh::SshError>()
        .expect("a classified failure");
    assert_eq!(ssh.kind, SshFailure::AuthRefused);
    assert_eq!(ssh.key_line.as_deref(), Some(PUBLIC));
    assert!(key_path_in(store.path()).exists());
    assert_eq!(fake.calls.len(), 2, "{:?}", fake.commands());
    assert!(!peers_dir(store.path()).exists());
}
