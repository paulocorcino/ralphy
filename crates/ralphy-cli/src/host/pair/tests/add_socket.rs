use super::*;

#[test]
fn add_to_a_host_that_reports_a_socket_writes_peer_socket() {
    let store = tempfile::tempdir().unwrap();
    let mut first = description("linux");
    first.require_token = false;
    first.autostart = false;
    let mut second = description("linux");
    second.token = Some("host-tok".to_string());
    second.socket = Some("/home/ralphy2/.ralphy/daemon.sock".to_string());
    let mut fake = FakeHost::before_doctor()
        .answer("uname -s", out(0, "Linux\n", ""))
        .answer("--- uid", out(0, LINUX_PROBE, ""))
        .answer("describe --with-token", out(0, &json(&second), ""))
        .answer("describe", out(0, &json(&first), ""))
        .answer("daemon install", out(0, "", ""))
        .answer("require-token on", out(0, "", ""))
        .answer("daemon restart", out(0, "", ""));
    let d = add(
        &mut fake,
        &local(store.path()),
        "svrapp",
        None,
        None,
        no_keygen,
        |_| true,
        &as_the_host,
        &mut Report::text(Vec::new()),
    )
    .unwrap();
    let tunnel = d.tunnel.as_ref().expect("a tunnel section");
    assert_eq!(tunnel.peer_port, 0);
    assert_eq!(
        tunnel.peer_socket.as_deref(),
        Some("/home/ralphy2/.ralphy/daemon.sock")
    );
}

/// A ready Linux host whose daemon reports a socket.
fn host_with_a_socket() -> FakeHost {
    let mut second = description("linux");
    second.token = Some("host-tok".to_string());
    second.socket = Some("/home/ralphy2/.ralphy/daemon.sock".to_string());
    let mut first = second.clone();
    first.token = None;
    FakeHost::before_doctor()
        .answer("uname -s", out(0, "Linux\n", ""))
        .answer("--- uid", out(0, LINUX_PROBE, ""))
        .answer("describe --with-token", out(0, &json(&second), ""))
        .answer("describe", out(0, &json(&first), ""))
}

/// What `ssh` prints for each connection when the host's sshd has
/// `AllowStreamLocalForwarding no` (measured 2026-10-02, OpenSSH 8.2p1).
const NOT_FORWARDED: &str = "channel 1: open failed: connect failed: open failed";

#[test]
fn a_socket_the_host_does_not_forward_falls_back_to_the_port() {
    let store = tempfile::tempdir().unwrap();
    let mut fake = host_with_a_socket();
    let tokens = std::cell::RefCell::new(Vec::new());
    let probe = |t: &TunnelSpec, token: Option<&str>| {
        tokens.borrow_mut().push(token.map(str::to_string));
        match t.peer_socket {
            Some(_) => Err(NOT_FORWARDED.to_string()),
            None => Ok(HOST_ID.to_string()),
        }
    };
    let mut report = Report::text(Vec::new());
    let d = add(
        &mut fake,
        &local(store.path()),
        "svrapp",
        None,
        None,
        no_keygen,
        |_| true,
        &probe,
        &mut report,
    )
    .unwrap();
    let tunnel = d.tunnel.as_ref().expect("a tunnel section");
    assert_eq!(
        (tunnel.peer_port, tunnel.peer_socket.as_deref()),
        (7257, None)
    );
    let (written, _) = peer::read_store(&peers_dir(store.path()));
    assert_eq!(written, vec![d.clone()]);
    assert_eq!(
        *tokens.borrow(),
        vec![Some("host-tok".to_string()), Some("host-tok".to_string())]
    );
    let printed = String::from_utf8(report.into_inner()).unwrap();
    assert!(
        printed.contains(&format!(
            "The host does not forward a connection to the daemon's socket ({NOT_FORWARDED}). This computer reaches the daemon through its port 7257."
        )),
        "{printed}"
    );
}

#[test]
fn a_port_that_belongs_to_another_account_fails_the_add_before_writing() {
    let store = tempfile::tempdir().unwrap();
    let mut fake = host_with_a_socket();
    let probe = |t: &TunnelSpec, _: Option<&str>| match t.peer_socket {
        Some(_) => Err(NOT_FORWARDED.to_string()),
        None => Ok("01ARZ3NDEKTSV4RRFFQ69G5FC1".to_string()),
    };
    let err = add(
        &mut fake,
        &local(store.path()),
        "svrapp",
        None,
        None,
        no_keygen,
        |_| true,
        &probe,
        &mut Report::text(Vec::new()),
    )
    .unwrap_err()
    .to_string();
    assert_eq!(
        err,
        format!(
            "the host does not forward a connection to the daemon's socket ({NOT_FORWARDED}), and its port 7257 belongs to another daemon (01ARZ3NDEKTSV4RRFFQ69G5FC1): ask the host's administrator to set `AllowStreamLocalForwarding yes` for sshd"
        )
    );
    assert!(!peers_dir(store.path()).exists());

    // The other account's daemon refuses this account's token, so the port
    // usually gives no id at all.
    let mut fake = host_with_a_socket();
    let refused = "a daemon answers there, but it refused this account's access token";
    let probe = |t: &TunnelSpec, _: Option<&str>| match t.peer_socket {
        Some(_) => Err(NOT_FORWARDED.to_string()),
        None => Err(refused.to_string()),
    };
    let err = add(
        &mut fake,
        &local(store.path()),
        "svrapp",
        None,
        None,
        no_keygen,
        |_| true,
        &probe,
        &mut Report::text(Vec::new()),
    )
    .unwrap_err()
    .to_string();
    assert_eq!(
        err,
        format!(
            "the host does not forward a connection to the daemon's socket ({NOT_FORWARDED}), and its port 7257 does not reach this account's daemon ({refused}): ask the host's administrator to set `AllowStreamLocalForwarding yes` for sshd"
        )
    );
    assert!(!peers_dir(store.path()).exists());
}

#[test]
fn a_running_daemon_with_no_socket_is_restarted_to_get_one() {
    let store = tempfile::tempdir().unwrap();
    let first = description("linux");
    assert!(first.running && first.require_token && first.socket.is_none());
    let mut second = description("linux");
    second.token = Some("host-tok".to_string());
    second.socket = Some("/home/ralphy2/.ralphy/daemon.sock".to_string());
    let mut fake = FakeHost::before_doctor()
        .answer("uname -s", out(0, "Linux\n", ""))
        .answer("--- uid", out(0, LINUX_PROBE, ""))
        .answer("describe --with-token", out(0, &json(&second), ""))
        .answer("describe", out(0, &json(&first), ""))
        .answer("daemon restart", out(0, "", ""));
    let d = add(
        &mut fake,
        &local(store.path()),
        "svrapp",
        None,
        None,
        no_keygen,
        |_| true,
        &as_the_host,
        &mut Report::text(Vec::new()),
    )
    .unwrap();
    assert!(fake.index_of("restart").is_some(), "{:?}", fake.commands());
    let tunnel = d.tunnel.as_ref().expect("a tunnel section");
    assert_eq!(
        tunnel.peer_socket.as_deref(),
        Some("/home/ralphy2/.ralphy/daemon.sock")
    );
}
