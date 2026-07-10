//! Test helper child driven by `tests/command_ws.rs` through
//! `CARGO_BIN_EXE_command_test_child` (pointed at via `RALPHY_EXE_OVERRIDE`). It
//! stands in for the real `ralphy` exe a dispatched command would spawn, so the
//! `/ws/command` spawn → ack → exit path can be exercised with a deterministic
//! exit code and no real run — portable on Windows and Unix, no shell-script
//! child (house convention).
//!
//! Behavior: read `RALPHY_TEST_EXIT_CODE` (default `0`), print a one-line marker,
//! and exit with that code. Ignores its argv. When `RALPHY_TEST_ENV_DUMP` names a
//! path, first write the child's view of BOTH `RALPHY_DAEMON_TOKEN` and
//! `RALPHY_DAEMON_ID` there (newline-separated) — one dump proving the boot-time
//! token strip AND the dispatch-path daemon_id injection reached this child.

fn main() {
    if let Ok(dump_path) = std::env::var("RALPHY_TEST_ENV_DUMP") {
        let token = std::env::var("RALPHY_DAEMON_TOKEN").unwrap_or_else(|_| "ABSENT".into());
        let daemon_id = std::env::var("RALPHY_DAEMON_ID").unwrap_or_else(|_| "ABSENT".into());
        std::fs::write(
            &dump_path,
            format!("RALPHY_DAEMON_TOKEN={token}\nRALPHY_DAEMON_ID={daemon_id}"),
        )
        .expect("writing the env dump");
    }
    let code = std::env::var("RALPHY_TEST_EXIT_CODE")
        .ok()
        .and_then(|v| v.parse::<i32>().ok())
        .unwrap_or(0);
    println!("command_test_child exiting {code}");
    std::process::exit(code);
}
