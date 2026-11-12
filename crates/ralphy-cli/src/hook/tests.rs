use super::*;

/// ADR-0059 §3: the status line carries the event, the tool, the
/// question and the clock — and NOT `stop_hook_active`, which our own
/// sentinel raises on every retried turn; a non-JSON payload still yields
/// a line with an empty event.
#[test]
fn status_line_carries_the_event_the_question_and_ts() {
    let line = status_line(
        HookVendor::Claude,
        concat!(
            r#"{"hook_event_name":"PreToolUse","tool_name":"AskUserQuestion","#,
            r#""tool_input":{"questions":[{"question":"which port?"}]},"session_id":"s"}"#,
        ),
        "2026-09-15T10:00:00-03:00",
    );
    let v: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(v["event"], "PreToolUse");
    assert_eq!(v["tool_name"], "AskUserQuestion");
    assert_eq!(v["question"], "which port?");
    assert_eq!(v["ts"], "2026-09-15T10:00:00-03:00");
    assert!(!line.contains('\n'), "one line, no newline inside");

    let stop = status_line(
        HookVendor::Claude,
        r#"{"hook_event_name":"Stop","stop_hook_active":true}"#,
        "t",
    );
    let v: Value = serde_json::from_str(&stop).unwrap();
    assert_eq!(v["event"], "Stop");
    assert_eq!(v["tool_name"], Value::Null);
    assert!(
        v.get("interrupted").is_none() && v.get("stop_hook_active").is_none(),
        "a sentinel-retried turn is not an interrupt: {v}"
    );

    let junk: Value =
        serde_json::from_str(&status_line(HookVendor::Claude, "not json", "t")).unwrap();
    assert_eq!(junk["event"], "");
}

/// No tool input is written: a `Bash` command or a `Write`'s content does
/// not land in the status file (review L3).
#[test]
fn status_line_drops_the_input_of_every_other_tool() {
    let bash = status_line(
        HookVendor::Claude,
        concat!(
            r#"{"hook_event_name":"PreToolUse","tool_name":"Bash","#,
            r#""tool_input":{"command":"curl -H 'Authorization: Bearer SECRET'"}}"#,
        ),
        "t",
    );
    assert!(!bash.contains("SECRET"), "{bash}");
    let v: Value = serde_json::from_str(&bash).unwrap();
    assert_eq!(v["tool_name"], "Bash");
    assert!(v.get("tool_input").is_none(), "{bash}");
    let write = status_line(
        HookVendor::Claude,
        concat!(
            r#"{"hook_event_name":"PostToolUse","tool_name":"Write","#,
            r#""tool_input":{"content":"whole file"}}"#,
        ),
        "t",
    );
    assert!(!write.contains("whole file"), "{write}");
}

/// ADR-0085 D8: the line keeps the session, the subagent, the tool use,
/// the question of each vendor and the notification type, and drops the
/// tool input.
#[test]
fn status_line_keeps_the_d8_fields_and_drops_tool_input() {
    let bash = status_line(
        HookVendor::Codex,
        concat!(
            r#"{"hook_event_name":"PermissionRequest","session_id":"s1","agent_id":"a1","#,
            r#""tool_name":"Bash","tool_use_id":"u1","notification_type":"permission_prompt","#,
            r#""tool_input":{"command":"rm -rf target"}}"#,
        ),
        "t",
    );
    let v: Value = serde_json::from_str(&bash).unwrap();
    assert_eq!(v["session_id"], "s1", "{bash}");
    assert_eq!(v["agent_id"], "a1", "{bash}");
    assert_eq!(v["tool_use_id"], "u1", "{bash}");
    assert_eq!(v["notification_type"], "permission_prompt", "{bash}");
    assert!(!bash.contains("rm -rf target"), "{bash}");
    let mut keys: Vec<&String> = v.as_object().unwrap().keys().collect();
    keys.sort();
    assert_eq!(
        keys,
        [
            "agent_id",
            "event",
            "notification_type",
            "question",
            "session_id",
            "tool_name",
            "tool_use_id",
            "ts"
        ]
    );

    let asks = |tool: &str| {
        format!(
            concat!(
                r#"{{"hook_event_name":"PreToolUse","tool_name":"{}","#,
                r#""tool_input":{{"questions":[{{"question":"which color?"}}]}}}}"#,
            ),
            tool
        )
    };
    let elicitation =
        r#"{"hook_event_name":"Elicitation","mcp_server_name":"git","message":"which color?"}"#;
    for (vendor, payload) in [
        (HookVendor::Claude, asks("AskUserQuestion")),
        (HookVendor::Claude, elicitation.to_owned()),
        (HookVendor::Codex, asks("request_user_input")),
        (HookVendor::OpenCode, asks("question")),
    ] {
        let v: Value = serde_json::from_str(&status_line(vendor, &payload, "t")).unwrap();
        assert_eq!(v["question"], "which color?", "{vendor:?} {payload}");
    }
    // Another vendor's question tool is not this vendor's question.
    let other = status_line(HookVendor::Claude, &asks("request_user_input"), "t");
    let v: Value = serde_json::from_str(&other).unwrap();
    assert_eq!(v["question"], Value::Null, "{other}");
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

#[test]
fn transcript_fallback_when_inline_blank_or_absent() {
    // (case, payload)
    let rows = [
        (
            "blank inline message",
            r#"{"last_assistant_message":"  ","transcript_path":"/x/t.jsonl"}"#,
        ),
        ("no inline message", r#"{"transcript_path":"/x/t.jsonl"}"#),
    ];
    for (case, payload) in rows {
        let got = classify_stop(payload, |p| {
            assert_eq!(p, "/x/t.jsonl", "{case}");
            Some("done now RALPHY_DONE_EXIT".to_owned())
        });
        assert_eq!(got, Some(FlagWrite::Done), "{case}");
    }
}

/// `BLOCKED <reason>` extraction trims the trailing reason.
#[test]
fn blocked_reason_extracted() {
    let payload = r#"{"last_assistant_message":"RALPHY_BLOCKED_EXIT needs a missing API key"}"#;
    let got = classify_stop(payload, |_| None);
    assert_eq!(
        got,
        Some(FlagWrite::Blocked("needs a missing API key".to_owned()))
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

/// Write `lines` as a transcript JSONL file and read it back.
fn last_assistant(lines: &[&str]) -> Option<String> {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("transcript.jsonl");
    fs::write(&path, lines.join("\n")).unwrap();
    read_transcript_last_assistant(path.to_str().unwrap())
}

fn assistant(texts: &[&str]) -> String {
    let blocks: Vec<Value> = texts
        .iter()
        .map(|t| serde_json::json!({ "type": "text", "text": t }))
        .collect();
    serde_json::json!({ "type": "assistant", "message": { "content": blocks } }).to_string()
}

#[test]
fn transcript_the_last_assistant_message_wins() {
    let got = last_assistant(&[&assistant(&["first"]), &assistant(&["second"])]);
    assert_eq!(got.as_deref(), Some("second"));
}

#[test]
fn transcript_skips_an_empty_text_block() {
    let got = last_assistant(&[&assistant(&["real", ""])]);
    assert_eq!(got.as_deref(), Some("real"));
}

#[test]
fn transcript_ignores_a_user_line() {
    let user = r#"{"type":"user","message":{"content":[{"type":"text","text":"from the user"}]}}"#;
    let got = last_assistant(&[&assistant(&["from the agent"]), user]);
    assert_eq!(got.as_deref(), Some("from the agent"));
}

#[test]
fn transcript_skips_a_malformed_line() {
    let got = last_assistant(&[&assistant(&["kept"]), "{ not json"]);
    assert_eq!(got.as_deref(), Some("kept"));
}

#[test]
fn transcript_missing_file_gives_none() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("absent.jsonl");
    assert_eq!(read_transcript_last_assistant(path.to_str().unwrap()), None);
}

fn json_str(s: &str) -> String {
    Value::String(s.to_owned()).to_string()
}

const AGY_RUN_COMMAND: &str =
    include_str!("../../tests/fixtures/antigravity-hooks/pretooluse-run-command.json");
const AGY_SESSION_START: &str =
    include_str!("../../tests/fixtures/antigravity-hooks/sessionstart.json");

#[test]
fn the_status_hook_reads_agy_camel_case_fields() {
    let line = status_line(HookVendor::Antigravity, AGY_RUN_COMMAND, "t");
    let v: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(v["event"], "PreToolUse");
    assert_eq!(v["session_id"], "4c9e68ec-9c07-43f0-9292-aa15a321723d");
    assert_eq!(v["tool_name"], "run_command");
    assert_eq!(v["tool_use_id"], "call_4566659267947845633");
    assert!(
        !line.contains("echo probe"),
        "no tool input is kept: {line}"
    );

    let start: Value = serde_json::from_str(&status_line(
        HookVendor::Antigravity,
        AGY_SESSION_START,
        "t",
    ))
    .unwrap();
    assert_eq!(start["event"], "SessionStart");
    assert_eq!(start["session_id"], "4c9e68ec-9c07-43f0-9292-aa15a321723d");
    assert_eq!(start["tool_name"], Value::Null);
}

/// agy runs a `PreToolUse` tool only on `{"decision":"allow"}` or an empty
/// stdout, and ignores a decision on its other events; the reply is printed
/// before the payload is read, so it is the same for every event.
#[test]
fn the_status_hook_never_denies_an_agy_tool() {
    let reply: Value = serde_json::from_str(status_reply(HookVendor::Antigravity)).unwrap();
    assert_eq!(reply, serde_json::json!({ "decision": "allow" }));
    assert_eq!(status_reply(HookVendor::Claude), "{}");
    assert_eq!(status_reply(HookVendor::Codex), "");
}
