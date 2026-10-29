use super::*;

/// A ready Linux host with a socket: only `first`'s gaps need a fix.
fn host_with_socket_and(first_gap: impl FnOnce(&mut DaemonDescription)) -> FakeHost {
    let mut first = description("linux");
    first.socket = Some("/home/ralphy2/.ralphy/daemon.sock".to_string());
    first_gap(&mut first);
    linux_host(LINUX_PROBE, first)
}

fn run_add(fake: &mut FakeHost, store: &Path) -> Result<PeerDescriptor> {
    add(
        fake,
        &local(store),
        "svrapp",
        None,
        None,
        no_keygen,
        |_| true,
        &as_the_host,
        &mut Report::text(Vec::new()),
    )
}

#[test]
fn a_fix_that_needs_a_restart_restarts_a_daemon_that_has_a_socket() {
    let store = tempfile::tempdir().unwrap();
    let mut fake = host_with_socket_and(|d| d.require_token = false);
    run_add(&mut fake, store.path()).unwrap();
    assert!(
        fake.index_of("ralphy daemon require-token on").is_some(),
        "{:?}",
        fake.commands()
    );
    let restarts = fake
        .commands()
        .iter()
        .filter(|c| c.contains("daemon restart"))
        .count();
    assert_eq!(restarts, 1, "{:?}", fake.commands());
}

#[test]
fn a_failing_fix_names_the_host_and_writes_no_descriptor() {
    let store = tempfile::tempdir().unwrap();
    let mut fake = FakeHost::before_doctor().answer(
        "require-token on",
        out(1, "", "no permission to write the settings\n"),
    );
    let mut ready = host_with_socket_and(|d| d.require_token = false);
    fake.answers.append(&mut ready.answers);
    let err = run_add(&mut fake, store.path()).unwrap_err().to_string();
    assert!(err.contains("failed on svrapp"), "{err}");
    assert!(err.contains("no permission to write the settings"), "{err}");
    assert!(fake.index_of("daemon restart").is_none());
    assert!(!peers_dir(store.path()).exists());
}

// format of `whoami /groups` on an account that is not an administrator
const WINDOWS_PROBE: &str = "--- host\r\nwinbox\r\n--- user\r\npaulo\r\n--- groups \r\n\
Everyone                               Well-known group S-1-1-0      Mandatory group\r\n\
BUILTIN\\Users                          Alias            S-1-5-32-545 Mandatory group\r\n\
--- arch\r\nAMD64\r\n";

#[test]
fn a_windows_host_with_nothing_to_fix_is_not_restarted() {
    let store = tempfile::tempdir().unwrap();
    let mut second = description("windows");
    second.token = Some("host-tok".to_string());
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
        .answer("--- groups", out(0, WINDOWS_PROBE, ""))
        .answer("describe --with-token", out(0, &json(&second), ""))
        .answer("describe", out(0, &json(&description("windows")), ""));
    run_add(&mut fake, store.path()).unwrap();
    assert!(
        fake.index_of("daemon restart").is_none(),
        "{:?}",
        fake.commands()
    );
}
