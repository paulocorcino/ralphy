use super::*;

#[test]
fn add_refuses_an_unknown_host_before_anything_else() {
    let store = tempfile::tempdir().unwrap();
    let mut fake = FakeHost::before_doctor().answer(
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
        &as_the_host,
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
    let mut fake = FakeHost::before_doctor().answer(
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
        &as_the_host,
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
        &as_the_host,
        &mut Report::text(Vec::new()),
    )
    .unwrap();

    let tunnel = d.tunnel.as_ref().expect("a tunnel section");
    assert_eq!(tunnel.destination, "svrapp");
    assert_eq!(tunnel.local_port, d.port);
    assert_ne!(d.port, 7401, "not the local daemon's port");
    assert_eq!(tunnel.peer_port, 7257);
    assert_eq!(tunnel.peer_socket, None);
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
fn add_names_an_unnamed_host_by_itself_in_one_run() {
    let store = tempfile::tempdir().unwrap();
    let mut first = description("linux");
    first.name = None;
    let mut fake = linux_host(LINUX_PROBE, first).answer("daemon setup", out(0, "", ""));
    add(
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
    let setup = fake.index_of("daemon setup").expect("setup sent");
    assert!(
        fake.commands()[setup].contains("--name svr-paulo --avatar 1"),
        "{:?}",
        fake.commands()
    );
    let restart = fake.index_of("daemon restart").expect("restart sent");
    assert!(setup < restart, "{:?}", fake.commands());
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
    let mut ready = description("linux");
    ready.socket = Some("/home/paulo/.ralphy/daemon.sock".to_string());
    let mut fake = linux_host(LINUX_PROBE, ready);
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
        &as_the_host,
        &mut Report::text(Vec::new()),
    )
    .unwrap();
    assert_eq!(again.port, 7403, "a re-add keeps its port");
}

#[test]
fn add_falls_back_to_the_peer_key() {
    let store = tempfile::tempdir().unwrap();
    let mut fake = FakeHost::before_doctor()
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
        &as_the_host,
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
    let mut fake = FakeHost::before_doctor().answer("uname -s", denied());
    let err = add(
        &mut fake,
        &local(store.path()),
        "svrapp",
        None,
        None,
        fake_keygen,
        |_| true,
        &as_the_host,
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
    let mut fake = FakeHost::before_doctor()
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
        &as_the_host,
        &mut printed,
    )
    .unwrap_err()
    .to_string();
    assert!(err.contains("not ready"), "{err}");
    let printed = String::from_utf8(printed.into_inner()).unwrap();
    assert!(printed.contains("ralphy host install svrapp"), "{printed}");
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
    let mut fake = FakeHost::before_doctor().answer(
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
        &as_the_host,
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
        &as_the_host,
        &mut Report::text(Vec::new()),
    )
    .unwrap_err()
    .to_string();
    assert!(err.contains("own daemon"), "{err}");
    assert!(!peers_dir(store.path()).exists());
}
