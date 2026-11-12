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

use regex::Regex;
use serde_json::Value;

use crate::cli::HookVendor;

/// Captures the reason trailing a `RALPHY_BLOCKED_EXIT` sentinel. Compiled once.
static BLOCKED_EXIT_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"RALPHY_BLOCKED_EXIT\s*(.*)").expect("valid regex"));

/// What the Stop hook decided to write to the flag file. Rendered with
/// [`FlagWrite::contents`]; the orchestrator reads it back verbatim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FlagWrite {
    /// The agent emitted `RALPHY_DONE_EXIT`.
    Done,
    /// The agent emitted `RALPHY_BLOCKED_EXIT <reason>`.
    Blocked(String),
}

impl FlagWrite {
    /// The exact bytes written to `$RALPHY_FLAG_FILE` (matches the ps1 oracle).
    pub(crate) fn contents(&self) -> String {
        match self {
            FlagWrite::Done => "DONE".to_owned(),
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
pub(crate) fn classify_stop(
    payload: &str,
    read_transcript: impl Fn(&str) -> Option<String>,
) -> Option<FlagWrite> {
    let value: Value = serde_json::from_str(payload).ok()?;

    let mut msg = value
        .get("last_assistant_message")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();

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
        return Some(FlagWrite::Blocked(reason.to_owned()));
    }
    None
}

/// Read the last `assistant` `text` block out of a transcript JSONL file. Returns
/// `None` if the path is missing/unreadable; version-robust like the ps1 oracle
/// (skips lines that don't parse, keeps the last text block seen).
pub(crate) fn read_transcript_last_assistant(path: &str) -> Option<String> {
    let p = Path::new(path);
    if !p.exists() {
        return None;
    }
    let body = fs::read_to_string(p).ok()?;
    let mut text: Option<String> = None;
    for line in body.lines() {
        let Some(content) = assistant_content(line) else {
            continue;
        };
        if let Some(t) = content.as_array().and_then(|blocks| last_text(blocks)) {
            text = Some(t.to_owned());
        }
    }
    text
}

/// The `message.content` of a transcript line that is an `assistant` turn;
/// `None` for a line that does not parse or is another kind of turn.
fn assistant_content(line: &str) -> Option<Value> {
    let mut obj = serde_json::from_str::<Value>(line).ok()?;
    if obj.get("type").and_then(Value::as_str) != Some("assistant") {
        return None;
    }
    obj.get_mut("message")?.get_mut("content").map(Value::take)
}

/// The last non-empty `text` block of a message's content blocks.
fn last_text(blocks: &[Value]) -> Option<&str> {
    blocks
        .iter()
        .filter(|block| block.get("type").and_then(Value::as_str) == Some("text"))
        .filter_map(|block| block.get("text").and_then(Value::as_str))
        .rfind(|t| !t.is_empty())
}

/// Run the `hook post` subcommand (`PostToolUse`, matcher `Bash`): close the
/// timing loop the guard's verification-cost gate opened. The `PreToolUse` guard
/// stamps when a plan `## Verify` command starts; this hook, firing right after
/// the tool returns, records the elapsed wall clock as that command's durable
/// cost — so even the first session in a fresh repo learns an expensive suite's
/// price after paying it once. Best-effort and always `Ok`: cost knowledge is
/// an optimization, never worth failing a session over.
pub(crate) fn run_post_hook() {
    let mut payload = String::new();
    if std::io::stdin().read_to_string(&mut payload).is_err() {
        return;
    }
    let Ok(value) = serde_json::from_str::<Value>(&payload) else {
        return;
    };
    if value.get("tool_name").and_then(Value::as_str) != Some("Bash") {
        return;
    }
    let Some(command) = value
        .get("tool_input")
        .and_then(|t| t.get("command"))
        .and_then(Value::as_str)
    else {
        return;
    };
    let Some(root) = crate::guard::project_root(&value) else {
        return;
    };
    let Ok(plan_md) = fs::read_to_string(root.join(".ralphy").join("plan.md")) else {
        return;
    };
    ralphy_core::cmdcost::note_finish(&root, command, &plan_md);
}

/// Run the `hook status` subcommand (ADR-0059 §3): read the vendor's hook
/// payload on stdin and append one status line (see [`status_line`]) to the
/// file named by `$RALPHY_STATUS_FILE`, which the adapter (or the daemon)
/// tails and folds into an agent state. Three things it must never do: block
/// the agent, fail it, or print anything the vendor does not expect. Claude
/// gets `{}` (ADR-0085 D6): a Claude permission hook with an empty stdout
/// fails CLOSED, so the `{}` is printed FIRST, before any read or write that
/// could go wrong. Every other vendor gets nothing. A no-op when the
/// variable is unset, like the Stop hook, so a settings file that leaks into
/// a non-Ralphy session does nothing. A write error goes to stderr; the exit
/// is still 0.
pub(crate) fn run_status_hook(vendor: HookVendor) {
    match vendor {
        HookVendor::Claude => outln!("{{}}"),
        HookVendor::Codex | HookVendor::OpenCode => {}
    }
    // Read stdin before every return: a vendor that writes the payload into a
    // hook that already exited gets a broken pipe.
    let mut payload = String::new();
    let read = std::io::stdin().read_to_string(&mut payload);
    let Ok(path) = std::env::var("RALPHY_STATUS_FILE") else {
        return;
    };
    if path.is_empty() || read.is_err() {
        return;
    }
    let line = status_line(vendor, &payload, &chrono::Local::now().to_rfc3339());
    if let Err(e) = append_line(Path::new(&path), &line) {
        eoutln!("ralphy hook status: could not append to {path}: {e:#}");
    }
}

/// The one line `hook status` appends, from the vendor's payload (ADR-0085
/// D8): the hook event name, the session and subagent ids, the tool and its
/// use id (when any), the question text of the vendor's question tool, the
/// notification type, and the wall clock. No tool input is kept: a `Write`'s
/// whole file or a `Bash` command with a secret in it does not land on disk a
/// second time for one string. A payload that is not JSON still yields a
/// line, with `event` empty, so a vendor change never silences the file — the
/// fold treats it as noise.
pub(crate) fn status_line(vendor: HookVendor, payload: &str, ts: &str) -> String {
    let value: Value = serde_json::from_str(payload).unwrap_or(Value::Null);
    let text = |name: &str| value.get(name).and_then(Value::as_str);
    // Not carried: `stop_hook_active`. It means "already continuing because a
    // Stop hook said so" — our own sentinel sets it on every retried turn —
    // and never an interrupt; the vendor fires no Stop on an interrupt at all
    // (ADR-0059, amendment 2026-09-16).
    serde_json::json!({
        "event": text("hook_event_name").unwrap_or(""),
        "session_id": text("session_id"),
        "agent_id": text("agent_id"),
        "tool_name": text("tool_name"),
        "tool_use_id": text("tool_use_id"),
        "question": question(vendor, &value),
        "notification_type": text("notification_type"),
        "ts": ts,
    })
    .to_string()
}

/// The question a payload asks the operator, where each vendor puts it: the
/// first question of the vendor's question tool, or the message of a Claude
/// MCP elicitation.
fn question(vendor: HookVendor, value: &Value) -> Option<&str> {
    let event = value.get("hook_event_name").and_then(Value::as_str);
    let asks = match vendor {
        HookVendor::Claude if event == Some("Elicitation") => {
            return value.get("message").and_then(Value::as_str);
        }
        HookVendor::Claude => "AskUserQuestion",
        HookVendor::Codex => "request_user_input",
        HookVendor::OpenCode => "question",
    };
    if value.get("tool_name").and_then(Value::as_str) != Some(asks) {
        return None;
    }
    value
        .get("tool_input")?
        .get("questions")?
        .as_array()?
        .first()?
        .get("question")?
        .as_str()
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
/// unset. It never fails — the hook must never fail the session.
pub(crate) fn run_stop_hook() {
    // Read stdin before every return, so a caller that writes the payload
    // never gets a broken pipe.
    let mut payload = String::new();
    let read = std::io::stdin().read_to_string(&mut payload);
    let Ok(flag) = std::env::var("RALPHY_FLAG_FILE") else {
        return;
    };
    if flag.is_empty() || read.is_err() {
        return;
    }

    if let Some(write) = classify_stop(&payload, read_transcript_last_assistant) {
        // the hook never fails the session, so an unwritable flag file is not an error
        match fs::write(&flag, write.contents()) {
            Ok(()) | Err(_) => {}
        }
    }
}

#[cfg(test)]
mod tests;
