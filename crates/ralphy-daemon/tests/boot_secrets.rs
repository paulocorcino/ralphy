//! A daemon started with its secrets in its environment passes none of them
//! to the children it starts at boot: the hook start check and the
//! `claude --version` read it makes for that check. Neither spawn removes the
//! secrets itself, so this holds only when `run` removed them from the
//! daemon's own environment before the runtime started.
//!
//! SOLE env-setter in its file: the variables are process-global, so this
//! test must be alone in its file (no intra-process race).

#![expect(
    clippy::disallowed_methods,
    reason = "nextest runs each test in its own process, so a test may change its environment"
)]

use std::net::{IpAddr, Ipv4Addr};
use std::path::Path;
use std::time::{Duration, Instant};

use ralphy_daemon::{auth, child_env, epoch, release, DaemonConfig};

/// How long the test waits for both children to write their dump.
const DUMP_WAIT: Duration = Duration::from_secs(15);

#[test]
fn boot_children_get_none_of_the_daemons_secrets() {
    let store = tempfile::tempdir().expect("a temporary store");
    let bin = tempfile::tempdir().expect("a folder for the fake claude");
    let hook_dump = store.path().join("hook-dump.txt");
    let version_dump = store.path().join("version-dump.txt");

    // Fake values only.
    std::env::set_var(auth::TOKEN_ENV, "fake-daemon-token");
    std::env::set_var("RALPHY_TELEGRAM_TOKEN", "fake-telegram-token");
    std::env::set_var("RALPHY_EVENTS_TOKEN", "fake-events-token");
    // The store is the temporary folder; nothing reads or writes `~/.ralphy`.
    std::env::set_var("RALPHY_DAEMON_DIR", store.path());
    // The hook program of the start check is the daemon's own program.
    let child = env!("CARGO_BIN_EXE_command_test_child");
    std::env::set_var("RALPHY_EXE_OVERRIDE", child);
    std::env::set_var("RALPHY_TEST_ENV_DUMP", &hook_dump);
    // The daemon finds `claude` on `PATH`; the fake answers `--version` with a
    // version new enough for the hook form the check runs.
    let claude = bin
        .path()
        .join(format!("claude{}", std::env::consts::EXE_SUFFIX));
    std::fs::copy(child, &claude).expect("copying the test child as claude");
    let path = std::env::var_os("PATH").unwrap_or_default();
    let dirs = std::iter::once(bin.path().to_path_buf()).chain(std::env::split_paths(&path));
    std::env::set_var(
        "PATH",
        std::env::join_paths(dirs).expect("joining the PATH"),
    );
    std::env::set_var("RALPHY_TEST_VERSION_DUMP", &version_dump);
    // No release check, so the start never reaches the network.
    release::set_watch_disabled_in(store.path(), true).expect("turning the release watch off");
    // A folder where the session epoch file belongs: the start fails after it
    // started the hook check.
    std::fs::create_dir(epoch::epoch_path_in(store.path())).expect("a folder for the epoch");

    let started = ralphy_daemon::run(DaemonConfig {
        port: 0,
        bind: IpAddr::V4(Ipv4Addr::LOCALHOST),
        allowed_hosts: Vec::new(),
        peer_stores: Vec::new(),
    });
    assert!(
        started.is_err(),
        "a start with an unreadable epoch must fail"
    );

    for dump in [&version_dump, &hook_dump] {
        let text = wait_for_dump(dump);
        for secret in child_env::SECRET_ENV {
            assert!(
                text.contains(&format!("{secret}=ABSENT")),
                "{} must not see {secret}:\n{text}",
                dump.display()
            );
        }
    }
}

/// The text of `dump` once it has all four lines, or a panic after
/// [`DUMP_WAIT`].
fn wait_for_dump(dump: &Path) -> String {
    let deadline = Instant::now() + DUMP_WAIT;
    loop {
        if let Ok(text) = std::fs::read_to_string(dump) {
            if text.lines().count() == 4 {
                return text;
            }
        }
        assert!(
            Instant::now() < deadline,
            "{} was not written in {} s",
            dump.display(),
            DUMP_WAIT.as_secs()
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}
