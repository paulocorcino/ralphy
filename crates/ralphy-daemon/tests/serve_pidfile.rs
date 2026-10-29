//! A daemon whose start fails after it recorded its pid leaves no pid file
//! behind: a stale file names a process that does not serve.
//!
//! SOLE env-setter in its file: `RALPHY_DAEMON_DIR`, `RALPHY_DAEMON_TOKEN` and
//! `RALPHY_EXE_OVERRIDE` are process-global, so this test must be alone in its
//! file (no intra-process race).

use std::net::{IpAddr, Ipv4Addr};

use ralphy_daemon::{auth, pidfile, release, DaemonConfig};

#[test]
fn a_start_that_fails_after_the_pid_is_written_leaves_no_pid_file() {
    let store = tempfile::tempdir().expect("a temporary store");
    // The store is the temporary folder; nothing reads or writes `~/.ralphy`.
    std::env::set_var("RALPHY_DAEMON_DIR", store.path());
    // With no token in the environment, the daemon reads `daemon-token` from
    // the store.
    std::env::remove_var(auth::TOKEN_ENV);
    // The start runs the console hook check with the daemon's own program. A
    // program that does not exist makes the check fail at once instead of
    // starting this test binary again.
    std::env::set_var("RALPHY_EXE_OVERRIDE", store.path().join("no-such-ralphy"));
    // No release check, so the start never reaches the network.
    release::set_watch_disabled_in(store.path(), true).expect("turning the release watch off");
    // A folder where the token file belongs: reading the token fails, after
    // the pid was recorded.
    std::fs::create_dir(auth::token_path_in(store.path())).expect("a folder named daemon-token");

    let started = ralphy_daemon::run(DaemonConfig {
        port: 0,
        bind: IpAddr::V4(Ipv4Addr::LOCALHOST),
        allowed_hosts: Vec::new(),
        peer_stores: Vec::new(),
    });

    let err = match started {
        Ok(()) => panic!("a start with an unreadable token must fail"),
        Err(e) => format!("{e:#}"),
    };
    // The failure is the token read, which comes after the pid is written.
    assert!(
        err.contains("daemon-token"),
        "the start must fail on reading the token: {err}"
    );
    assert!(
        !pidfile::pid_path_in(store.path()).exists(),
        "a failed start must not leave a pid file"
    );

    std::env::remove_var("RALPHY_EXE_OVERRIDE");
    std::env::remove_var("RALPHY_DAEMON_DIR");
}
