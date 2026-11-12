//! The guard for an Antigravity (`agy`) `PreToolUse` payload.
//!
//! agy names the tool in `toolCall.name` and its input in `toolCall.args`.
//! A command is judged by the rules of Claude's shell tool, in the shell agy
//! runs it in; a file write is judged by the file-write rules, and must stay
//! in the workspace. A payload the guard cannot read is denied.
//!
//! agy reads the decision from stdout. An empty stdout lets the tool run, so
//! the guard always prints one: `{"decision":"allow"}`, or
//! `{"decision":"deny","reason":"..."}`; the exit code is 0.
// ADR-0086 D9, D12; shapes measured in docs/spike/live/antigravity-784-hooks.log.

use std::path::Path;
use std::process::ExitCode;
use std::sync::LazyLock;

use regex::Regex;
use serde_json::{json, Value};

use super::{evaluate_guard, normalise_path, read_payload, GuardContext, GuardDecision};

/// The agy commands that start a remote take-over, a listening server, or
/// change the installed binary: Ralphy never runs them, so the agent may not.
// ADR-0086 D12.
static DAEMON_COMMAND: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(concat!(
        r#"(?i)\b(?:agy|antigravity)(?:\.exe)?['"]?(?:\s+-\S+)*\s+"#,
        r"(?:remote-control|mic-serve|install|update)\b",
    ))
    .expect("valid agy deny regex")
});

/// The Claude tool whose rules judge a `run_command`: agy runs it in
/// PowerShell on Windows (measured on agy 1.5.6).
const SHELL_TOOL: &str = if cfg!(windows) { "PowerShell" } else { "Bash" };

/// agy's tools that change a file. The model was seen to use only the first
/// two; agy's tool list names the others.
const FILE_TOOLS: [&str; 7] = [
    "write_file",
    "edit_file",
    "write_to_file",
    "replace_file_content",
    "multi_replace_file_content",
    "sed_file",
    "notebook_edit",
];

/// The `args` keys that name the file a file tool changes.
const FILE_KEYS: [&str; 2] = ["Path", "TargetFile"];

/// Judge the payload `raw` and print the answer agy reads.
pub(super) fn answer(
    raw: &str,
    fallback_cwd: Option<&Path>,
    tool_dir: String,
    temp_dir: String,
) -> ExitCode {
    outln!("{}", reply(&decide(raw, fallback_cwd, tool_dir, temp_dir)));
    ExitCode::SUCCESS
}

/// The decision as the JSON line agy reads.
fn reply(decision: &GuardDecision) -> String {
    match decision {
        GuardDecision::Allow => json!({ "decision": "allow" }),
        GuardDecision::Deny(reason) => json!({ "decision": "deny", "reason": reason }),
    }
    .to_string()
}

/// Judge one agy payload; `fallback_cwd` stands in for a payload without `cwd`.
fn decide(
    raw: &str,
    fallback_cwd: Option<&Path>,
    tool_dir: String,
    temp_dir: String,
) -> GuardDecision {
    let Some(payload) = read_payload(raw) else {
        return unreadable("not a JSON object");
    };
    let call = payload.get("toolCall");
    let Some(name) = call.and_then(|c| c.get("name")).and_then(Value::as_str) else {
        return unreadable("no toolCall name");
    };
    let args = call.and_then(|c| c.get("args")).unwrap_or(&Value::Null);
    let cwd = match payload.get("cwd").and_then(Value::as_str) {
        Some(cwd) => normalise_path(Path::new(cwd)),
        None => fallback_cwd.map(normalise_path).unwrap_or_default(),
    };
    let ctx = GuardContext {
        tool_dir,
        cwd,
        temp_dir,
    };
    match name {
        "run_command" => judge_command(args, &ctx),
        _ if FILE_TOOLS.contains(&name) => judge_file_write(&payload, args, &ctx),
        _ => GuardDecision::Allow,
    }
}

fn unreadable(what: &str) -> GuardDecision {
    GuardDecision::Deny(format!("could not read the tool call: {what}"))
}

/// A `run_command`: the agy daemon commands, then the rules of the shell tool.
fn judge_command(args: &Value, ctx: &GuardContext) -> GuardDecision {
    let Some(command) = args.get("CommandLine").and_then(Value::as_str) else {
        return unreadable("no CommandLine");
    };
    if DAEMON_COMMAND.is_match(command) {
        return GuardDecision::Deny(
            "agy remote control, mic-serve, install and update are blocked".into(),
        );
    }
    evaluate_guard(SHELL_TOOL, &json!({ "command": command }), ctx)
}

/// A file write: inside the workspace, then the file-write rules.
fn judge_file_write(payload: &Value, args: &Value, ctx: &GuardContext) -> GuardDecision {
    let Some(path) = FILE_KEYS
        .iter()
        .find_map(|k| args.get(*k).and_then(Value::as_str))
    else {
        return unreadable("no Path");
    };
    if !in_workspace(path, payload, ctx) {
        return GuardDecision::Deny(format!("writing outside the workspace is blocked ({path})"));
    }
    evaluate_guard("Write", &json!({ "file_path": path }), ctx)
}

/// Is `path` under the workspace: the session's `cwd`, a `workspacePaths`
/// entry, agy's artifact folder for the conversation, or the temp dir? A
/// relative path is under `cwd`; `..`, `~` and a variable are never trusted.
fn in_workspace(path: &str, payload: &Value, ctx: &GuardContext) -> bool {
    let p = normalise_path(Path::new(path));
    if p.split('/').any(|part| part == "..") || p.starts_with(['~', '$', '%']) {
        return false;
    }
    if !is_absolute(&p) {
        return true;
    }
    let listed = payload
        .get("workspacePaths")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .chain(payload.get("artifactDirectoryPath"))
        .filter_map(Value::as_str)
        .map(|root| normalise_path(Path::new(root)));
    [ctx.cwd.clone(), ctx.temp_dir.clone()]
        .into_iter()
        .chain(listed)
        .any(|root| !root.is_empty() && (p == root || p.starts_with(&format!("{root}/"))))
}

/// `/x` or a drive path `c:/x`, in the normalised form.
fn is_absolute(p: &str) -> bool {
    match p.as_bytes() {
        [b'/', ..] => true,
        [drive, b':', ..] => drive.is_ascii_alphabetic(),
        _ => false,
    }
}

#[cfg(test)]
mod tests;
