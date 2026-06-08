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

use anyhow::Result;
use regex::Regex;
use serde_json::Value;

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
    let re = Regex::new(r"RALPHY_BLOCKED_EXIT\s*(.*)").expect("valid regex");
    if let Some(caps) = re.captures(msg) {
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
