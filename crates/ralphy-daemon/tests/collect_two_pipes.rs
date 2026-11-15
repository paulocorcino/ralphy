//! `collect_apart` with the two-pipe spawner on a real OS child: stdout and
//! stderr come back in two buffers, and a child that fills both pipes does not
//! hang the collect.
#![expect(
    clippy::tests_outside_test_module,
    reason = "an integration test crate is the test module"
)]

use std::sync::mpsc;
use std::time::Duration;

use ralphy_daemon::dispatch::{collect_apart, CollectedOutput, TwoPipeSpawner};

const FOUR_MIB: usize = 4 * 1024 * 1024;

/// Collect the test child flooding `FOUR_MIB` on each pipe, within 30 s. Every
/// test sets the same value, so tests that share this process agree on it.
fn collect_flood() -> CollectedOutput {
    let tmp = tempfile::tempdir().expect("creating a temp dir");
    let exe = tmp.path().join(format!(
        "command_test_child{}",
        std::env::consts::EXE_SUFFIX
    ));
    std::fs::copy(env!("CARGO_BIN_EXE_command_test_child"), &exe).expect("copying the test child");
    let cwd = tmp.path().to_path_buf();
    #[expect(
        clippy::disallowed_methods,
        reason = "every test of this binary sets the same value, so a race changes nothing"
    )]
    std::env::set_var("RALPHY_TEST_FLOOD_BYTES", FOUR_MIB.to_string());
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let got = collect_apart(&TwoPipeSpawner, exe.as_os_str(), &[], &cwd, None);
        // The test may have stopped waiting, and then nobody reads the answer.
        match tx.send(got.map_err(|e| format!("{e:#}"))) {
            Ok(()) | Err(_) => {}
        }
    });
    let (code, out) = rx
        .recv_timeout(Duration::from_secs(30))
        .expect("collect must answer within 30 s")
        .expect("collect must succeed");
    assert_eq!(code, Some(0));
    out
}

#[test]
fn a_child_filling_both_pipes_does_not_hang() {
    let out = collect_flood();
    assert_eq!(out.stdout.len(), FOUR_MIB, "every stdout byte arrives");
    assert_eq!(out.stderr.len(), FOUR_MIB, "every stderr byte arrives");
}

#[test]
fn stdout_and_stderr_stay_apart() {
    let out = collect_flood();
    assert!(
        out.stdout.iter().all(|&b| b == b'o'),
        "stdout holds only stdout bytes"
    );
    assert!(
        out.stderr.iter().all(|&b| b == b'e'),
        "stderr holds only stderr bytes"
    );
    assert!(
        !out.stdout.is_empty() && !out.stderr.is_empty(),
        "both pipes carried bytes"
    );
}
