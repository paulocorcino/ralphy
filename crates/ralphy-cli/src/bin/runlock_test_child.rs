//! Test-only helper child for `tests/mutate.rs`: a live, non-shell process
//! whose PID a test writes into a `run.lock` fixture to exercise the
//! `HeldAlive` refusal path deterministically — portable on Windows and Unix
//! (house convention, mirrors `ralphy-daemon/src/bin/command_test_child.rs`).
//!
//! Behavior: sleep 60s then exit 0. The test kills it once done with it.

fn main() {
    std::thread::sleep(std::time::Duration::from_secs(60));
}
