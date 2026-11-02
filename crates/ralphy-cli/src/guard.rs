//! The `ralphy hook guard` PreToolUse safety hook (originally a port of the
//! retired `guard.ps1`; this file is now the canonical implementation).
//!
//! Claude Code runs this before every Bash/PowerShell/Edit/Write/MultiEdit/
//! NotebookEdit call. Because the loop runs with --dangerously-skip-permissions (no
//! interactive prompts), this hook is the main guardrail between the agent and
//! a destructive command — but it is a best-effort deny-list, not a sandbox: a
//! sufficiently creative command line can slip past the regexes.
//!
//! ADAPTER ASYMMETRY: only the Claude adapter installs this hook. Codex runs
//! with `-s danger-full-access` and OpenCode with
//! `--dangerously-skip-permissions`, and neither CLI offers a PreToolUse
//! equivalent — for those adapters, safety rests on the isolated run branch
//! and the self-review (see README "Command guardrails").
//!
//! Recursive deletes get a carve-out instead of a blanket deny: deleting
//! inside the agent's worktree or the system temp dir is legitimate (build
//! artifacts, `node_modules`, browser profiles from e2e runs) and is allowed;
//! only targets that escape both — absolute paths elsewhere, `..`, `~`, or
//! unresolvable `$VAR`/`%VAR%` expansions — are blocked.
//!
//! The guard judges what a segment runs, not its first word
//! ([`wrappers`]): it reads past `sudo`, `env`, `time`, `nice` and `nohup`,
//! and judges the script of `bash|sh|zsh -c`, `cmd /c|/k` and
//! `powershell|pwsh -Command` as a command of its own, up to three shells
//! deep. A deeper script, an encoded PowerShell command, and a recursive
//! delete behind `xargs` (its targets come from the input) are denied.
//! Still not read: `find … -exec`, `$(…)` command substitution, and aliases
//! or functions the session defines.
//!
//! Protocol: exit 2 (reason to stderr) to block; exit 0 to allow.
//!
//! The deny-list logic is factored into [`evaluate_guard`], a pure function
//! over the tool name, tool input, and a [`GuardContext`], so it unit-tests
//! without touching the filesystem or environment.

use std::sync::LazyLock;

use regex::Regex;
use serde_json::Value;

mod wrappers;

use wrappers::{command_name, Runs, Shell};

/// How many shells inside shells (`bash -c "bash -c …"`) the guard reads.
/// A deeper script is denied: it is not something an agent needs.
const MAX_NESTING: usize = 3;

/// The Bash deny-list: each `(regex, why)` pair blocks a destructive command.
/// All rules are case-insensitive (`(?i)`) — PowerShell is case-insensitive, so
/// `remove-item` must be caught as surely as `Remove-Item`. Compiled once — the
/// guard hook fires before *every* Bash/Edit/Write call, so recompiling these
/// per invocation was wasted work on the one hot safety path.
///
/// Recursive deletes are NOT in this list — they get path-aware treatment in
/// [`first_disallowed_target`] (allowed inside the worktree/temp).
static BASH_DENY_RULES: LazyLock<Vec<(Regex, &'static str)>> = LazyLock::new(|| {
    [
        (
            r"(?i)\bgit\s+push\b",
            "pushing is the orchestrator's job, not the agent's",
        ),
        (
            r"(?i)\bgit\s+reset\s+--hard\b",
            "hard reset can destroy uncommitted work",
        ),
        (r"(?i)\bgit\s+clean\b", "git clean deletes untracked files"),
        (
            r"(?i)\bgit\s+rebase\b",
            "history rewrite is not allowed in the loop",
        ),
        (
            r"(?i)\bgit\s+(checkout|switch)\b",
            "the agent must stay on the run branch the orchestrator created",
        ),
        (
            r"(?i)\bgit\s+worktree\b",
            "worktrees are the orchestrator's business, not the agent's",
        ),
        (
            // Global flags (`-R o/r`, `--repo=o/r`) may sit between `gh` and `pr`.
            r"(?i)\bgh(?:\s+-\S+(?:\s+[^-\s]\S*)?)*\s+pr\s+(create|edit|ready|reopen|review|comment|merge|close)\b",
            "writing to a pull request is the operator's job",
        ),
        (
            r"(?i)\bgh\s+(release|repo|workflow|secret|auth)\b",
            "repo/release/workflow/secret/auth ops are out of scope",
        ),
        (
            r"(?i)\bcargo\s+publish\b",
            "publishing crates is out of scope",
        ),
        (
            // `format` only as a command name (start of command or after a
            // separator) — `--format json` / `--format=%H` flags are benign.
            r"(?i)(?:^|[;&|]\s*)format(?:\.com)?\s|(?i)\b(?:mkfs|diskpart)\b",
            "disk-level command is blocked",
        ),
        (
            r"(?i)\bcurl\b.*\|\s*(sh|bash|pwsh|powershell)",
            "piping a download into a shell is blocked",
        ),
        (
            r"(?i)iwr\b.*\|\s*iex|(?i)Invoke-Expression\b",
            "remote code execution is blocked",
        ),
    ]
    .into_iter()
    .map(|(rx, why)| (Regex::new(rx).expect("valid guard regex"), why))
    .collect()
});

/// The decision returned by [`evaluate_guard`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GuardDecision {
    Allow,
    Deny(String),
}

/// The environment [`evaluate_guard`] judges against. All paths are
/// pre-normalised via [`normalise_path`] (lowercase, `/`-separated, no
/// trailing slash) so the pure evaluation logic never touches the filesystem.
pub struct GuardContext {
    /// The Ralphy binary's parent directory (writes here are blocked).
    pub tool_dir: String,
    /// The hook invocation's working directory — the agent's worktree.
    /// Recursive deletes under it are allowed.
    pub cwd: String,
    /// The system temp directory. Recursive deletes under it are allowed.
    pub temp_dir: String,
}

/// Evaluate whether a PreToolUse call should be allowed or denied.
///
/// `tool_name` is the `tool_name` field from the hook payload.
/// `tool_input` is the `tool_input` object.
pub fn evaluate_guard(tool_name: &str, tool_input: &Value, ctx: &GuardContext) -> GuardDecision {
    match tool_name {
        // On Windows a session also runs commands with its `PowerShell` tool,
        // whose input is a `command` like Bash's (measured on Claude Code
        // 2.1.316, #810). The rules are case-insensitive and know PowerShell.
        "Bash" => evaluate_tool_command(tool_input, ctx, Shell::Posix),
        "PowerShell" => evaluate_tool_command(tool_input, ctx, Shell::PowerShell),
        "Edit" | "Write" | "MultiEdit" | "NotebookEdit" => evaluate_file_write(tool_input, ctx),
        _ => GuardDecision::Allow,
    }
}

/// What one command is judged against: the context, the shell that runs it,
/// and how many shells are around it.
#[derive(Clone, Copy)]
struct Judge<'a> {
    ctx: &'a GuardContext,
    shell: Shell,
    depth: usize,
}

/// Deny-list for the command of a Bash or PowerShell call, plus path-aware
/// recursive-delete handling.
fn evaluate_tool_command(tool_input: &Value, ctx: &GuardContext, shell: Shell) -> GuardDecision {
    let cmd = tool_input
        .get("command")
        .and_then(Value::as_str)
        .unwrap_or("");
    if cmd.trim().is_empty() {
        return GuardDecision::Allow;
    }
    evaluate_command(
        cmd,
        Judge {
            ctx,
            shell,
            depth: 0,
        },
    )
}

/// Judge one command line: the deny rules over its whole text, then each of
/// its segments.
fn evaluate_command(cmd: &str, judge: Judge) -> GuardDecision {
    if let Some(why) = denied_by_rules(cmd) {
        return GuardDecision::Deny(why);
    }
    if let Some(tok) = dotnet_delete_target(cmd, judge.ctx) {
        return GuardDecision::Deny(format!(
            "recursive delete outside the worktree/temp is blocked ({tok})"
        ));
    }
    for segment in cmd.split(['\n', ';', '&', '|']) {
        let decision = evaluate_segment(segment, judge);
        if decision != GuardDecision::Allow {
            return decision;
        }
    }
    GuardDecision::Allow
}

/// PowerShell's .NET recursive delete, `[System.IO.Directory]::Delete(path,
/// $true)`: its path is the first argument.
static DOTNET_DELETE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(concat!(
        r"(?i)\[(?:system\.)?io\.directory\]::delete\(",
        r#"\s*['"]?([^,'"()]+?)['"]?\s*,\s*\$true\s*\)"#,
    ))
    .expect("valid guard regex")
});

/// The first path of a .NET recursive delete in `cmd` that a recursive
/// delete may not touch.
fn dotnet_delete_target(cmd: &str, ctx: &GuardContext) -> Option<String> {
    DOTNET_DELETE
        .captures_iter(cmd)
        .map(|c| c[1].trim().to_string())
        .find(|path| !recursive_delete_target_allowed(path, ctx))
}

/// The reason of the first deny rule `text` matches.
fn denied_by_rules(text: &str) -> Option<String> {
    BASH_DENY_RULES
        .iter()
        .find(|(re, _)| re.is_match(text))
        .map(|(_, why)| (*why).to_string())
}

/// Judge one segment by what it runs once its wrapper words are skipped.
fn evaluate_segment(segment: &str, judge: Judge) -> GuardDecision {
    let tokens = shell_tokens(segment);
    match wrappers::what_runs(&tokens) {
        Runs::Script(..) if judge.depth >= MAX_NESTING => {
            GuardDecision::Deny("a shell nested this deep cannot be judged".into())
        }
        Runs::Script(script, shell) => evaluate_command(
            &script,
            Judge {
                shell,
                depth: judge.depth + 1,
                ..judge
            },
        ),
        Runs::Unreadable => GuardDecision::Deny("an encoded command cannot be judged".into()),
        Runs::FromInput(args) => evaluate_from_input(args, judge.shell),
        Runs::Command(words) => evaluate_words(words, tokens.len(), judge),
    }
}

/// `xargs` runs its command with arguments from the input, which the guard
/// cannot see: a recursive delete there is denied wherever it points.
fn evaluate_from_input(args: &[String], shell: Shell) -> GuardDecision {
    if (0..args.len()).any(|at| classify_delete(&args[at..], shell).is_some()) {
        return GuardDecision::Deny(
            "a recursive delete with targets from the input (xargs) is blocked".into(),
        );
    }
    GuardDecision::Allow
}

/// Judge a command by its words. When wrapper words were skipped (`words`
/// is shorter than the segment), the deny rules read the command again: the
/// `format` rule looks only at the start of a command.
fn evaluate_words(words: &[String], segment_len: usize, judge: Judge) -> GuardDecision {
    if words.len() < segment_len {
        if let Some(why) = denied_by_rules(&words.join(" ")) {
            return GuardDecision::Deny(why);
        }
    }
    match first_disallowed_target(words, judge) {
        Some(tok) => GuardDecision::Deny(format!(
            "recursive delete outside the worktree/temp is blocked ({tok})"
        )),
        None => GuardDecision::Allow,
    }
}

/// The shape of delete command a segment turned out to be.
enum DeleteKind {
    /// `rm` with combined/separate recursive+force flags.
    Rm,
    /// PowerShell `Remove-Item -Recurse`.
    RemoveItem,
    /// cmd.exe `del /s` or `rmdir /s`.
    DelRmdir,
}

/// The first target of the command `words` that a recursive delete may not
/// touch, or `None` when it is not a recursive delete or every target is
/// allowed. A target must stay inside the worktree (`ctx.cwd`), the system
/// temp dir, or `/tmp`; a relative path (no `..`) resolves under the
/// worktree; `~`, `$VAR` and `%VAR%` cannot be resolved here.
fn first_disallowed_target(words: &[String], judge: Judge) -> Option<String> {
    let (kind, args) = classify_delete(words, judge.shell)?;
    args.iter()
        .find(|tok| !is_delete_flag(&kind, tok) && !recursive_delete_target_allowed(tok, judge.ctx))
        .cloned()
}

/// Is `tok` an option of the delete command, not a target?
fn is_delete_flag(kind: &DeleteKind, tok: &str) -> bool {
    match kind {
        DeleteKind::Rm | DeleteKind::RemoveItem => tok.starts_with('-'),
        // cmd.exe switches: `/s`, `/q`, `/a:h`, … (also tolerate `-`).
        DeleteKind::DelRmdir => {
            tok.starts_with('-')
                || (tok.starts_with('/')
                    && tok.len() <= 4
                    && tok[1..]
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == ':'))
        }
    }
}

/// Classify a command's words as a recursive delete, returning the kind and
/// the argument words (everything after the command word). The wrapper words
/// in front (`sudo`, `env`, …) are already skipped (`wrappers::what_runs`).
fn classify_delete(words: &[String], shell: Shell) -> Option<(DeleteKind, &[String])> {
    let cmd = command_name(words.first()?);
    let args = &words[1..];
    let kind = powershell_delete(&cmd, args, shell).or_else(|| shell_delete(&cmd, args))?;
    Some((kind, args))
}

/// `Remove-Item` and its PowerShell aliases with `-Recurse`, written in full
/// or cut short as PowerShell allows. In a POSIX shell `rm -r` is the POSIX
/// flag, judged by [`shell_delete`], so there an alias needs at least
/// `-re`; in PowerShell `rm -r` is `Remove-Item -Recurse`.
fn powershell_delete(cmd: &str, args: &[String], shell: Shell) -> Option<DeleteKind> {
    let shortest = match (cmd, shell) {
        ("remove-item" | "ri", _) => 2,
        ("rm" | "del" | "erase" | "rd" | "rmdir", Shell::PowerShell) => 2,
        ("rm" | "del" | "erase" | "rd" | "rmdir", Shell::Posix) => 3,
        _ => return None,
    };
    args.iter()
        .any(|t| t.len() >= shortest && wrappers::is_prefix_of(&t.to_lowercase(), "-recurse"))
        .then_some(DeleteKind::RemoveItem)
}

/// A POSIX `rm` that is recursive AND forced (parity with the original
/// rule: `rm -r` alone still prompts per file), or cmd.exe `del /s` and
/// `rmdir /s`.
fn shell_delete(cmd: &str, args: &[String]) -> Option<DeleteKind> {
    match cmd {
        "rm" => {
            let flags: Vec<&str> = args
                .iter()
                .filter(|t| t.starts_with('-'))
                .map(String::as_str)
                .collect();
            let has = |short: char, long: &str| {
                flags
                    .iter()
                    .any(|f| *f == long || (!f.starts_with("--") && f.contains(short)))
            };
            (has('r', "--recursive") && has('f', "--force")).then_some(DeleteKind::Rm)
        }
        "del" | "rmdir" | "rd" if args.iter().any(|t| t.eq_ignore_ascii_case("/s")) => {
            Some(DeleteKind::DelRmdir)
        }
        _ => None,
    }
}

/// Is `raw` an acceptable target for a recursive delete? Allowed: relative
/// paths without `..` (they resolve under the worktree), and absolute paths
/// under `ctx.cwd`, `ctx.temp_dir`, or `/tmp`.
fn recursive_delete_target_allowed(raw: &str, ctx: &GuardContext) -> bool {
    let p = raw.replace('\\', "/").to_lowercase();
    let p = p.trim_end_matches('/');
    // `/` (and `//`) trims to nothing: it is the root, not a relative path.
    if p.is_empty() {
        return false;
    }
    if p.split('/').any(|seg| seg == "..") {
        return false;
    }
    if p.starts_with(['~', '$', '%']) {
        return false;
    }
    let is_drive_absolute =
        p.len() >= 2 && p.as_bytes()[1] == b':' && p.as_bytes()[0].is_ascii_alphabetic();
    if p.starts_with('/') || is_drive_absolute {
        return under(p, &ctx.cwd) || under(p, &ctx.temp_dir) || under(p, "/tmp");
    }
    true
}

/// Is normalised path `p` equal to or below normalised directory `dir`?
fn under(p: &str, dir: &str) -> bool {
    !dir.is_empty() && (p == dir || p.starts_with(&format!("{dir}/")))
}

/// Quote-aware whitespace tokeniser — just enough shell/PowerShell lexing to
/// pull path arguments (possibly containing spaces) out of a delete command.
/// Inside double quotes `\"` is a `"`, so the script of
/// `bash -c "bash -c \"…\""` keeps its inner quotes; a backslash anywhere
/// else is kept, so a Windows path is read as it is written.
fn shell_tokens(segment: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut chars = segment.chars().peekable();
    while let Some(c) = chars.next() {
        match (quote, c) {
            (Some('"'), '\\') if chars.peek() == Some(&'"') => {
                cur.push('"');
                chars.next();
            }
            (Some(q), c) if c == q => quote = None,
            (None, '\'' | '"') => quote = Some(c),
            (None, c) if c.is_whitespace() => end_token(&mut out, &mut cur),
            (_, c) => cur.push(c),
        }
    }
    end_token(&mut out, &mut cur);
    out
}

/// Close the token being read: move `cur` into `out` unless it is empty.
fn end_token(out: &mut Vec<String>, cur: &mut String) {
    if !cur.is_empty() {
        out.push(std::mem::take(cur));
    }
}

/// Deny-list for file-write tools.
fn evaluate_file_write(tool_input: &Value, ctx: &GuardContext) -> GuardDecision {
    // NotebookEdit names its file `notebook_path`.
    let path = tool_input
        .get("file_path")
        .or_else(|| tool_input.get("notebook_path"))
        .and_then(Value::as_str)
        .unwrap_or("");
    if path.trim().is_empty() {
        return GuardDecision::Allow;
    }

    let p = path.replace('\\', "/").to_lowercase();

    let blocked = [
        "/.git/",
        "/.env",
        "/secrets",
        "/credentials",
        "/id_rsa",
        ".pem",
        ".pfx",
    ];
    for b in &blocked {
        if p.contains(b) {
            return GuardDecision::Deny(format!("writing to a protected path ({path})"));
        }
    }

    let tool_dir = ctx.tool_dir.trim_end_matches('/');
    if !tool_dir.is_empty() && p.starts_with(&format!("{tool_dir}/")) {
        return GuardDecision::Deny(format!(
            "writing to Ralphy's own tooling is not allowed ({path})"
        ));
    }

    GuardDecision::Allow
}

/// Normalise a path for use in [`GuardContext`]: lower-case, `\` → `/`, and no
/// trailing slash.
fn normalise_path(dir: &std::path::Path) -> String {
    dir.to_string_lossy()
        .replace('\\', "/")
        .trim_end_matches('/')
        .to_lowercase()
}

/// Parse a hook payload and judge it. Input the guard cannot read — empty,
/// blank, not JSON, a JSON root that is not an object, no `tool_name`, or a
/// judged tool without its string `command` or path — is a deny: the guard
/// fails closed. A readable payload is judged by the rules, so a tool the
/// guard does not know is allowed.
///
/// `fallback_cwd` stands in for a payload without `cwd`; `tool_dir` and
/// `temp_dir` are already in the [`GuardContext`] form.
// ADR-0072 D6.
pub fn decide_hook_input(
    raw: &str,
    fallback_cwd: Option<&std::path::Path>,
    tool_dir: String,
    temp_dir: String,
) -> GuardDecision {
    let Some(payload) = read_payload(raw) else {
        let what = if raw.trim().is_empty() {
            "empty input"
        } else {
            "not JSON"
        };
        return GuardDecision::Deny(format!("could not read the tool call: {what}"));
    };
    let (tool_name, tool_input) = tool_call(&payload);
    if let Some(what) = unreadable_shape(&payload, tool_name, tool_input) {
        return GuardDecision::Deny(format!("could not read the tool call: {what}"));
    }
    // The hook payload's `cwd` is the agent's worktree; fall back to the
    // process cwd (hooks run in the project directory).
    let cwd = payload
        .get("cwd")
        .and_then(Value::as_str)
        .map(|s| normalise_path(std::path::Path::new(s)))
        .or_else(|| fallback_cwd.map(normalise_path))
        .unwrap_or_default();
    let ctx = GuardContext {
        tool_dir,
        cwd,
        temp_dir,
    };
    evaluate_guard(tool_name, tool_input, &ctx)
}

/// The payload as a JSON object, or `None` when it is not one.
fn read_payload(raw: &str) -> Option<Value> {
    if raw.trim().is_empty() {
        return None;
    }
    serde_json::from_str::<Value>(raw)
        .ok()
        .filter(Value::is_object)
}

/// What is missing from a payload whose fields the rules need. The settings
/// matcher sends only the tools the guard judges, so a call without a
/// `tool_name`, or one of those tools without its string `command` or path,
/// is a shape the guard cannot read, not a call with nothing to judge.
fn unreadable_shape(payload: &Value, tool_name: &str, tool_input: &Value) -> Option<&'static str> {
    if !payload.get("tool_name").is_some_and(Value::is_string) {
        return Some("no tool_name");
    }
    let has_str = |key: &str| tool_input.get(key).is_some_and(Value::is_string);
    match tool_name {
        "Bash" | "PowerShell" if !has_str("command") => Some("no command"),
        "Edit" | "Write" | "MultiEdit" if !has_str("file_path") => Some("no file_path"),
        "NotebookEdit" if !has_str("file_path") && !has_str("notebook_path") => {
            Some("no notebook_path")
        }
        _ => None,
    }
}

fn tool_call(payload: &Value) -> (&str, &Value) {
    let tool_name = payload
        .get("tool_name")
        .and_then(Value::as_str)
        .unwrap_or("");
    (tool_name, payload.get("tool_input").unwrap_or(&Value::Null))
}

/// Run the `hook guard` subcommand: read the payload from stdin, evaluate it,
/// and exit 0 (allow) or exit 2 (deny). With `cost_gate`, an allowed Bash call
/// also passes the verification-cost gate. Never returns — always calls
/// `std::process::exit`.
#[expect(
    clippy::disallowed_methods,
    reason = "the hook guard reports its decision by its exit code, so it must exit"
)]
pub fn run_guard_hook(cost_gate: bool) -> ! {
    use std::io::Read;

    let mut raw = String::new();
    // A read error leaves `raw` empty or partial; both are judged unreadable.
    if std::io::stdin().read_to_string(&mut raw).is_err() {
        raw.clear();
    }
    let tool_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(normalise_path))
        .unwrap_or_default();
    let process_cwd = std::env::current_dir().ok();
    let temp_dir = normalise_path(&std::env::temp_dir());

    let reason = match decide_hook_input(&raw, process_cwd.as_deref(), tool_dir, temp_dir) {
        GuardDecision::Allow if cost_gate => cost_gate_reason(&raw),
        GuardDecision::Allow => None,
        GuardDecision::Deny(reason) => Some(reason),
    };
    match reason {
        Some(reason) => {
            eprintln!("BLOCKED by Ralphy guard: {reason}");
            std::process::exit(2);
        }
        None => std::process::exit(0),
    }
}

/// The steering message of the verification-cost gate for an allowed call, or
/// `None` to let it run. The gate judges Bash calls only, in execute sessions:
/// it denies re-paying a plan `## Verify` command already measured as
/// expensive while the plan still has real work open. Safety-neutral and
/// fail-open: any missing file or unknown cost allows.
fn cost_gate_reason(raw: &str) -> Option<String> {
    let payload = read_payload(raw)?;
    let (tool_name, tool_input) = tool_call(&payload);
    if tool_name != "Bash" {
        return None;
    }
    evaluate_cmd_cost(&payload, tool_input)
}

/// The verification-cost gate over one allowed Bash call: consult the durable
/// cost knowledge under the project's `.ralphy/` and either deny (returning the
/// steering message) or stamp the command's start for the Post hook to time.
/// The project root comes from the payload's `cwd` (raw, NOT the lower-cased
/// [`GuardContext`] form — this one is used for file IO); missing plan or state
/// degrades to allow.
fn evaluate_cmd_cost(payload: &Value, tool_input: &Value) -> Option<String> {
    let command = tool_input.get("command").and_then(Value::as_str)?;
    let root = project_root(payload)?;
    let plan_md = std::fs::read_to_string(root.join(".ralphy").join("plan.md")).ok()?;
    let state = ralphy_core::cmdcost::load(&root);
    match ralphy_core::cmdcost::decide(command, &plan_md, &state) {
        ralphy_core::cmdcost::CostDecision::Deny(reason) => Some(reason),
        ralphy_core::cmdcost::CostDecision::Allow => {
            ralphy_core::cmdcost::note_start(&root, command, &plan_md);
            None
        }
    }
}

/// The project root for hook file IO: the payload's `cwd` (the agent session
/// runs in the project directory), falling back to the process cwd.
pub(crate) fn project_root(payload: &Value) -> Option<std::path::PathBuf> {
    payload
        .get("cwd")
        .and_then(Value::as_str)
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::current_dir().ok())
}

#[cfg(test)]
mod tests;
