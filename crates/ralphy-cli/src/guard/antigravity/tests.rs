use super::*;
use serde_json::json;

const RUN_COMMAND: &str =
    include_str!("../../../tests/fixtures/antigravity-hooks/pretooluse-run-command.json");
const WRITE_FILE: &str =
    include_str!("../../../tests/fixtures/antigravity-hooks/pretooluse-write-file.json");
const EDIT_FILE: &str =
    include_str!("../../../tests/fixtures/antigravity-hooks/pretooluse-edit-file.json");
const READ_FILE: &str =
    include_str!("../../../tests/fixtures/antigravity-hooks/pretooluse-read-file.json");

/// The fixture's workspace, as agy wrote it in the payload's `cwd`.
const WORKSPACE: &str = r"<home>\AppData\Local\Temp\agy784root\repo_p1";

/// The guard's answer on stdout for `raw`, judged with a tool dir and a temp
/// dir that hold none of the fixture's paths.
fn answer(raw: &str) -> String {
    reply(&decide(
        raw,
        None,
        "c:/tools/ralphy".into(),
        "c:/sys-temp".into(),
    ))
}

/// The fixture `raw` with its `toolCall.args` replaced by `args`.
fn with_args(raw: &str, args: Value) -> String {
    let mut payload: Value = serde_json::from_str(raw).unwrap();
    payload["toolCall"]["args"] = args;
    payload.to_string()
}

fn command(cmd: &str) -> String {
    with_args(
        RUN_COMMAND,
        json!({ "CommandLine": cmd, "WaitMsBeforeAsync": 5000 }),
    )
}

/// The answer parsed, so a test reads its `decision` and `reason`.
fn parsed(answer: &str) -> Value {
    serde_json::from_str(answer).unwrap_or_else(|e| panic!("not JSON: {answer}: {e}"))
}

fn assert_denied(answer: &str, case: &str) {
    let v = parsed(answer);
    assert_eq!(v["decision"], "deny", "{case}: {answer}");
    let reason = v["reason"].as_str().unwrap_or("");
    assert!(
        !reason.trim().is_empty(),
        "{case}: a deny names its reason: {answer}"
    );
}

#[test]
fn agy_run_command_with_a_denied_command_is_denied() {
    for cmd in [
        "git push origin main",
        "git reset --hard HEAD~1",
        "git clean -fd",
    ] {
        assert_denied(&answer(&command(cmd)), cmd);
    }
}

#[test]
fn agy_run_command_with_a_safe_command_prints_allow() {
    assert_eq!(answer(RUN_COMMAND), r#"{"decision":"allow"}"#);
    assert_eq!(
        answer(&command("cargo test -p x")),
        r#"{"decision":"allow"}"#
    );
}

#[test]
fn agy_write_file_outside_the_workspace_is_denied() {
    assert_eq!(
        answer(WRITE_FILE),
        r#"{"decision":"allow"}"#,
        "inside the workspace"
    );
    assert_eq!(
        answer(EDIT_FILE),
        r#"{"decision":"allow"}"#,
        "inside the workspace"
    );
    let outside = r"C:\Users\someone\Documents\notes.txt";
    let escape = format!(r"{WORKSPACE}\..\other\a.txt");
    let rows = [
        (
            "write_file",
            with_args(WRITE_FILE, json!({ "Path": outside, "Content": "x" })),
        ),
        (
            "write_file ..",
            with_args(WRITE_FILE, json!({ "Path": escape, "Content": "x" })),
        ),
        (
            "write_file ~",
            with_args(WRITE_FILE, json!({ "Path": "~/.bashrc", "Content": "x" })),
        ),
        (
            "edit_file",
            with_args(
                EDIT_FILE,
                json!({ "Path": outside, "Target": "a", "Replacement": "b" }),
            ),
        ),
    ];
    for (case, raw) in rows {
        assert_denied(&answer(&raw), case);
    }
}

#[test]
fn an_agy_payload_without_a_tool_call_is_denied() {
    let mut no_call: Value = serde_json::from_str(RUN_COMMAND).unwrap();
    no_call.as_object_mut().unwrap().remove("toolCall");
    let mut no_name: Value = serde_json::from_str(RUN_COMMAND).unwrap();
    no_name["toolCall"].as_object_mut().unwrap().remove("name");
    let rows = [
        ("empty", String::new()),
        ("not JSON", "toolCall".to_owned()),
        ("no toolCall", no_call.to_string()),
        ("toolCall without a name", no_name.to_string()),
        (
            "toolCall is a string",
            json!({ "toolCall": "run_command" }).to_string(),
        ),
        (
            "run_command without CommandLine",
            with_args(RUN_COMMAND, json!({})),
        ),
        (
            "write_file without Path",
            with_args(WRITE_FILE, json!({ "Content": "x" })),
        ),
    ];
    for (case, raw) in rows {
        assert_denied(&answer(&raw), case);
    }
}

#[test]
fn agy_remote_control_mic_serve_install_and_update_are_denied() {
    let rows = [
        "agy remote-control start",
        "antigravity remote-control serve",
        "agy mic-serve",
        "& 'C:\\Program Files\\agy\\agy.exe' install",
        "pwsh -Command \"agy --gemini_dir=C:\\r update\"",
    ];
    for cmd in rows {
        assert_denied(&answer(&command(cmd)), cmd);
    }
}

/// A tool that changes no file is not judged, as Claude's other tools are not.
#[test]
fn an_agy_read_only_tool_is_allowed() {
    assert_eq!(answer(READ_FILE), r#"{"decision":"allow"}"#);
}

/// A settings file written before `--vendor` runs `hook guard` alone: that is
/// Claude's guard.
#[test]
fn hook_guard_vendor_defaults_to_claude() {
    use crate::cli::{Cli, Command, GuardVendor, HookCommand};
    use clap::Parser;
    let vendor_of = |argv: &[&str]| match Cli::try_parse_from(argv).map(|cli| cli.command) {
        Ok(Command::Hook(HookCommand::Guard { vendor, .. })) => vendor,
        _ => panic!("{argv:?} must parse as hook guard"),
    };
    assert_eq!(vendor_of(&["ralphy", "hook", "guard"]), GuardVendor::Claude);
    assert_eq!(
        vendor_of(&["ralphy", "hook", "guard", "--cost-gate"]),
        GuardVendor::Claude
    );
    let agy = ["ralphy", "hook", "guard", "--vendor", "antigravity"];
    assert_eq!(vendor_of(&agy), GuardVendor::Antigravity);
}
