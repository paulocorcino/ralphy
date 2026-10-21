use super::*;

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
    FakeHost::before_doctor()
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
    let mut fake = FakeHost::before_doctor().answer(
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
    let mut fake = FakeHost::before_doctor()
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
    let mut fake = FakeHost::before_doctor().answer(
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
    let mut fake = FakeHost::before_doctor().answer(
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
