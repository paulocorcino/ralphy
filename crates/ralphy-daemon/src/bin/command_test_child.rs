//! Test helper child driven by `tests/command_ws.rs` through
//! `CARGO_BIN_EXE_command_test_child` (pointed at via `RALPHY_EXE_OVERRIDE`). It
//! stands in for the real `ralphy` exe a dispatched command would spawn, so the
//! `/ws/command` spawn → ack → exit path can be exercised with a deterministic
//! exit code and no real run — portable on Windows and Unix, no shell-script
//! child (house convention).
//!
//! Behavior: read `RALPHY_TEST_EXIT_CODE` (default `0`), print stdout+stderr
//! markers, echoes its argv on stdout for the run-params test, and exit with that
//! code. When `RALPHY_TEST_ENV_DUMP`
//! names a path, first write the child's view of BOTH `RALPHY_DAEMON_TOKEN` and
//! `RALPHY_DAEMON_ID` there (newline-separated) — one dump proving the boot-time
//! token strip AND the dispatch-path daemon_id injection reached this child.
//! `RALPHY_TEST_SLEEP_MS` delays the exit (lets a test drop the client mid-run);
//! `RALPHY_TEST_DONE_FILE` names a path the child writes `dispatch-done` to just
//! before exiting (a sentinel proving it ran to completion after a disconnect).

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
    // Markers on BOTH streams: the merged-pipe test asserts the log carries each.
    println!("dispatch-stdout-marker");
    eprintln!("dispatch-stderr-marker");
    // Echo the argv so the run-params test can assert the composed flags reached
    // the child (proving `spawn_argv` → CLI end to end).
    println!(
        "dispatch-argv: {}",
        std::env::args().skip(1).collect::<Vec<_>>().join(" ")
    );
    println!(
        "dispatch-cwd: {}",
        std::env::current_dir()
            .map(|path| path.display().to_string())
            .unwrap_or_default()
    );
    println!("command_test_child exiting {code}");
    if let Some(ms) = std::env::var("RALPHY_TEST_SLEEP_MS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
    {
        std::thread::sleep(std::time::Duration::from_millis(ms));
    }
    if let Ok(argv_file) = std::env::var("RALPHY_TEST_ARGV_FILE") {
        // One line per invocation: the count is how `tests/repos_rekey.rs`
        // proves the registrar was spawned exactly once per (slug, remote).
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&argv_file)
            .expect("opening the argv log");
        writeln!(
            f,
            "{}",
            std::env::args().skip(1).collect::<Vec<_>>().join(" ")
        )
        .expect("appending to the argv log");
    }
    if let Ok(spec) = std::env::var("RALPHY_TEST_REGISTRY_REKEY") {
        // `<file>|<from>|<to>`: stand in for `ralphy daemon add`'s migration by
        // rewriting the registry key — what the real CLI does through
        // `register_or_migrate`, which this child (no `ralphy-core`) cannot. A
        // hash key is a bare TOML key (`[repos.path-abc]`), a forge key a quoted
        // one (`[repos."o/r"]`); both spellings are covered.
        if let [file, from, to] = spec.splitn(3, '|').collect::<Vec<_>>()[..] {
            let text = std::fs::read_to_string(file).expect("reading the registry to re-key");
            let rekeyed = text
                .replace(&format!("repos.{from}]"), &format!("repos.\"{to}\"]"))
                .replace(&format!("\"{from}\""), &format!("\"{to}\""));
            std::fs::write(file, rekeyed).expect("writing the re-keyed registry");
        }
    }
    if let Ok(done_path) = std::env::var("RALPHY_TEST_DONE_FILE") {
        // Sentinel: proof the run reached completion despite a client disconnect.
        // Published by a rename: `fs::write` creates the file before it writes
        // the text, and a test polling in between read "" (Windows CI, #449).
        let staged = format!("{done_path}.tmp");
        std::fs::write(&staged, "dispatch-done").expect("writing the done sentinel");
        std::fs::rename(&staged, &done_path).expect("publishing the done sentinel");
    }
    std::process::exit(code);
}
