//! Child-env hygiene (docs/adr/0032 §4; issues #164, #168): proves two facts in
//! one child dump. A child dispatched AFTER `auth::strip_token_from_env()` does
//! NOT inherit `RALPHY_DAEMON_TOKEN`, yet DOES receive the dispatch-path
//! `RALPHY_DAEMON_ID`. The test sets the token, calls the strip itself,
//! dispatches the helper bin with a daemon_id, and asserts the dump reads
//! `RALPHY_DAEMON_TOKEN=ABSENT` next to the injected `RALPHY_DAEMON_ID`.
//!
//! Limit: the test calls the strip itself, so it does NOT prove that the
//! daemon's boot calls it. Deleting that call in `serve.rs` leaves this test
//! green; the boot path has no seam a test can reach yet (issue #485 follow-up).
//! Own file (single test) so no intra-process env race with the other suites.

use ralphy_daemon::auth;
use ralphy_daemon::dispatch;

#[test]
fn spawned_child_does_not_inherit_the_token() {
    let dir = tempfile::tempdir().unwrap();
    let dump = dir.path().join("dump.txt");

    // The daemon would have this in its env at boot; the strip must remove it
    // before any child is spawned. Here the test runs the strip, not the boot.
    std::env::set_var(auth::TOKEN_ENV, "secret");
    std::env::set_var(
        "RALPHY_EXE_OVERRIDE",
        env!("CARGO_BIN_EXE_command_test_child"),
    );
    std::env::set_var("RALPHY_TEST_ENV_DUMP", &dump);

    auth::strip_token_from_env();

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
    assert!(
        contents.contains("RALPHY_DAEMON_TOKEN=ABSENT"),
        "the spawned child must not inherit the token; dump: {contents}"
    );
    assert!(
        contents.contains("RALPHY_DAEMON_ID=01DAEMONID0000000000000000"),
        "the spawned child must receive the injected daemon_id; dump: {contents}"
    );

    std::env::remove_var("RALPHY_EXE_OVERRIDE");
    std::env::remove_var("RALPHY_TEST_ENV_DUMP");
}
