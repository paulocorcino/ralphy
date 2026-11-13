//! The preamble: text written to the child's stdin before the prompt gate runs,
//! with stdin kept open until the gate releases the prompt.

use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use ralphy_adapter_support::{run_text_session_with_preamble, PromptGate, TextSession};

use super::*;

// These mirror the constants in `src/bin/headless_test_child.rs`.
const STDIN_LINES_ENV: &str = "RALPHY_STDIN_LINES_FILE";
const STDIN_EOF_MARKER: &str = "<stdin-eof>";

const PREAMBLE: &str = "{\"turn\":\"proof\"}\n";
const PROMPT: &str = "{\"turn\":\"task\"}\n";

/// The `stream-stdin` child, appending what it reads to `lines`.
fn streaming_cmd(lines: &Path) -> Command {
    let mut cmd = child_cmd("stream-stdin");
    cmd.env(STDIN_LINES_ENV, lines);
    cmd
}

/// A fresh scratch path for the lines file of one test.
fn lines_file(tag: &str) -> PathBuf {
    let path = temp_log(&format!("preamble-{tag}"));
    remove_scratch(&path);
    path
}

fn read_lines(lines: &Path) -> String {
    std::fs::read_to_string(lines).unwrap_or_default()
}

/// Wait up to 5s for the child to have the preamble on disk, with no prompt and
/// stdin still open. `true` when it did.
fn preamble_arrives_alone(lines: &Path) -> bool {
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(5) {
        let seen = read_lines(lines);
        if seen.contains(PREAMBLE) {
            return !seen.contains(PROMPT) && !seen.contains(STDIN_EOF_MARKER);
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    false
}

fn open_gate<'a>() -> PromptGate<'a> {
    Box::new(|| Ok(()))
}

#[test]
fn the_preamble_is_written_before_the_gate_runs() {
    let lines = lines_file("before-gate");
    let log = temp_log("preamble-before-gate-log");
    let gate_saw_preamble = Cell::new(false);

    let run = HeadlessCall::new(streaming_cmd(&lines), PROMPT, Duration::from_secs(30), &log)
        .preamble(PREAMBLE)
        .hold_prompt(Box::new(|| {
            gate_saw_preamble.set(preamble_arrives_alone(&lines));
            Ok(())
        }))
        .run()
        .expect("the open gate lets the call finish");

    assert!(
        gate_saw_preamble.get(),
        "the child had the preamble, and only it, before the gate returned: {:?}",
        read_lines(&lines)
    );
    assert!(run.exited_cleanly, "the child read to EOF and exited 0");
    remove_scratch(&lines);
    remove_scratch(&log);
}

#[test]
fn the_prompt_follows_the_preamble_then_stdin_closes() {
    let lines = lines_file("then-prompt");
    let log = temp_log("preamble-then-prompt-log");

    let run = HeadlessCall::new(streaming_cmd(&lines), PROMPT, Duration::from_secs(30), &log)
        .preamble(PREAMBLE)
        .hold_prompt(open_gate())
        .run()
        .expect("the open gate lets the call finish");

    assert!(run.exited_cleanly, "the child saw EOF and exited 0");
    assert_eq!(
        read_lines(&lines),
        format!("{PREAMBLE}{PROMPT}{STDIN_EOF_MARKER}"),
        "the preamble, then the prompt, then EOF"
    );
    remove_scratch(&lines);
    remove_scratch(&log);
}

#[test]
fn a_refused_gate_sends_the_preamble_but_never_the_prompt() {
    let lines = lines_file("refused");
    let log = temp_log("preamble-refused-log");

    let err = HeadlessCall::new(streaming_cmd(&lines), PROMPT, Duration::from_secs(30), &log)
        .preamble(PREAMBLE)
        .hold_prompt(Box::new(|| {
            anyhow::ensure!(preamble_arrives_alone(&lines), "the preamble never came");
            anyhow::bail!("gate said no")
        }))
        .run()
        .expect_err("the gate refuses the prompt");

    let chain = format!("{err:#}");
    assert!(
        chain.contains("gate said no"),
        "the gate's own error: {chain}"
    );
    assert_eq!(
        read_lines(&lines),
        PREAMBLE,
        "the preamble arrived, the prompt never did, and the child was killed before EOF"
    );
    remove_scratch(&lines);
    remove_scratch(&log);
}

#[test]
fn a_held_text_session_writes_its_preamble_first() {
    let lines = lines_file("text-session");
    let log = temp_log("preamble-text-session-log");

    run_text_session_with_preamble(
        TextSession {
            cmd: streaming_cmd(&lines),
            prompt: PROMPT,
            timeout: Duration::from_secs(30),
            log_path: &log,
            spawn_err: "SPAWN",
            auth_msg: "AUTH FAILED",
            timeout_msg: "TIMED OUT",
        },
        PREAMBLE,
        open_gate(),
        |_| false,
    )
    .expect("the open gate lets the session finish");

    let seen = read_lines(&lines);
    assert!(
        seen.starts_with(PREAMBLE),
        "the preamble comes first: {seen:?}"
    );
    assert!(seen.contains(PROMPT), "the prompt follows it: {seen:?}");
    remove_scratch(&lines);
    remove_scratch(&log);
}

#[test]
fn a_call_without_a_preamble_writes_only_the_prompt() {
    let lines = lines_file("no-preamble");
    let log = temp_log("preamble-none-log");
    let prompt = "first line\nsecond line with no newline";

    let run = HeadlessCall::new(streaming_cmd(&lines), prompt, Duration::from_secs(30), &log)
        .run()
        .expect("a plain call finishes");

    assert!(run.exited_cleanly, "the child saw EOF and exited 0");
    assert_eq!(
        read_lines(&lines),
        format!("{prompt}{STDIN_EOF_MARKER}"),
        "exactly the prompt bytes, then EOF"
    );
    remove_scratch(&lines);
    remove_scratch(&log);
}
