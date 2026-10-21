use std::time::Duration;

use ralphy_adapter_support::{run_init_session, run_text_session, JsonSession, TextSession};

use super::*;

#[test]
fn run_text_session_returns_the_log_and_bails_on_auth_then_timeout() {
    // Clean child: no auth match, no timeout → returns the combined log.
    let log_path = temp_log("text-clean");
    let _ = std::fs::remove_file(&log_path);
    let log = run_text_session(
        TextSession {
            cmd: child_cmd("clean"),
            prompt: "ignored prompt",
            timeout: Duration::from_secs(30),
            log_path: &log_path,
            spawn_err: "failed to spawn the test child",
            auth_msg: "AUTH FAILED",
            timeout_msg: "TIMED OUT",
        },
        |_log| false,
    )
    .expect("a clean child with no auth match returns the log");
    assert!(log.contains(CLEAN_STDOUT) && log.contains(CLEAN_STDERR));
    assert!(log_path.is_file(), "the log file was written");
    let _ = std::fs::remove_file(&log_path);

    // Auth detector fires on the stdout marker → Err carries auth_msg + log path.
    let log_path = temp_log("text-auth");
    let _ = std::fs::remove_file(&log_path);
    let err = run_text_session(
        TextSession {
            cmd: child_cmd("clean"),
            prompt: "ignored prompt",
            timeout: Duration::from_secs(30),
            log_path: &log_path,
            spawn_err: "failed to spawn the test child",
            auth_msg: "AUTH FAILED",
            timeout_msg: "TIMED OUT",
        },
        |log| log.contains(CLEAN_STDOUT),
    )
    .expect_err("an auth match must bail");
    let msg = format!("{err}");
    assert!(
        msg.contains("AUTH FAILED"),
        "err names the auth message: {msg}"
    );
    assert!(
        msg.contains(&log_path.display().to_string()),
        "err names the log path: {msg}"
    );
    let _ = std::fs::remove_file(&log_path);

    // Sleep child (no auth match) → timeout bail with timeout_msg.
    let log_path = temp_log("text-timeout");
    let _ = std::fs::remove_file(&log_path);
    let err = run_text_session(
        TextSession {
            cmd: child_cmd("sleep"),
            prompt: "ignored prompt",
            timeout: Duration::from_millis(300),
            log_path: &log_path,
            spawn_err: "failed to spawn the test child",
            auth_msg: "AUTH FAILED",
            timeout_msg: "TIMED OUT",
        },
        |_log| false,
    )
    .expect_err("a timed-out session must bail");
    assert!(
        format!("{err}").contains("TIMED OUT"),
        "err names the timeout message: {err}"
    );
    let _ = std::fs::remove_file(&log_path);
}

#[test]
fn run_init_session_clears_a_stale_artifact_before_the_run() {
    // A stale artifact from a prior run must never survive into this session. Drive
    // a child that writes NO artifact, seed a stale `out_path`, and assert the run
    // bails with `missing_msg` (proving the file was cleared, not read back).
    let log_path = temp_log("init-log");
    let out_path = temp_log("init-out");
    let _ = std::fs::remove_file(&log_path);
    std::fs::write(&out_path, "stale contents from a prior run").unwrap();

    let err = run_init_session(
        JsonSession {
            cmd: child_cmd("clean"),
            prompt: "ignored prompt",
            timeout: Duration::from_secs(30),
            log_path: &log_path,
            out_path: &out_path,
            spawn_err: "failed to spawn the test child",
            auth_msg: "AUTH FAILED",
            timeout_msg: "TIMED OUT",
            missing_msg: "NO ARTIFACT",
        },
        |_log| false,
        |raw| Ok::<String, anyhow::Error>(raw.to_string()),
    )
    .expect_err("the stale artifact was cleared, so the read must fail with missing_msg");

    assert!(
        format!("{err}").contains("NO ARTIFACT"),
        "err names the missing-artifact message: {err}"
    );
    assert!(
        !out_path.exists(),
        "the stale artifact was removed before the run"
    );
    let _ = std::fs::remove_file(&log_path);
}
