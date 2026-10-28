use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use ralphy_adapter_support::{
    run_init_session, run_json_session_held, run_text_session, run_text_session_held, HeadlessCall,
    JsonSession, PromptGate, TextSession,
};

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

const HELD_PROMPT: &str = "the held prompt";

/// A `record-stdin` style child that records into `out` and announces its first
/// hook event in `status`.
fn recording_cmd(mode: &str, out: &Path, status: &Path) -> std::process::Command {
    let mut cmd = child_cmd(mode);
    cmd.arg(out).env("RALPHY_STATUS_FILE", status);
    cmd
}

/// A gate that waits up to `limit` for a `SessionStart` line in `status`. When it
/// arrives, the gate asserts the child has not yet been given the prompt: `out` is
/// written by the child only after stdin reached EOF.
fn session_start_gate<'a>(status: PathBuf, out: PathBuf, limit: Duration) -> PromptGate<'a> {
    Box::new(move || {
        let started = Instant::now();
        while started.elapsed() < limit {
            let seen = std::fs::read_to_string(&status).unwrap_or_default();
            if seen.contains("\"SessionStart\"") {
                anyhow::ensure!(
                    !out.exists(),
                    "the prompt reached the child before the gate released it"
                );
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        anyhow::bail!("no SessionStart line within {limit:?}")
    })
}

#[test]
fn a_held_prompt_reaches_the_child_after_the_first_hook_event() {
    let log = temp_log("held-ok-log");
    let out = temp_log("held-ok-out");
    let status = temp_log("held-ok-status");
    for p in [&out, &status] {
        let _ = std::fs::remove_file(p);
    }

    let run = HeadlessCall::new(
        recording_cmd("session-start-then-record", &out, &status),
        HELD_PROMPT,
        Duration::from_secs(30),
        &log,
    )
    .hold_prompt(session_start_gate(
        status.clone(),
        out.clone(),
        Duration::from_secs(3),
    ))
    .run()
    .expect("the gate saw SessionStart, so the call succeeds");

    assert!(run.exited_cleanly, "the child read the prompt and exited 0");
    assert_eq!(std::fs::read_to_string(&out).unwrap(), HELD_PROMPT);
    for p in [&log, &out, &status] {
        let _ = std::fs::remove_file(p);
    }
}

#[test]
fn a_closed_gate_returns_its_error_and_sends_no_prompt() {
    let log = temp_log("held-closed-log");
    let out = temp_log("held-closed-out");
    let status = temp_log("held-closed-status");
    for p in [&out, &status] {
        let _ = std::fs::remove_file(p);
    }

    let started = Instant::now();
    let err = HeadlessCall::new(
        recording_cmd("record-stdin", &out, &status),
        HELD_PROMPT,
        Duration::from_secs(30),
        &log,
    )
    .hold_prompt(session_start_gate(
        status.clone(),
        out.clone(),
        Duration::from_secs(1),
    ))
    .run()
    .expect_err("no SessionStart line ever comes, so the gate refuses");
    let elapsed = started.elapsed();

    let chain = format!("{err:#}");
    assert!(
        chain.contains("no SessionStart line within"),
        "the chain holds the gate's message: {chain}"
    );
    assert!(
        chain.contains("the prompt was held and not sent"),
        "the chain says the prompt was held: {chain}"
    );
    // The child writes `out` only after stdin reaches EOF, and the kill came first.
    assert!(!out.exists(), "the child was killed before it read stdin");
    assert!(
        elapsed < Duration::from_secs(10),
        "the call returned promptly (took {elapsed:?})"
    );
    for p in [&log, &out, &status] {
        let _ = std::fs::remove_file(p);
    }
}

#[test]
fn a_held_session_returns_the_gate_error() {
    let refuse = || -> PromptGate<'static> { Box::new(|| anyhow::bail!("gate said no")) };

    let log = temp_log("held-text-log");
    let err = run_text_session_held(
        TextSession {
            cmd: child_cmd("sleep"),
            prompt: HELD_PROMPT,
            timeout: Duration::from_secs(30),
            log_path: &log,
            spawn_err: "failed to spawn the test child",
            auth_msg: "AUTH FAILED",
            timeout_msg: "TIMED OUT",
        },
        refuse(),
        |_log| false,
    )
    .expect_err("a refusing gate fails the text session");
    let chain = format!("{err:#}");
    assert!(
        chain.contains("gate said no"),
        "text session chain: {chain}"
    );
    assert!(!chain.contains("TIMED OUT"), "not a timeout: {chain}");

    let log = temp_log("held-json-log");
    let out = temp_log("held-json-out");
    let err = run_json_session_held(
        JsonSession {
            cmd: child_cmd("sleep"),
            prompt: HELD_PROMPT,
            timeout: Duration::from_secs(30),
            log_path: &log,
            out_path: &out,
            spawn_err: "failed to spawn the test child",
            auth_msg: "AUTH FAILED",
            timeout_msg: "TIMED OUT",
            missing_msg: "NO ARTIFACT",
        },
        refuse(),
        |_log| false,
        |raw| Ok::<String, anyhow::Error>(raw.to_string()),
    )
    .expect_err("a refusing gate fails the json session");
    let chain = format!("{err:#}");
    assert!(
        chain.contains("gate said no"),
        "json session chain: {chain}"
    );
    assert!(
        !chain.contains("NO ARTIFACT"),
        "not a missing artifact: {chain}"
    );
}

#[test]
fn a_refused_prompt_gets_no_spawn_context() {
    const SPAWN_ERR: &str = "failed to spawn the test child (is it on PATH?)";
    let session = |cmd: std::process::Command, log: &Path| -> anyhow::Result<String> {
        run_text_session_held(
            TextSession {
                cmd,
                prompt: HELD_PROMPT,
                timeout: Duration::from_secs(30),
                log_path: log,
                spawn_err: SPAWN_ERR,
                auth_msg: "AUTH FAILED",
                timeout_msg: "TIMED OUT",
            },
            Box::new(|| anyhow::bail!("gate said no")),
            |_log| false,
        )
    };

    let log = temp_log("held-refused-log");
    let err = session(child_cmd("sleep"), &log).expect_err("the gate refuses");
    let chain = format!("{err:#}");
    assert!(
        chain.contains("gate said no"),
        "the gate's message: {chain}"
    );
    assert!(!chain.contains(SPAWN_ERR), "the child started: {chain}");

    // A child that never starts still gets the spawn context.
    let mut missing = std::process::Command::new("ralphy-no-such-program-for-this-test");
    missing
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let err = session(missing, &log).expect_err("the program does not exist");
    assert!(
        format!("{err:#}").starts_with(SPAWN_ERR),
        "a spawn failure: {err:#}"
    );
    let _ = std::fs::remove_file(&log);
}
