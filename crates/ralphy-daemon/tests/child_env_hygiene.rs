//! Child-env hygiene (docs/adr/0032 §4, docs/adr/0072 D7; issues #164, #168):
//! proves two facts in one child dump. A child the daemon dispatches gets none
//! of the daemon's secrets (`RALPHY_DAEMON_TOKEN`, `RALPHY_TELEGRAM_TOKEN`,
//! `RALPHY_EVENTS_TOKEN`), yet DOES receive the dispatch-path
//! `RALPHY_DAEMON_ID`. The test puts all three secrets in its own environment,
//! dispatches the helper bin with a daemon_id, and asserts the dump reads
//! `ABSENT` for each secret next to the injected `RALPHY_DAEMON_ID`.
//!
//! The spawn removes the secrets itself, so this holds whether or not the
//! boot path also stripped `RALPHY_DAEMON_TOKEN` from the process env.
//! Own file (single test) so no intra-process env race with the other suites.

use ralphy_daemon::auth;
use ralphy_daemon::dispatch;

#[test]
fn spawned_child_does_not_inherit_the_token() {
    let dir = tempfile::tempdir().unwrap();
    let dump = dir.path().join("dump.txt");

    // What a daemon started with every secret in its environment carries.
    // Fake values only.
    std::env::set_var(auth::TOKEN_ENV, "fake-daemon-token");
    std::env::set_var("RALPHY_TELEGRAM_TOKEN", "fake-telegram-token");
    std::env::set_var("RALPHY_EVENTS_TOKEN", "fake-events-token");
    std::env::set_var(
        "RALPHY_EXE_OVERRIDE",
        env!("CARGO_BIN_EXE_command_test_child"),
    );
    std::env::set_var("RALPHY_TEST_ENV_DUMP", &dump);

    let mut child = dispatch::dispatch(
        &dispatch::ProcessSpawner,
        &dispatch::ralphy_exe(),
        &["run", "--if-idle"],
        dir.path(),
        Some("01DAEMONID0000000000000000"),
    )
    .expect("dispatching the helper child");
    let _ = child.wait();

    let contents = std::fs::read_to_string(&dump).expect("the child must write its env dump");
    for secret in [
        "RALPHY_DAEMON_TOKEN",
        "RALPHY_TELEGRAM_TOKEN",
        "RALPHY_EVENTS_TOKEN",
    ] {
        assert!(
            contents.contains(&format!("{secret}=ABSENT")),
            "the spawned child must not inherit {secret}; dump: {contents}"
        );
    }
    assert!(
        contents.contains("RALPHY_DAEMON_ID=01DAEMONID0000000000000000"),
        "the spawned child must receive the injected daemon_id; dump: {contents}"
    );

    std::env::remove_var("RALPHY_EXE_OVERRIDE");
    std::env::remove_var("RALPHY_TEST_ENV_DUMP");
}
