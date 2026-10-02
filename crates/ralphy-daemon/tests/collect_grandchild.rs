//! `collect` on a real OS child whose grandchild inherited the output pipe and
//! holds it open: the child's answer must come back without waiting for the
//! grandchild. Proves the handle inheritance on each OS, which the unit fakes
//! cannot.

use std::sync::mpsc;
use std::time::Duration;

use ralphy_daemon::dispatch::{collect, ProcessSpawner};

#[test]
fn collect_returns_when_a_grandchild_holds_the_pipe() {
    // The grandchild outlives this test. Run a copy from a temp dir, so it never
    // holds `target/debug` files that the next build must replace.
    let tmp = tempfile::tempdir().expect("creating a temp dir");
    let exe = tmp.path().join(format!(
        "command_test_child{}",
        std::env::consts::EXE_SUFFIX
    ));
    std::fs::copy(env!("CARGO_BIN_EXE_command_test_child"), &exe).expect("copying the test child");
    let cwd = tmp.path().to_path_buf();
    // Inherited by the spawned child; this binary holds one test.
    std::env::set_var("RALPHY_TEST_GRANDCHILD_MS", "20000");
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let got = collect(&ProcessSpawner, exe.as_os_str(), &[], &cwd, None);
        tx.send(got.map_err(|e| format!("{e:#}")))
            .expect("the test waits for the answer");
    });
    let (code, out) = rx
        .recv_timeout(Duration::from_secs(5))
        .expect("collect must answer before the grandchild exits")
        .expect("collect must succeed");
    assert_eq!(code, Some(0));
    let out = String::from_utf8_lossy(&out);
    assert!(out.contains("dispatch-stdout-marker"), "output: {out}");
    // The grandchild still runs from `tmp`; keep the dir rather than fail to
    // delete a running exe on Windows.
    let _kept = tmp.keep();
}
