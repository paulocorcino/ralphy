//! Child-env hygiene (docs/adr/0032 §4; issue #164): after the boot-time
//! `auth::strip_token_from_env()`, a child the daemon spawns must NOT inherit
//! `RALPHY_DAEMON_TOKEN`. Sets the token, strips it, dispatches the helper bin
//! (which dumps its own view of the var), and asserts the dump reads `ABSENT`.
//! Own file (single test) so no intra-process env race with the other suites.

use ralphy_daemon::auth;
use ralphy_daemon::dispatch;

#[test]
fn spawned_child_does_not_inherit_the_token() {
    let dir = tempfile::tempdir().unwrap();
    let dump = dir.path().join("dump.txt");

    // The daemon would have this in its env at boot; the strip must remove it
    // before any child is spawned.
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
        dispatch::Verb::Run,
        dir.path(),
    )
    .expect("dispatching the helper child");
    let _ = child.wait();

    let contents = std::fs::read_to_string(&dump).expect("the child must write its env dump");
    assert!(
        contents.contains("RALPHY_DAEMON_TOKEN=ABSENT"),
        "the spawned child must not inherit the token; dump: {contents}"
    );

    std::env::remove_var("RALPHY_EXE_OVERRIDE");
    std::env::remove_var("RALPHY_TEST_ENV_DUMP");
}
