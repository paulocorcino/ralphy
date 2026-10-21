use super::*;

#[test]
fn connect_with_a_key_file_never_falls_back() {
    let store = tempfile::tempdir().unwrap();
    let mut fake = FakeHost::before_doctor().answer("uname -s", denied());
    let err = connect(
        &mut fake,
        store.path(),
        "svrapp",
        Some(Path::new("my_key")),
        no_keygen,
        &mut Report::text(Vec::new()),
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

#[test]
fn add_with_a_password_adds_the_peer_key_then_signs_in_with_it() {
    let store = tempfile::tempdir().unwrap();
    // The SSH config and the peer key are refused; the password session
    // answers; then the peer key it added signs in.
    let mut fake = FakeHost::before_doctor()
        .with_password()
        .answer("uname -s", denied())
        .answer("uname -s", denied())
        .answer("uname -s", out(0, "Linux\n", ""))
        .answer("authorized_keys", out(0, "", ""));
    let mut rest = linux_host(LINUX_PROBE, description("linux"));
    fake.answers.append(&mut rest.answers);
    let mut report = Report::json(Vec::new());
    let d = add(
        &mut fake,
        &local(store.path()),
        "svrapp",
        None,
        None,
        fake_keygen,
        |_| true,
        &as_the_host,
        &mut report,
    )
    .unwrap();

    let key = key_path_in(store.path());
    assert_eq!(
        d.tunnel.unwrap().identity_file,
        Some(key.display().to_string())
    );
    assert_eq!(fake.password_commands().len(), 2, "{:?}", fake.commands());
    let append = fake.index_of("authorized_keys").expect("the key line sent");
    assert_eq!(fake.calls[append].2, format!("{PUBLIC}\n").into_bytes());
    assert!(fake.calls[append + 1..]
        .iter()
        .all(|c| c.0.as_deref() == Some(key.as_path())));
    let printed = String::from_utf8(report.into_inner()).unwrap();
    assert!(
        printed.contains("installed its SSH key on")
            && printed.contains("do not need the password"),
        "{printed}"
    );
    assert!(
        !printed.contains("Added"),
        "the host is not added yet: {printed}"
    );
}

#[test]
fn a_password_is_not_used_when_a_key_signs_in() {
    let store = tempfile::tempdir().unwrap();
    let mut fake = linux_host(LINUX_PROBE, description("linux")).with_password();
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
    assert!(fake.password_commands().is_empty(), "{:?}", fake.commands());
}
