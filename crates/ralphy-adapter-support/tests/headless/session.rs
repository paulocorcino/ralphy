use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use ralphy_adapter_support::{
    run_init_session, run_json_session, run_json_session_held, run_text_session,
    run_text_session_held, HeadlessCall, JsonSession, PromptGate, TextSession,
};

use super::*;

#[test]
fn run_text_session_returns_the_log_and_bails_on_auth_then_timeout() {
    // Clean child: no auth match, no timeout → returns the combined log.
    let log_path = temp_log("text-clean");
    remove_scratch(&log_path);
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
    remove_scratch(&log_path);

    // The auth detector matches the log, but the child exited 0: the log is only
    // a log, and the session keeps its result.
    let log_path = temp_log("text-auth");
    remove_scratch(&log_path);
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
        |log| log.contains(CLEAN_STDOUT),
    )
    .expect("a child that exited 0 is not an auth failure");
    assert!(log.contains(CLEAN_STDOUT), "the log is returned: {log}");
    remove_scratch(&log_path);

    // Sleep child (no auth match) → timeout bail with timeout_msg.
    let log_path = temp_log("text-timeout");
    remove_scratch(&log_path);
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
    remove_scratch(&log_path);
}

#[test]
fn run_init_session_clears_a_stale_artifact_before_the_run() {
    // A stale artifact from a prior run must never survive into this session. Drive
    // a child that writes NO artifact, seed a stale `out_path`, and assert the run
    // bails with `missing_msg` (proving the file was cleared, not read back).
    let log_path = temp_log("init-log");
    let out_path = temp_log("init-out");
    remove_scratch(&log_path);
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
    remove_scratch(&log_path);
}

// These mirror the constants in `src/bin/headless_test_child.rs`.
const AUTH_TEXT: &str = "not-logged-in: please run login";
const ARTIFACT_TEXT: &str = "the-artifact";

fn has_auth_text(log: &str) -> bool {
    log.contains(AUTH_TEXT)
}

/// Run `child_cmd(mode)` plus `args` as a text session whose detector is
/// [`has_auth_text`].
fn auth_text_session(
    mode: &str,
    args: &[&str],
    timeout: Duration,
    log: &Path,
) -> anyhow::Result<String> {
    let mut cmd = child_cmd(mode);
    cmd.args(args);
    run_text_session(
        TextSession {
            cmd,
            prompt: "ignored prompt",
            timeout,
            log_path: log,
            spawn_err: "failed to spawn the test child",
            auth_msg: "AUTH FAILED",
            timeout_msg: "TIMED OUT",
        },
        has_auth_text,
    )
}

#[test]
fn a_clean_text_session_with_auth_text_returns_its_log() {
    let log_path = temp_log("clean-auth-text");
    remove_scratch(&log_path);
    let log = auth_text_session("auth-exit", &["0"], Duration::from_secs(30), &log_path)
        .expect("a child that exits 0 keeps its result, whatever its log quotes");
    assert!(log.contains(AUTH_TEXT), "the log is returned: {log}");
    remove_scratch(&log_path);
}

#[test]
fn a_clean_json_session_with_auth_text_returns_its_artifact() {
    let log_path = temp_log("clean-auth-json-log");
    let out_path = temp_log("clean-auth-json-out");
    remove_scratch(&log_path);
    remove_scratch(&out_path);
    let mut cmd = child_cmd("artifact-then-auth");
    cmd.arg(&out_path);
    let artifact = run_json_session(
        JsonSession {
            cmd,
            prompt: "ignored prompt",
            timeout: Duration::from_secs(30),
            log_path: &log_path,
            out_path: &out_path,
            spawn_err: "failed to spawn the test child",
            auth_msg: "AUTH FAILED",
            timeout_msg: "TIMED OUT",
            missing_msg: "NO ARTIFACT",
        },
        has_auth_text,
        |raw| Ok::<String, anyhow::Error>(raw.to_string()),
    )
    .expect("a session that wrote its artifact and exited 0 keeps it");
    assert_eq!(artifact, ARTIFACT_TEXT);
    remove_scratch(&log_path);
    remove_scratch(&out_path);
}

#[test]
fn a_failed_session_with_auth_text_returns_the_auth_error() {
    let log_path = temp_log("failed-auth-text");
    remove_scratch(&log_path);
    let err = auth_text_session("auth-exit", &["1"], Duration::from_secs(30), &log_path)
        .expect_err("a child that exits 1 with a login error is an auth failure");
    assert_eq!(
        format!("{err}"),
        format!("AUTH FAILED (see {})", log_path.display())
    );
    remove_scratch(&log_path);
}

#[test]
fn a_timed_out_session_with_auth_text_returns_the_auth_error() {
    let log_path = temp_log("timeout-auth-text");
    remove_scratch(&log_path);
    let err = auth_text_session(
        "auth-then-sleep",
        &[],
        Duration::from_millis(1500),
        &log_path,
    )
    .expect_err("a killed child with a login error in its log is an auth failure");
    assert_eq!(
        format!("{err}"),
        format!("AUTH FAILED (see {})", log_path.display()),
        "the auth error comes before the timeout error"
    );
    remove_scratch(&log_path);
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
        remove_scratch(p);
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
        remove_scratch(p);
    }
}

#[test]
fn a_closed_gate_returns_its_error_and_sends_no_prompt() {
    let log = temp_log("held-closed-log");
    let out = temp_log("held-closed-out");
    let status = temp_log("held-closed-status");
    for p in [&out, &status] {
        remove_scratch(p);
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
        remove_scratch(p);
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
    remove_scratch(&log);
}

#[test]
fn the_wall_deadline_is_read_before_the_idle_window_on_one_tick() {
    let log = temp_log("wall-before-idle");
    // The gate holds the prompt long enough that, on the first tick, both clocks
    // have run out: the wall deadline (zero) and the 1 ms idle window.
    let slow_gate: PromptGate<'_> = Box::new(|| {
        std::thread::sleep(Duration::from_millis(30));
        Ok(())
    });

    let run = HeadlessCall::new(child_cmd("sleep"), "ignored prompt", Duration::ZERO, &log)
        .idle_window(Duration::from_millis(1))
        .hold_prompt(slow_gate)
        .run()
        .expect("a reaped run returns its output");

    assert!(run.timed_out, "the wall deadline reaped the child");
    assert!(
        !run.idle_killed,
        "both clocks had expired, and the wall clock owns the tick"
    );
    assert!(run.exit_code.is_none(), "a killed child has no exit code");
    remove_scratch(&log);
}
