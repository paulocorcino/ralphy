use super::*;
use crate::host::report::Report;

#[test]
fn choose_local_port_skips_the_daemon_port() {
    assert_eq!(choose_local_port(7401, &[], None, |_| true), Some(7402));
}

#[test]
fn choose_local_port_skips_taken_and_busy() {
    assert_eq!(
        choose_local_port(7257, &[7401], None, |p| p != 7402),
        Some(7403)
    );
    assert_eq!(choose_local_port(7257, &[], None, |_| false), None);
}

#[test]
fn choose_local_port_keeps_a_readd() {
    assert_eq!(
        choose_local_port(7257, &[7410], Some(7410), |_| false),
        Some(7410)
    );
    assert_eq!(
        choose_local_port(7410, &[], Some(7410), |_| true),
        Some(7401),
        "an old port that is now the daemon's is not kept"
    );
}

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
const LINUX_PROBE: &str = "--- uid\n1000\n--- user\npaulo\n--- linger\nLinger=yes\n";
const LINUX_NO_LINGER: &str = "--- uid\n1000\n--- user\npaulo\n--- linger\nLinger=no\n";

fn local(store: &Path) -> Local<'_> {
    Local {
        store,
        daemon_id: Some(LOCAL_ID.to_string()),
        name: Some("anvil".to_string()),
        port: 7401,
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

fn no_keygen(_: &Path) -> Result<()> {
    panic!("the peer key must not be generated here")
}

/// A Linux host with Ralphy ready except for `first`'s gaps; the second
/// describe carries the token.
fn linux_host(probe: &str, first: ralphy_daemon::peer::DaemonDescription) -> FakeHost {
    let mut second = description("linux");
    second.token = Some("host-tok".to_string());
    FakeHost::default()
        .answer("uname -s", out(0, "Linux\n", ""))
        .answer("--- uid", out(0, probe, ""))
        .answer("describe --with-token", out(0, &json(&second), ""))
        .answer("describe", out(0, &json(&first), ""))
        .answer("daemon install", out(0, "", ""))
        .answer("require-token on", out(0, "", ""))
        .answer("daemon restart", out(0, "", ""))
}

fn peers_dir(store: &Path) -> PathBuf {
    store.join("peers")
}

fn denied() -> HostOutput {
    out(255, "", "paulo@svrapp: Permission denied (publickey).\r\n")
}

#[test]
fn add_refuses_an_unknown_host_before_anything_else() {
    let store = tempfile::tempdir().unwrap();
    let mut fake = FakeHost::default().answer(
        "uname -s",
        out(
            255,
            "",
            "No ED25519 host key is known for svrapp and you have requested strict checking.\r\nHost key verification failed.\r\n",
        ),
    );
    let err = add(
        &mut fake,
        &local(store.path()),
        "svrapp",
        None,
        None,
        no_keygen,
        |_| true,
        &mut Report::text(Vec::new()),
    )
    .unwrap_err()
    .to_string();
    assert_eq!(fake.calls.len(), 1, "{:?}", fake.commands());
    assert!(err.contains("ssh svrapp"), "{err}");
    assert!(err.contains("sent nothing"), "{err}");
    assert!(!peers_dir(store.path()).exists());
    assert!(!key_path_in(store.path()).exists());
}

#[test]
fn add_refuses_a_changed_host_key() {
    let store = tempfile::tempdir().unwrap();
    let mut fake = FakeHost::default().answer(
        "uname -s",
        out(
            255,
            "",
            "@ WARNING: REMOTE HOST IDENTIFICATION HAS CHANGED! @\r\n",
        ),
    );
    let err = add(
        &mut fake,
        &local(store.path()),
        "svrapp",
        None,
        None,
        no_keygen,
        |_| true,
        &mut Report::text(Vec::new()),
    )
    .unwrap_err()
    .to_string();
    assert_eq!(fake.calls.len(), 1);
    assert!(err.contains("has changed"), "{err}");
    assert!(!peers_dir(store.path()).exists());
}

#[test]
fn add_writes_a_tunnel_descriptor_and_turns_the_marker_on() {
    let store = tempfile::tempdir().unwrap();
    let mut first = description("linux");
    first.require_token = false;
    first.autostart = false;
    let mut fake = linux_host(LINUX_PROBE, first);
    let d = add(
        &mut fake,
        &local(store.path()),
        "svrapp",
        None,
        None,
        no_keygen,
        |_| true,
        &mut Report::text(Vec::new()),
    )
    .unwrap();

    let tunnel = d.tunnel.as_ref().expect("a tunnel section");
    assert_eq!(tunnel.destination, "svrapp");
    assert_eq!(tunnel.local_port, d.port);
    assert_ne!(d.port, 7401, "not the local daemon's port");
    assert_eq!(tunnel.peer_port, 7257);
    assert_eq!(tunnel.identity_file, None);
    assert_eq!(d.token, "host-tok");
    assert_eq!(d.daemon_id, HOST_ID);

    let (written, rejected) = peer::read_store(&peers_dir(store.path()));
    assert!(rejected.is_empty(), "{rejected:?}");
    assert_eq!(written, vec![d]);

    let install = fake
        .index_of("ralphy daemon install")
        .expect("install sent");
    let marker = fake
        .index_of("ralphy daemon require-token on")
        .expect("marker sent");
    let restart = fake
        .index_of("ralphy daemon restart")
        .expect("restart sent");
    let token = fake.index_of("describe --with-token").expect("token read");
    assert!(install < restart, "{:?}", fake.commands());
    assert!(marker < restart && restart < token, "{:?}", fake.commands());
    assert!(fake.calls.iter().all(|c| c.0.is_none()));
}

#[test]
fn add_keeps_the_port_of_a_readd_and_skips_other_hosts() {
    let store = tempfile::tempdir().unwrap();
    let mut other = description("linux");
    other.daemon_id = Some("01ARZ3NDEKTSV4RRFFQ69G5FE0".to_string());
    other.name = Some("box-2".to_string());
    other.token = Some("t".to_string());
    peer::write_descriptor(
        store.path(),
        &peer::paired_descriptor(&other, "other", 7402, None).unwrap(),
    )
    .unwrap();
    let mut fake = linux_host(LINUX_PROBE, description("linux"));
    let d = add(
        &mut fake,
        &local(store.path()),
        "svrapp",
        None,
        None,
        no_keygen,
        |_| true,
        &mut Report::text(Vec::new()),
    )
    .unwrap();
    assert_eq!(d.port, 7403);
    assert!(fake.index_of("restart").is_none(), "{:?}", fake.commands());

    let mut fake = linux_host(LINUX_PROBE, description("linux"));
    let again = add(
        &mut fake,
        &local(store.path()),
        "svrapp",
        None,
        None,
        no_keygen,
        |p| p != 7403,
        &mut Report::text(Vec::new()),
    )
    .unwrap();
    assert_eq!(again.port, 7403, "a re-add keeps its port");
}

#[test]
fn add_falls_back_to_the_peer_key() {
    let store = tempfile::tempdir().unwrap();
    let mut fake = FakeHost::default()
        .answer("uname -s", denied())
        .answer("uname -s", out(0, "Linux\n", ""));
    let mut rest = linux_host(LINUX_PROBE, description("linux"));
    rest.answers.remove(0);
    fake.answers.append(&mut rest.answers);
    let d = add(
        &mut fake,
        &local(store.path()),
        "svrapp",
        None,
        None,
        fake_keygen,
        |_| true,
        &mut Report::text(Vec::new()),
    )
    .unwrap();
    let key = key_path_in(store.path());
    assert_eq!(
        d.tunnel.unwrap().identity_file,
        Some(key.display().to_string())
    );
    assert_eq!(fake.calls[0].0, None);
    assert_eq!(fake.calls[1].0.as_deref(), Some(key.as_path()));
    assert!(fake.calls[2..]
        .iter()
        .all(|c| c.0.as_deref() == Some(key.as_path())));
}

#[test]
fn add_both_keys_refused_prints_the_public_line() {
    let store = tempfile::tempdir().unwrap();
    let mut fake = FakeHost::default().answer("uname -s", denied());
    let err = add(
        &mut fake,
        &local(store.path()),
        "svrapp",
        None,
        None,
        fake_keygen,
        |_| true,
        &mut Report::text(Vec::new()),
    )
    .unwrap_err()
    .to_string();
    for needle in [
        PUBLIC,
        "passphrase",
        ".ssh/authorized_keys",
        "administrators_authorized_keys",
    ] {
        assert!(err.contains(needle), "{needle} not in {err}");
    }
    assert_eq!(fake.calls.len(), 2);
    assert!(!peers_dir(store.path()).exists());
}

#[test]
fn add_blocks_on_a_missing_ralphy_and_writes_nothing() {
    let store = tempfile::tempdir().unwrap();
    let mut fake = FakeHost::default()
        .answer("uname -s", out(0, "Linux\n", ""))
        .answer("--- uid", out(0, LINUX_NO_LINGER, ""))
        .answer("describe", out(127, "", "sh: 1: ralphy: not found\n"));
    let mut printed = Report::text(Vec::new());
    let err = add(
        &mut fake,
        &local(store.path()),
        "svrapp",
        None,
        Some("svrapp"),
        no_keygen,
        |_| true,
        &mut printed,
    )
    .unwrap_err()
    .to_string();
    assert!(err.contains("not ready"), "{err}");
    let printed = String::from_utf8(printed.into_inner()).unwrap();
    assert!(printed.contains("releases"), "{printed}");
    for sent in fake.commands() {
        for word in [
            "setup",
            "install",
            "require-token",
            "enable-linger",
            "restart",
        ] {
            assert!(!sent.contains(word), "{sent}");
        }
    }
    assert!(!peers_dir(store.path()).exists());
}

#[test]
fn add_linger_needs_sudo_is_not_fatal() {
    let store = tempfile::tempdir().unwrap();
    let mut fake = FakeHost::default().answer(
        "enable-linger",
        out(1, "", "Could not enable linger: Access denied\n"),
    );
    fake.answers
        .append(&mut linux_host(LINUX_NO_LINGER, description("linux")).answers);
    let mut printed = Report::text(Vec::new());
    let d = add(
        &mut fake,
        &local(store.path()),
        "svrapp",
        None,
        None,
        no_keygen,
        |_| true,
        &mut printed,
    )
    .unwrap();
    let printed = String::from_utf8(printed.into_inner()).unwrap();
    assert!(
        printed.contains("sudo loginctl enable-linger paulo"),
        "{printed}"
    );
    assert!(fake.index_of("enable-linger").is_some());
    assert!(peers_dir(store.path())
        .join(format!("{}.toml", d.daemon_id))
        .is_file());
}

#[test]
fn add_refuses_this_computers_own_daemon() {
    let store = tempfile::tempdir().unwrap();
    let mut own = description("linux");
    own.daemon_id = Some(LOCAL_ID.to_string());
    let mut fake = linux_host(LINUX_PROBE, own);
    let err = add(
        &mut fake,
        &local(store.path()),
        "localhost",
        None,
        None,
        no_keygen,
        |_| true,
        &mut Report::text(Vec::new()),
    )
    .unwrap_err()
    .to_string();
    assert!(err.contains("own daemon"), "{err}");
    assert!(!peers_dir(store.path()).exists());
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
        "svrapp",
        None,
        None,
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
fn check_never_creates_the_peer_key() {
    let store = tempfile::tempdir().unwrap();
    let mut fake = FakeHost::default().answer("uname -s", denied());
    let err = check(
        &mut fake,
        &local(store.path()),
        "svrapp",
        None,
        None,
        &mut Report::text(Vec::new()),
    )
    .unwrap_err();
    assert!(format!("{err:#}").contains("ralphy host add"), "{err:#}");
    assert!(!key_path_in(store.path()).exists());
}

/// A store with the peer key and a paired descriptor signed in with it.
fn paired_store() -> (tempfile::TempDir, PeerDescriptor) {
    let store = tempfile::tempdir().unwrap();
    let key = ensure_peer_key(store.path(), fake_keygen).unwrap();
    let mut d = description("linux");
    d.token = Some("host-tok".to_string());
    let host =
        peer::paired_descriptor(&d, "svrapp", 7402, Some(key.path.display().to_string())).unwrap();
    peer::write_descriptor(store.path(), &host).unwrap();
    (store, host)
}

fn linux_keys_host() -> FakeHost {
    FakeHost::default()
        .answer("uname -s", out(0, "Linux\n", ""))
        .answer("cat >", out(0, "", ""))
        .answer("cat .ssh/authorized_keys", out(0, KEYS, ""))
        .answer("rotate-token", out(0, "access token: changed\n", ""))
        .answer("daemon restart", out(0, "", ""))
}

fn descriptor_file(store: &Path) -> PathBuf {
    peers_dir(store).join(format!("{HOST_ID}.toml"))
}

#[test]
fn remove_deletes_exactly_our_line_and_forgets_the_descriptor() {
    let (store, host) = paired_store();
    assert_eq!(find_host(store.path(), "svrapp").unwrap(), host);
    assert_eq!(find_host(store.path(), HOST_ID).unwrap(), host);
    let mut fake = linux_keys_host();
    let mut printed = Report::text(Vec::new());
    remove(&mut fake, store.path(), &host, false, &mut printed).unwrap();

    let write = fake
        .calls
        .iter()
        .find(|c| c.1.contains("cat >"))
        .expect("the keys file was written");
    assert_eq!(
        String::from_utf8(write.2.clone()).unwrap(),
        "ssh-ed25519 OTHER me@laptop\nssh-ed25519 BODY2 ralphy-peer@forge\n"
    );
    let key = key_path_in(store.path());
    assert!(fake
        .calls
        .iter()
        .all(|c| c.0.as_deref() == Some(key.as_path())));
    for sent in fake.commands() {
        for word in ["restart", "uninstall", "rotate-token", "stop"] {
            assert!(!sent.contains(word), "{sent}");
        }
    }
    assert!(!descriptor_file(store.path()).exists());
    let printed = String::from_utf8(printed.into_inner()).unwrap();
    assert!(
        printed.contains("closes when the daemon restarts"),
        "{printed}"
    );
}

#[test]
fn remove_rotate_token_rotates_then_restarts() {
    let (store, host) = paired_store();
    let mut fake = linux_keys_host();
    let mut printed = Report::text(Vec::new());
    remove(&mut fake, store.path(), &host, true, &mut printed).unwrap();
    let rotate = fake.index_of("rotate-token").expect("rotated");
    let restart = fake.index_of("daemon restart").expect("restarted");
    let write = fake.index_of("cat >").expect("keys written");
    assert!(rotate < restart && restart < write, "{:?}", fake.commands());
    let printed = String::from_utf8(printed.into_inner()).unwrap();
    assert!(printed.contains("now disconnected"), "{printed}");
    assert!(!descriptor_file(store.path()).exists());
}

#[test]
fn remove_host_silent_prints_the_line() {
    let (store, host) = paired_store();
    let mut fake = FakeHost::default().answer(
        "uname -s",
        out(
            255,
            "",
            "ssh: connect to host svrapp port 22: Connection timed out\r\n",
        ),
    );
    let err = remove(
        &mut fake,
        store.path(),
        &host,
        false,
        &mut Report::text(Vec::new()),
    )
    .unwrap_err()
    .to_string();
    assert!(err.contains(PUBLIC), "{err}");
    assert!(err.contains("remains on svrapp"), "{err}");
    assert_eq!(fake.calls.len(), 1);
    assert!(!descriptor_file(store.path()).exists());
}

#[test]
fn remove_finds_no_unknown_host() {
    let (store, _) = paired_store();
    let err = find_host(store.path(), "nope").unwrap_err().to_string();
    assert!(err.contains("svrapp"), "{err}");
}

#[test]
fn remove_on_a_windows_admin_clears_the_shared_keys_file() {
    let (store, host) = paired_store();
    let mut fake = FakeHost::default()
        .answer(
            "uname -s",
            out(
                9009,
                "",
                "'uname' is not recognized as an internal or external command,\r\n",
            ),
        )
        .answer(
            "cmd /c ver",
            out(0, "\r\nMicrosoft Windows [Version 10.0.26200.1]\r\n", ""),
        )
        .answer(
            "--- groups",
            out(
                0,
                "--- groups \r\nBUILTIN\\Administrators  Alias  S-1-5-32-544  Mandatory group\r\n",
                "",
            ),
        )
        .answer("type nul >", out(0, "", ""))
        .answer("if exist", out(0, &format!("{PUBLIC}\r\n"), ""));
    remove(
        &mut fake,
        store.path(),
        &host,
        false,
        &mut Report::text(Vec::new()),
    )
    .unwrap();
    let cleared = fake
        .index_of("type nul >")
        .expect("the keys file was cleared");
    assert_eq!(
        fake.commands()[cleared],
        r"type nul > C:\ProgramData\ssh\administrators_authorized_keys"
    );
    assert!(
        fake.index_of("findstr").is_none(),
        "findstr fails on empty input: {:?}",
        fake.commands()
    );
    assert!(!descriptor_file(store.path()).exists());
}

#[test]
fn remove_stops_on_a_changed_host_key_and_keeps_the_descriptor() {
    let (store, host) = paired_store();
    let mut fake = FakeHost::default().answer(
        "uname -s",
        out(
            255,
            "",
            "@ WARNING: REMOTE HOST IDENTIFICATION HAS CHANGED! @\r\n",
        ),
    );
    let err = remove(
        &mut fake,
        store.path(),
        &host,
        false,
        &mut Report::text(Vec::new()),
    )
    .unwrap_err()
    .to_string();
    assert!(err.contains("has changed"), "{err}");
    assert!(descriptor_file(store.path()).exists());
}

#[test]
fn remove_rotate_token_on_a_silent_host_keeps_the_descriptor() {
    let (store, host) = paired_store();
    let mut fake = FakeHost::default().answer(
        "uname -s",
        out(
            255,
            "",
            "ssh: connect to host svrapp port 22: Connection refused\r\n",
        ),
    );
    let err = remove(
        &mut fake,
        store.path(),
        &host,
        true,
        &mut Report::text(Vec::new()),
    )
    .unwrap_err()
    .to_string();
    assert!(err.contains("not changed"), "{err}");
    assert!(descriptor_file(store.path()).exists());
}

#[test]
fn connect_with_a_key_file_never_falls_back() {
    let store = tempfile::tempdir().unwrap();
    let mut fake = FakeHost::default().answer("uname -s", denied());
    let err = connect(
        &mut fake,
        store.path(),
        "svrapp",
        Some(Path::new("my_key")),
        no_keygen,
    )
    .unwrap_err();
    let ssh = err
        .downcast_ref::<crate::host::ssh::SshError>()
        .expect("a classified failure");
    assert_eq!(ssh.kind, SshFailure::AuthRefused);
    assert!(ssh.message.contains("my_key"), "{}", ssh.message);
    assert_eq!(fake.calls.len(), 1, "{:?}", fake.commands());
    assert_eq!(fake.calls[0].0.as_deref(), Some(Path::new("my_key")));
    assert!(!key_path_in(store.path()).exists());
}
