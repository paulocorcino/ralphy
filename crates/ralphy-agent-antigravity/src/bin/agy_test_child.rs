//! A fake `agy` for the adapter's integration tests (`tests/agent.rs`). It
//! answers `--version`, `models` and the quota read-out, and plays one headless
//! session: it reads NDJSON lines from stdin, appends a `SessionStart` line to
//! `$RALPHY_STATUS_FILE` after the first one (as the status hook would), and
//! records the order of its stdin lines and that line in `<root>/order.log`.
//!
//! The scenario is the text of `<root>/scenario`: `done` (the default), `plan`
//! (`done` that also writes `.ralphy/plan.md`), `logged_out`, `limit` or
//! `no_hook`. The adapter builds the child's environment, and a test may not
//! change its own, so the scenario rides in the root the test chooses.

use std::fs;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::mpsc::{self, Receiver};
use std::sync::Mutex;
use std::time::Duration;

/// What `--version` prints.
const VERSION: &str = "1.5.6-test";

/// The reset time of the spent bucket in the quota read-out.
const RESET: &str = "2026-11-12T20:00:00Z";

/// The conversation id of the session.
const CONVERSATION: &str = "00000000-0000-4000-8000-0000000000aa";

/// The line written to the status file and to the order log.
const SESSION_START: &str = "SessionStart";

/// Serializes the two threads that append to the order log.
static ORDER: Mutex<()> = Mutex::new(());

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--version") {
        return say(VERSION);
    }
    let root = root_of(&args);
    let scenario = fs::read_to_string(root.join("scenario")).unwrap_or_default();
    let scenario = scenario.trim();
    if args.iter().any(|a| a == "models") {
        return models(scenario);
    }
    if args.iter().any(|a| a == "/quota") {
        return quota();
    }
    session(&root, scenario)
}

/// The `--gemini_dir=<root>` the adapter passed.
fn root_of(args: &[String]) -> PathBuf {
    args.iter()
        .find_map(|a| a.strip_prefix("--gemini_dir="))
        .map(PathBuf::from)
        .expect("the adapter passes --gemini_dir on every call")
}

fn say(line: &str) -> ExitCode {
    let mut out = std::io::stdout().lock();
    writeln!(out, "{line}").expect("the test child can write stdout");
    out.flush().expect("the test child can flush stdout");
    ExitCode::SUCCESS
}

fn complain(line: &str) {
    let mut err = std::io::stderr().lock();
    writeln!(err, "{line}").expect("the test child can write stderr");
    err.flush().expect("the test child can flush stderr");
}

fn models(scenario: &str) -> ExitCode {
    if scenario == "logged_out" {
        complain("Please sign in to view available models.");
        return ExitCode::from(1);
    }
    say("gemini-3.8-flash-low\tGemini 3.8 Flash (Low)")
}

fn quota() -> ExitCode {
    let report = serde_json::json!({ "command": { "data": { "groups": [{
        "name": "Gemini Models",
        "buckets": [{ "remaining_fraction": 0.0, "reset_time": RESET }],
    }]}}});
    say(&report.to_string())
}

fn session(root: &Path, scenario: &str) -> ExitCode {
    if scenario == "logged_out" {
        complain("Authentication required. Please visit the URL to log in:");
        std::thread::sleep(Duration::from_secs(60));
        return ExitCode::from(1);
    }
    let order = root.join("order.log");
    let lines = read_stdin(order.clone());
    say(&serde_json::json!({ "event": "init", "conversation_id": CONVERSATION }).to_string());
    if lines.recv().is_err() {
        return ExitCode::from(2);
    }
    // A task written before the proof shows up in the order log first.
    std::thread::sleep(Duration::from_millis(300));
    if scenario != "no_hook" {
        record(&order, SESSION_START);
        append(
            &status_file(),
            "{\"event\":\"SessionStart\",\"ts\":\"2026-11-12T10:00:00Z\"}",
        );
    }
    turn(100, 10, 5);
    result("SUCCESS", "READY\n", None);
    if lines.recv().is_err() {
        return ExitCode::SUCCESS;
    }
    task(scenario)
}

fn task(scenario: &str) -> ExitCode {
    if scenario == "limit" {
        result("ERROR", "", Some("RESOURCE_EXHAUSTED"));
        complain("AGY_ERROR: {\"short_error\":\"quota\",\"status\":\"RESOURCE_EXHAUSTED\"}");
        return ExitCode::from(1);
    }
    if scenario == "plan" {
        fs::create_dir_all(".ralphy").expect("the test child can make .ralphy");
        fs::write(".ralphy/plan.md", "# Plan\n\n- [ ] one step\n")
            .expect("the test child can write the plan");
    }
    turn(200, 20, 7);
    result("SUCCESS", "Work done.\nRALPHY_DONE_EXIT\n", None);
    ExitCode::SUCCESS
}

/// Read stdin on a thread: each line is recorded in the order log as it
/// arrives, then handed to the session.
fn read_stdin(order: PathBuf) -> Receiver<String> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines() {
            let line = line.expect("the test child reads UTF-8 stdin");
            record(&order, &format!("stdin: {line}"));
            if tx.send(line).is_err() {
                return;
            }
        }
    });
    rx
}

fn status_file() -> PathBuf {
    std::env::var_os("RALPHY_STATUS_FILE")
        .map(PathBuf::from)
        .expect("the adapter names the status file")
}

fn record(order: &Path, line: &str) {
    let _held = ORDER.lock().expect("the order lock is never poisoned");
    append(order, line);
}

fn append(path: &Path, line: &str) {
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .expect("the test child can open its log");
    writeln!(file, "{line}").expect("the test child can append");
}

/// One finished model call with its usage.
fn turn(input: u64, cache_read: u64, output: u64) {
    let step = serde_json::json!({ "event": "step_update", "step_update": {
        "conversation_id": CONVERSATION,
        "state": "DONE",
        "step_type": "agent_response",
        "usage": {
            "input_tokens": input,
            "cache_read_tokens": cache_read,
            "output_tokens": output,
        },
    }});
    say(&step.to_string());
}

fn result(status: &str, response: &str, error: Option<&str>) {
    let line = serde_json::json!({ "event": "result", "result": {
        "conversation_id": CONVERSATION,
        "status": status,
        "response": response,
        "error": error,
    }});
    say(&line.to_string());
}
