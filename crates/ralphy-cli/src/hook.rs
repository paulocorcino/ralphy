//! The `ralphy hook stop` Stop-hook handler — a port of `stop_exit_hook.ps1`.
//!
//! Claude Code runs this each time the interactive execution session finishes a
//! turn and would wait for user input. We do NOT kill anything here: we only
//! record the agent's exit signal (`RALPHY_DONE_EXIT` / `RALPHY_BLOCKED_EXIT`) to
//! the path in `$RALPHY_FLAG_FILE`. The orchestrator polls that file and owns the
//! actual process termination (it holds the PTY).
//!
//! The hook is a no-op unless `RALPHY_FLAG_FILE` is set, so it is harmless if it
//! ever leaks into a normal interactive session. It always exits 0.
//!
//! The parsing is factored into [`classify_stop`], a pure function over the hook
//! payload plus a transcript-reader closure, so it unit-tests against fixture
//! JSON without touching the filesystem or the environment.

use std::fs;
use std::io::Read;
use std::path::Path;
use std::sync::LazyLock;

use anyhow::Result;
use regex::Regex;
use serde_json::Value;

/// Captures the reason trailing a `RALPHY_BLOCKED_EXIT` sentinel. Compiled once.
static BLOCKED_EXIT_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"RALPHY_BLOCKED_EXIT\s*(.*)").expect("valid regex"));

/// What the Stop hook decided to write to the flag file. Rendered with
/// [`FlagWrite::contents`]; the orchestrator reads it back verbatim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FlagWrite {
    /// The agent emitted `RALPHY_DONE_EXIT`.
    Done,
    /// The agent emitted `RALPHY_BLOCKED_EXIT <reason>`.
    Blocked(String),
}

impl FlagWrite {
    /// The exact bytes written to `$RALPHY_FLAG_FILE` (matches the ps1 oracle).
    pub fn contents(&self) -> String {
        match self {
            FlagWrite::Done => "DONE".to_string(),
            FlagWrite::Blocked(reason) => format!("BLOCKED {reason}"),
        }
    }
}

/// Decide what (if anything) to record from a Stop-hook payload.
///
/// Pulls the last assistant message from the payload's `last_assistant_message`
/// field, falling back to `read_transcript(transcript_path)` when that field is
/// absent or blank (older/newer CLIs differ on whether it is included). Returns
/// `None` when neither sentinel is present — the caller then writes nothing.
pub fn classify_stop(
    payload: &str,
    read_transcript: impl Fn(&str) -> Option<String>,
) -> Option<FlagWrite> {
    let value: Value = serde_json::from_str(payload).ok()?;

    let mut msg = value
        .get("last_assistant_message")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();

    if msg.trim().is_empty() {
        if let Some(path) = value.get("transcript_path").and_then(Value::as_str) {
            msg = read_transcript(path).unwrap_or_default();
        }
    }

    classify_message(&msg)
}

/// Map a last-assistant message to a flag write. `DONE` is checked first, exactly
/// as `stop_exit_hook.ps1` does, so a message carrying both sentinels reports done.
fn classify_message(msg: &str) -> Option<FlagWrite> {
    if msg.contains("RALPHY_DONE_EXIT") {
        return Some(FlagWrite::Done);
    }
    if let Some(caps) = BLOCKED_EXIT_RE.captures(msg) {
        let reason = caps.get(1).map_or("", |m| m.as_str()).trim();
        return Some(FlagWrite::Blocked(reason.to_string()));
    }
    None
}

/// Read the last `assistant` `text` block out of a transcript JSONL file. Returns
/// `None` if the path is missing/unreadable; version-robust like the ps1 oracle
/// (skips lines that don't parse, keeps the last text block seen).
pub fn read_transcript_last_assistant(path: &str) -> Option<String> {
    let p = Path::new(path);
    if !p.exists() {
        return None;
    }
    let body = fs::read_to_string(p).ok()?;
    let mut text: Option<String> = None;
    for line in body.lines() {
        let Ok(obj) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if obj.get("type").and_then(Value::as_str) != Some("assistant") {
            continue;
        }
        let Some(content) = obj.get("message").and_then(|m| m.get("content")) else {
            continue;
        };
        if let Some(blocks) = content.as_array() {
            for block in blocks {
                if block.get("type").and_then(Value::as_str) == Some("text") {
                    if let Some(t) = block.get("text").and_then(Value::as_str) {
                        if !t.is_empty() {
                            text = Some(t.to_string());
                        }
                    }
                }
            }
        }
    }
    text
}

/// Run the `hook post` subcommand (PostToolUse, matcher `Bash`): close the
/// timing loop the guard's verification-cost gate opened. The PreToolUse guard
/// stamps when a plan `## Verify` command starts; this hook, firing right after
/// the tool returns, records the elapsed wall clock as that command's durable
/// cost — so even the first session in a fresh repo learns an expensive suite's
/// price after paying it once. Best-effort and always `Ok`: cost knowledge is
/// an optimization, never worth failing a session over.
pub fn run_post_hook() -> Result<()> {
    let mut payload = String::new();
    if std::io::stdin().read_to_string(&mut payload).is_err() {
        return Ok(());
    }
    let Ok(value) = serde_json::from_str::<Value>(&payload) else {
        return Ok(());
    };
    if value.get("tool_name").and_then(Value::as_str) != Some("Bash") {
        return Ok(());
    }
    let Some(command) = value
        .get("tool_input")
        .and_then(|t| t.get("command"))
        .and_then(Value::as_str)
    else {
        return Ok(());
    };
    let Some(root) = crate::guard::project_root(&value) else {
        return Ok(());
    };
    let Ok(plan_md) = fs::read_to_string(root.join(".ralphy").join("plan.md")) else {
        return Ok(());
    };
    ralphy_core::cmdcost::note_finish(&root, command, &plan_md);
    Ok(())
}

/// Run the `hook status` subcommand (ADR-0059 §3): read the vendor's hook
/// payload on stdin and append one JSON line
/// `{ "event", "tool_name", "tool_input", "ts" }` to the file named by
/// `$RALPHY_STATUS_FILE`, which the adapter (or the daemon) tails and folds
/// into an agent state. Three things it must never do: block the agent,
/// fail it, or say anything on stdout but `{}` — a permission hook with an
/// empty stdout fails CLOSED in the vendor, so the `{}` is printed FIRST,
/// before any read or write that could go wrong. A no-op when the variable
/// is unset, like the Stop hook, so a settings file that leaks into a
/// non-Ralphy session does nothing. A write error goes to stderr; the exit
/// is still 0.
pub fn run_status_hook() -> Result<()> {
    println!("{{}}");
    let Ok(path) = std::env::var("RALPHY_STATUS_FILE") else {
        return Ok(());
    };
    if path.is_empty() {
        return Ok(());
    }
    let mut payload = String::new();
    if std::io::stdin().read_to_string(&mut payload).is_err() {
        return Ok(());
    }
    let line = status_line(&payload, &chrono::Local::now().to_rfc3339());
    if let Err(e) = append_line(Path::new(&path), &line) {
        eprintln!("ralphy hook status: could not append to {path}: {e:#}");
    }
    Ok(())
}

/// The one tool whose input the status line keeps: what it asks is the
/// `waiting` detail (ADR-0059 §1).
const ASKS_THE_OPERATOR: &str = "AskUserQuestion";

/// The one line `hook status` appends, from the vendor's payload: the hook
/// event name, the tool it concerns (when any), the tool's input ONLY when
/// the fold reads it — an `AskUserQuestion`'s questions — and the wall
/// clock. Any other tool's input (a `Write`'s whole file, a `Bash` command
/// with a secret in it) is dropped here rather than landing on disk a second
/// time for nothing to read. A payload that is not JSON still yields a line,
/// with `event` empty, so a vendor change never silences the file — the fold
/// treats it as noise.
pub fn status_line(payload: &str, ts: &str) -> String {
    let value: Value = serde_json::from_str(payload).unwrap_or(Value::Null);
    let event = value
        .get("hook_event_name")
        .and_then(Value::as_str)
        .unwrap_or("");
    let tool_name = value.get("tool_name").and_then(Value::as_str);
    let tool_input = match tool_name {
        Some(ASKS_THE_OPERATOR) => value.get("tool_input").cloned().unwrap_or(Value::Null),
        _ => Value::Null,
    };
    // Not carried: `stop_hook_active`. It means "already continuing because a
    // Stop hook said so" — our own sentinel sets it on every retried turn —
    // and never an interrupt; the vendor fires no Stop on an interrupt at all
    // (ADR-0059, amendment 2026-09-16).
    serde_json::json!({
        "event": event,
        "tool_name": tool_name,
        "tool_input": tool_input,
        "ts": ts,
    })
    .to_string()
}

/// ONE `write_all` per line, newline included: Claude Code runs parallel tool
/// calls, so two `hook status` processes can be appending at once, and an
/// `O_APPEND` handle keeps each write contiguous but not two writes in a row —
/// line and newline as separate syscalls could interleave as `{A}{B}\n\n` and
/// cost both events.
fn append_line(path: &Path, line: &str) -> std::io::Result<()> {
    use std::io::Write;
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    file.write_all(format!("{line}\n").as_bytes())
}

/// Run the `hook stop` subcommand: read the payload from stdin, classify it, and
/// write the flag file named by `$RALPHY_FLAG_FILE`. No-op when that env var is
/// unset. Always returns `Ok` — the hook must never fail the session.
pub fn run_stop_hook() -> Result<()> {
    let Ok(flag) = std::env::var("RALPHY_FLAG_FILE") else {
        return Ok(());
    };
    if flag.is_empty() {
        return Ok(());
    }

    let mut payload = String::new();
    if std::io::stdin().read_to_string(&mut payload).is_err() {
        return Ok(());
    }

    if let Some(write) = classify_stop(&payload, read_transcript_last_assistant) {
        let _ = fs::write(&flag, write.contents());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ADR-0059 §3: the status line carries the event, the tool, its input
    /// and the clock — and NOT `stop_hook_active`, which our own sentinel
    /// raises on every retried turn; a non-JSON payload still yields a line
    /// with an empty event.
    #[test]
    fn status_line_carries_event_tool_input_and_ts() {
        let line = status_line(
            r#"{"hook_event_name":"PreToolUse","tool_name":"AskUserQuestion","tool_input":{"questions":[{"question":"which port?"}]},"session_id":"s"}"#,
            "2026-09-15T10:00:00-03:00",
        );
        let v: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(v["event"], "PreToolUse");
        assert_eq!(v["tool_name"], "AskUserQuestion");
        assert_eq!(v["tool_input"]["questions"][0]["question"], "which port?");
        assert_eq!(v["ts"], "2026-09-15T10:00:00-03:00");
        assert!(!line.contains('\n'), "one line, no newline inside");

        let stop = status_line(r#"{"hook_event_name":"Stop","stop_hook_active":true}"#, "t");
        let v: Value = serde_json::from_str(&stop).unwrap();
        assert_eq!(v["event"], "Stop");
        assert_eq!(v["tool_name"], Value::Null);
        assert!(
            v.get("interrupted").is_none() && v.get("stop_hook_active").is_none(),
            "a sentinel-retried turn is not an interrupt: {v}"
        );

        let junk: Value = serde_json::from_str(&status_line("not json", "t")).unwrap();
        assert_eq!(junk["event"], "");
    }

    /// Only an `AskUserQuestion` keeps its `tool_input`: a `Bash` command or a
    /// `Write`'s content is not written to the status file (review L3).
    #[test]
    fn status_line_drops_the_input_of_every_other_tool() {
        let bash = status_line(
            r#"{"hook_event_name":"PreToolUse","tool_name":"Bash","tool_input":{"command":"curl -H 'Authorization: Bearer SECRET'"}}"#,
            "t",
        );
        assert!(!bash.contains("SECRET"), "{bash}");
        let v: Value = serde_json::from_str(&bash).unwrap();
        assert_eq!(v["tool_name"], "Bash");
        assert_eq!(v["tool_input"], Value::Null);
        let write = status_line(
            r#"{"hook_event_name":"PostToolUse","tool_name":"Write","tool_input":{"content":"whole file"}}"#,
            "t",
        );
        assert!(!write.contains("whole file"), "{write}");
    }

    /// The append is a real append: two lines, in order, newline-terminated.
    #[test]
    fn append_line_appends_newline_terminated_lines() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("agent-status.jsonl");
        append_line(&path, "{\"a\":1}").unwrap();
        append_line(&path, "{\"b\":2}").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "{\"a\":1}\n{\"b\":2}\n");
    }

    /// Inline `last_assistant_message` carrying the DONE sentinel is recorded
    /// without ever consulting the transcript.
    #[test]
    fn inline_done_sentinel() {
        let payload = r#"{"last_assistant_message":"all set\nRALPHY_DONE_EXIT"}"#;
        let got = classify_stop(payload, |_| panic!("should not read transcript"));
        assert_eq!(got, Some(FlagWrite::Done));
        assert_eq!(got.unwrap().contents(), "DONE");
    }

    /// A blank inline field falls back to the transcript reader.
    #[test]
    fn transcript_fallback_when_inline_blank() {
        let payload = r#"{"last_assistant_message":"  ","transcript_path":"/x/t.jsonl"}"#;
        let got = classify_stop(payload, |p| {
            assert_eq!(p, "/x/t.jsonl");
            Some("done now RALPHY_DONE_EXIT".to_string())
        });
        assert_eq!(got, Some(FlagWrite::Done));
    }

    /// A missing inline field also falls back to the transcript.
    #[test]
    fn transcript_fallback_when_inline_absent() {
        let payload = r#"{"transcript_path":"/x/t.jsonl"}"#;
        let got = classify_stop(payload, |_| Some("RALPHY_DONE_EXIT".to_string()));
        assert_eq!(got, Some(FlagWrite::Done));
    }

    /// `BLOCKED <reason>` extraction trims the trailing reason.
    #[test]
    fn blocked_reason_extracted() {
        let payload = r#"{"last_assistant_message":"RALPHY_BLOCKED_EXIT needs a missing API key"}"#;
        let got = classify_stop(payload, |_| None);
        assert_eq!(
            got,
            Some(FlagWrite::Blocked("needs a missing API key".to_string()))
        );
        assert_eq!(got.unwrap().contents(), "BLOCKED needs a missing API key");
    }

    /// DONE wins when both sentinels appear, mirroring the ps1's check order.
    #[test]
    fn done_precedes_blocked() {
        let msg = "RALPHY_BLOCKED_EXIT reason\nRALPHY_DONE_EXIT";
        let payload = format!("{{\"last_assistant_message\":{}}}", json_str(msg));
        assert_eq!(classify_stop(&payload, |_| None), Some(FlagWrite::Done));
    }

    /// Neither sentinel present → nothing to write.
    #[test]
    fn no_sentinel_writes_nothing() {
        let payload = r#"{"last_assistant_message":"just a normal turn"}"#;
        assert_eq!(classify_stop(payload, |_| None), None);
    }

    /// Unparseable payload is swallowed (no write), as the hook fails safe.
    #[test]
    fn garbage_payload_is_none() {
        assert_eq!(classify_stop("not json", |_| None), None);
    }

    fn json_str(s: &str) -> String {
        serde_json::Value::String(s.to_string()).to_string()
    }
}
