//! `ralphy hook status` end to end (ADR-0059 §3): the real binary, a vendor
//! payload on stdin, `{}` on stdout FIRST and exit 0 whether the file can be
//! written or not; one newline-terminated JSON line appended per call; and a
//! no-op with `RALPHY_STATUS_FILE` unset.

use std::io::Write;
use std::process::{Command, Stdio};

fn run(env: Option<&std::path::Path>, payload: &str) -> (i32, String) {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_ralphy"));
    cmd.args(["hook", "status"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    match env {
        Some(p) => {
            cmd.env("RALPHY_STATUS_FILE", p);
        }
        None => {
            cmd.env_remove("RALPHY_STATUS_FILE");
        }
    }
    let mut child = cmd.spawn().expect("spawning ralphy hook status");
    child
        .stdin
        .take()
        .expect("piped stdin")
        .write_all(payload.as_bytes())
        .expect("writing the payload");
    let out = child.wait_with_output().expect("waiting for the hook");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).trim().to_string(),
    )
}

#[test]
fn hook_status_appends_one_line_per_call_and_always_answers_empty_json() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("agent-status.jsonl");

    let (code, stdout) = run(
        Some(&file),
        r#"{"hook_event_name":"PreToolUse","tool_name":"AskUserQuestion","tool_input":{"questions":[{"question":"which port?"}]}}"#,
    );
    assert_eq!((code, stdout.as_str()), (0, "{}"));
    let (code, stdout) = run(
        Some(&file),
        r#"{"hook_event_name":"Stop","stop_hook_active":true}"#,
    );
    assert_eq!((code, stdout.as_str()), (0, "{}"));

    let text = std::fs::read_to_string(&file).unwrap();
    let lines: Vec<serde_json::Value> = text
        .lines()
        .map(|l| serde_json::from_str(l).expect("one JSON object per line"))
        .collect();
    assert_eq!(lines.len(), 2, "{text}");
    assert_eq!(lines[0]["event"], "PreToolUse");
    assert_eq!(lines[0]["tool_name"], "AskUserQuestion");
    assert_eq!(
        lines[0]["tool_input"]["questions"][0]["question"],
        "which port?"
    );
    assert!(lines[0]["ts"].as_str().is_some_and(|t| !t.is_empty()));
    assert_eq!(lines[1]["event"], "Stop");
    assert!(lines[1].get("interrupted").is_none(), "{text}");
    assert!(text.ends_with('\n'));

    // Unset: nothing written, still `{}` and 0.
    let (code, stdout) = run(None, r#"{"hook_event_name":"Stop"}"#);
    assert_eq!((code, stdout.as_str()), (0, "{}"));
    assert_eq!(std::fs::read_to_string(&file).unwrap(), text, "unchanged");

    // Unwritable path: still `{}` and 0 — the hook never fails the agent.
    let (code, stdout) = run(
        Some(&dir.path().join("no-such-dir").join("x.jsonl")),
        r#"{"hook_event_name":"Stop"}"#,
    );
    assert_eq!((code, stdout.as_str()), (0, "{}"));
}
