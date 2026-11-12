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

use serde_json::{json, Value};

use super::{evaluate_guard, normalise_path, read_payload, GuardContext, GuardDecision};

/// The agy verbs that start a remote take-over, a listening server, or change
/// the installed binary: Ralphy never runs them, so the agent may not.
// ADR-0086 D12.
const DAEMON_VERBS: [&str; 4] = ["remote-control", "mic-serve", "install", "update"];

/// Does `command` name an agy program token with a denied verb as a later
/// token? Quotes, `,`, `;`, `|`, `&` and parentheses separate tokens, so a
/// quoted verb, a flag value before the verb, a path, `agy.exe` and
/// `Start-Process agy -ArgumentList 'update'` all match.
fn is_daemon_command(command: &str) -> bool {
    let tokens = command
        .split(|c: char| c.is_whitespace() || ",;|&()'\"`".contains(c))
        .filter(|t| !t.is_empty());
    let mut after_agy = false;
    for token in tokens {
        let lower = token.to_lowercase();
        if after_agy && DAEMON_VERBS.contains(&lower.as_str()) {
            return true;
        }
        after_agy |= is_agy_program(&lower);
    }
    false
}

/// A token whose last path component is `agy` or `antigravity`, with or
/// without `.exe`; a flag is never a program.
fn is_agy_program(token: &str) -> bool {
    let name = token.rsplit(['/', '\\']).next().unwrap_or(token);
    !token.starts_with('-') && matches!(name.trim_end_matches(".exe"), "agy" | "antigravity")
}

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

/// The `args` keys that carry the text a `send_command_input` types. The name
/// was not measured, so the usual spellings are read.
const INPUT_KEYS: &[&str] = &["Input", "input", "Text", "text"];

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
        "run_command" => judge_command(args, &["CommandLine"], &ctx),
        "send_command_input" => judge_command(args, INPUT_KEYS, &ctx),
        _ if FILE_TOOLS.contains(&name) => judge_file_write(&payload, args, &ctx),
        _ => GuardDecision::Allow,
    }
}

fn unreadable(what: &str) -> GuardDecision {
    GuardDecision::Deny(format!("could not read the tool call: {what}"))
}

/// A `run_command`, or the input of a `send_command_input` (it types into a
/// running shell): the agy daemon commands, then the rules of the shell tool.
fn judge_command(args: &Value, keys: &[&str], ctx: &GuardContext) -> GuardDecision {
    let Some(command) = keys
        .iter()
        .find_map(|k| args.get(*k).and_then(Value::as_str))
    else {
        return unreadable(&format!("no {}", keys.join(" or ")));
    };
    if is_daemon_command(command) {
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
