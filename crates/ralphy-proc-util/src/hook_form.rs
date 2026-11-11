//! The form of a Claude Code hook entry, decided once for every writer.
//!
//! Claude Code runs a hook entry without a shell from version 2.1.139 (the
//! exec form: `command` plus `args`). Below that version, or when the version
//! cannot be read, the entry is one shell string. Both the run settings and the
//! daemon settings render through this module, so they never disagree.

use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{LazyLock, Mutex, MutexGuard};
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};

/// The first Claude Code version that runs a hook entry without a shell,
/// measured on a live session.
pub const EXEC_FORM_FROM: (u32, u32, u32) = (2, 1, 139);

/// How long `claude --version` may run before the read gives up.
pub const VERSION_READ_LIMIT: Duration = Duration::from_secs(5);

/// How a hook entry runs its program.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookForm {
    /// `command` is the program and `args` are its arguments; no shell.
    Exec,
    /// `command` is one string that a shell parses.
    Shell,
}

/// One hook entry: the program, its arguments, the form, and why that form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookCommand {
    program: PathBuf,
    args: Vec<String>,
    form: HookForm,
    reason: String,
}

impl HookCommand {
    /// The JSON object that goes into a `hooks` array of a settings file.
    pub fn entry(&self) -> Value {
        match self.form {
            HookForm::Exec => json!({
                "type": "command",
                "command": self.program.to_string_lossy(),
                "args": self.args,
            }),
            HookForm::Shell => json!({
                "type": "command",
                "command": self.shell_string(),
            }),
        }
    }

    pub fn form(&self) -> HookForm {
        self.form
    }

    /// A plain sentence that says why this form was chosen.
    pub fn reason(&self) -> &str {
        &self.reason
    }

    fn shell_string(&self) -> String {
        let mut parts = vec![shell_quote(&shell_path(&self.program))];
        parts.extend(self.args.iter().map(|a| shell_quote(a)));
        parts.join(" ")
    }
}

/// `x.y.z` from the first word of `claude --version` output, such as
/// `2.1.138 (Claude Code)`.
pub fn parse_claude_version(text: &str) -> Option<(u32, u32, u32)> {
    let word = text.split_whitespace().next()?;
    let mut parts = word.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next()?.parse().ok()?;
    parts.next().is_none().then_some((major, minor, patch))
}

/// The hook entry for a Claude Code version. Pure: `None`, or text that holds
/// no version, gives the shell form.
pub fn hook_for_version(version: Option<&str>, program: &Path, args: &[&str]) -> HookCommand {
    let (form, reason) = decide(version);
    HookCommand {
        program: crate::exe::plain_path(program.to_path_buf()),
        args: args.iter().map(|a| (*a).to_owned()).collect(),
        form,
        reason,
    }
}

fn decide(version: Option<&str>) -> (HookForm, String) {
    let shell = HookForm::Shell;
    let Some(text) = version else {
        return (
            shell,
            "the Claude Code version could not be read, so the hook runs through a shell".into(),
        );
    };
    let Some(found) = parse_claude_version(text) else {
        return (
            shell,
            format!(
                "the Claude Code version \"{}\" is not a version number, \
                 so the hook runs through a shell",
                text.trim()
            ),
        );
    };
    if found >= EXEC_FORM_FROM {
        return (
            HookForm::Exec,
            format!("Claude Code {} runs a hook without a shell", dotted(found)),
        );
    }
    (
        shell,
        format!(
            "Claude Code {} is older than {}, the first version that runs a hook without a shell",
            dotted(found),
            dotted(EXEC_FORM_FROM)
        ),
    )
}

fn dotted((major, minor, patch): (u32, u32, u32)) -> String {
    format!("{major}.{minor}.{patch}")
}

/// Read the version of `claude_bin` and decide the form. A shell form is
/// logged with its reason once per binary and reason: one settings file asks
/// for several hooks, and each would log the same line.
pub fn claude_hook(claude_bin: &Path, program: &Path, args: &[&str]) -> HookCommand {
    let version = read_version(claude_bin, VERSION_READ_LIMIT);
    let hook = hook_for_version(version.as_deref(), program, args);
    if hook.form() == HookForm::Shell && first_warning(claude_bin, hook.reason()) {
        tracing::warn!("{}", hook.reason());
    }
    hook
}

static WARNED: LazyLock<Mutex<HashSet<(PathBuf, String)>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));

/// `true` the first time this process sees `reason` for `bin`.
fn first_warning(bin: &Path, reason: &str) -> bool {
    let bin = std::fs::canonicalize(bin).unwrap_or_else(|_| bin.to_path_buf());
    // A poisoned set still holds valid entries: each insert is one call.
    let mut warned = WARNED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    warned.insert((bin, reason.to_owned()))
}

type CacheKey = (PathBuf, SystemTime);

static VERSION_CACHE: LazyLock<Mutex<HashMap<CacheKey, String>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn lock_cache() -> MutexGuard<'static, HashMap<CacheKey, String>> {
    // A poisoned cache still holds valid entries: each insert is one call.
    VERSION_CACHE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn cache_key(bin: &Path) -> Option<CacheKey> {
    let path = std::fs::canonicalize(bin).ok()?;
    let modified = std::fs::metadata(&path).ok()?.modified().ok()?;
    Some((path, modified))
}

/// The first line `<bin> --version` prints, or `None` when the program does
/// not start, fails, prints nothing, or runs past `limit` (its tree is then
/// killed). A good answer is cached per canonical path and modification time,
/// so a binary that did not change is run once.
pub fn read_version(bin: &Path, limit: Duration) -> Option<String> {
    let key = cache_key(bin);
    if let Some(hit) = key.as_ref().and_then(|k| lock_cache().get(k).cloned()) {
        return Some(hit);
    }
    let version = spawn_version(bin, limit)?;
    if let Some(key) = key {
        lock_cache().insert(key, version.clone());
    }
    Some(version)
}

fn spawn_version(bin: &Path, limit: Duration) -> Option<String> {
    let mut cmd = Command::new(bin);
    cmd.arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    crate::no_window(&mut cmd);
    crate::own_process_group(&mut cmd);
    let mut child = cmd.spawn().ok()?;
    let mut stdout = child.stdout.take()?;
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut text = String::new();
        let read = stdout.read_to_string(&mut text).map(|_| text);
        // The receiver is gone after a timeout; nothing reads the text then.
        #[expect(
            clippy::let_underscore_must_use,
            reason = "the receiver is gone after a timeout, and nothing reads the text then"
        )]
        let _ = tx.send(read);
    });
    let Ok(Ok(text)) = rx.recv_timeout(limit) else {
        crate::kill_tree(&mut child);
        return None;
    };
    let succeeded = child.wait().is_ok_and(|s| s.success());
    let line = text.lines().next()?.trim();
    (succeeded && !line.is_empty()).then(|| line.to_owned())
}

/// How long the start check lets the hook program run.
pub const CHECK_LIMIT: Duration = Duration::from_secs(10);

/// The test event the start check sends on the hook's stdin.
const CHECK_EVENT: &str =
    r#"{"hook_event_name":"SessionStart","session_id":"ralphy-start-check","source":"startup"}"#;

/// What one run of a hook command with a test event showed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookCheck {
    /// The program exited 0 and wrote a status line.
    Ok,
    /// The program did not run, failed, or wrote no line.
    Failed { cause: String },
    /// The command was not run.
    NotChecked { reason: String },
}

/// Run `cmd` once with a test `SessionStart` event and a temporary status
/// file. It passes only on exit 0 AND a written line. A shell form is not
/// run. A failure names a `disableAllHooks` file under `home` first.
pub fn check(cmd: &HookCommand, limit: Duration, home: Option<&Path>) -> HookCheck {
    if cmd.form == HookForm::Shell {
        return HookCheck::NotChecked {
            reason: cmd.reason.clone(),
        };
    }
    match run_check(cmd, limit) {
        Ok(()) => HookCheck::Ok,
        Err(cause) => HookCheck::Failed {
            cause: match disable_all_hooks(home, None) {
                Some(file) => format!(
                    "{} sets disableAllHooks, so Claude Code runs no hook; {cause}",
                    file.display()
                ),
                None => cause,
            },
        },
    }
}

fn run_check(cmd: &HookCommand, limit: Duration) -> Result<(), String> {
    let program = cmd.program.display();
    let dir = tempfile::tempdir().map_err(|e| format!("could not make a temporary folder: {e}"))?;
    let status_file = dir.path().join("status.jsonl");
    let mut command = Command::new(&cmd.program);
    command
        .args(&cmd.args)
        .env("RALPHY_STATUS_FILE", &status_file)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    crate::no_window(&mut command);
    crate::own_process_group(&mut command);
    let mut child = command
        .spawn()
        .map_err(|e| format!("{program} did not start: {e}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        // A hook that exits before it reads its stdin closes the pipe; the
        // exit code and the status file still decide the check.
        let _write = std::io::Write::write_all(&mut stdin, CHECK_EVENT.as_bytes());
    }
    let status = wait_until(&mut child, limit)
        .ok_or_else(|| format!("{program} ran longer than {} s", limit.as_secs()))?;
    if !status.success() {
        return Err(match status.code() {
            Some(code) => format!("{program} exited with code {code}"),
            None => format!("{program} was stopped by a signal"),
        });
    }
    let written = std::fs::read_to_string(&status_file)
        .is_ok_and(|text| text.lines().any(|l| !l.trim().is_empty()));
    if written {
        Ok(())
    } else {
        Err(format!("{program} wrote no status line"))
    }
}

/// The exit status of `child`, or `None` after `limit` (its tree is then
/// killed).
fn wait_until(
    child: &mut std::process::Child,
    limit: Duration,
) -> Option<std::process::ExitStatus> {
    let started = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Some(status),
            Ok(None) if started.elapsed() < limit => {
                std::thread::sleep(Duration::from_millis(25));
            }
            _ => {
                crate::kill_tree(child);
                return None;
            }
        }
    }
}

/// The first of the user, project and local project settings files that sets
/// `disableAllHooks` to `true`. Claude Code then runs no hook, so no proof
/// can arrive. The path uses the separator of the platform only.
pub fn disable_all_hooks(home: Option<&Path>, project: Option<&Path>) -> Option<PathBuf> {
    let home = home.map(native);
    let project = project.map(native);
    let candidates = [
        home.map(|h| h.join(".claude").join("settings.json")),
        project
            .as_ref()
            .map(|p| p.join(".claude").join("settings.json")),
        project.map(|p| p.join(".claude").join("settings.local.json")),
    ];
    candidates
        .into_iter()
        .flatten()
        .find(|f| sets_disable_all_hooks(f))
}

/// `path` rebuilt one component at a time. On Windows, `git rev-parse
/// --show-toplevel` gives a root with `/`, and a join adds `\`; the rebuilt
/// path has `\` only. Use it to show a path to a person: the text then has
/// the separator of the platform and no other. On other platforms it is the
/// same path.
pub fn native(path: &Path) -> PathBuf {
    path.components().collect()
}

fn sets_disable_all_hooks(file: &Path) -> bool {
    let Ok(text) = std::fs::read_to_string(file) else {
        return false;
    };
    let Ok(json) = serde_json::from_str::<Value>(&text) else {
        return false;
    };
    json.get("disableAllHooks") == Some(&Value::Bool(true))
}

/// The program path as the shell on the hook host reads it. On Windows the
/// shell is Git Bash, which reads `/` and takes `\` as an escape.
fn shell_path(program: &Path) -> String {
    let text = program.to_string_lossy().into_owned();
    if cfg!(windows) {
        text.replace('\\', "/")
    } else {
        text
    }
}

/// POSIX single quotes: nothing inside stays special except `'`, which
/// closes the quote, adds an escaped quote and opens it again. A word of
/// plain characters stays bare.
fn shell_quote(text: &str) -> String {
    let plain = !text.is_empty()
        && text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./:=@%+,".contains(c));
    if plain {
        return text.to_owned();
    }
    format!("'{}'", text.replace('\'', r"'\''"))
}

// --- Codex ------------------------------------------------------------------

/// The Codex hook events a status hook is registered on (ADR-0085 D4, D5,
/// D9). Codex fails open on every one of them when the hook exits 1 or does
/// not start; only exit 2 blocks, and `ralphy hook status` never exits 2.
/// Measured with codex-cli 0.160.0 (Windows) and 0.162.1 (WSL), 2026-10-28;
/// the facts the Codex code below names are from the same probes.
pub const CODEX_STATUS_EVENTS: [&str; 7] = [
    "SessionStart",
    "UserPromptSubmit",
    "PreToolUse",
    "PermissionRequest",
    "PostToolUse",
    "Stop",
    "Interrupt",
];

/// The flag without which Codex runs no hook at all, and says nothing about
/// it.
pub const CODEX_HOOK_TRUST_FLAG: &str = "--dangerously-bypass-hook-trust";

/// The arguments that give Codex the status hooks at launch: the trust flag,
/// then one `-c hooks.<Event>=[...]` per event of `status_events`, each
/// running `<exe> hook status codex`. The caller puts them on the same side
/// of `exec` as its other `-c`: Codex drops a `-c` list given before `exec`
/// when another one follows it.
pub fn codex_hook_args(exe: &Path, status_events: &[&str]) -> Vec<String> {
    let command = toml_basic_string(&codex_hook_command(exe));
    let mut args = vec![CODEX_HOOK_TRUST_FLAG.to_owned()];
    for event in status_events {
        args.push("-c".to_owned());
        args.push(format!(
            "hooks.{event}=[{{hooks=[{{type=\"command\",command={command}}}]}}]"
        ));
    }
    args
}

/// Whether the project at `dir`, or a folder above it up to the one with
/// `.git`, has Codex hooks of its own: a `.codex/hooks.json`, or a root `hooks`
/// key in `.codex/config.toml`; a file that cannot be read or parsed counts.
/// [`CODEX_HOOK_TRUST_FLAG`] would run them with no trust prompt (spike M1 of
/// 2026-10-28), so the caller then gives neither (ADR-0085, 2026-10-29).
pub fn codex_project_has_hooks(dir: &Path) -> bool {
    let mut layers = Vec::new();
    for layer in dir.ancestors() {
        layers.push(layer);
        if layer.join(".git").exists() {
            return layers.into_iter().any(codex_layer_has_hooks);
        }
    }
    codex_layer_has_hooks(dir)
}

/// [`codex_project_has_hooks`] for one folder.
fn codex_layer_has_hooks(dir: &Path) -> bool {
    let codex = dir.join(".codex");
    if !matches!(codex.join("hooks.json").try_exists(), Ok(false)) {
        return true;
    }
    match std::fs::read_to_string(codex.join("config.toml")) {
        Ok(text) => text
            .parse::<toml::Table>()
            .map_or(true, |table| table.contains_key("hooks")),
        Err(e) => e.kind() != std::io::ErrorKind::NotFound,
    }
}

/// The hook command line Codex runs on this platform.
pub fn codex_hook_command(exe: &Path) -> String {
    codex_hook_command_for(exe, cfg!(windows))
}

/// The hook command line for a Windows or a Unix Codex. On Windows Codex runs
/// a hook through `pwsh -NoProfile -Command`, where a quoted path is a string
/// and not a call, so the command starts with the call operator `&`; the
/// path has `/`. On Unix it is a shell command line.
pub fn codex_hook_command_for(exe: &Path, windows: bool) -> String {
    let exe = crate::exe::plain_path(exe.to_path_buf());
    let text = exe.to_string_lossy();
    if windows {
        let path = text
            .replace('\\', "/")
            .replace('`', "``")
            .replace('$', "`$")
            .replace('"', "`\"");
        format!("& \"{path}\" hook status codex")
    } else {
        format!("{} hook status codex", shell_quote(&text))
    }
}

/// The command `ralphy doctor` runs to check the Codex hook: the program and
/// its arguments, run without a shell, the same program and arguments a
/// Codex hook runs.
pub fn codex_check_hook(exe: &Path) -> HookCommand {
    HookCommand {
        program: crate::exe::plain_path(exe.to_path_buf()),
        args: ["hook", "status", "codex"].map(String::from).to_vec(),
        form: HookForm::Exec,
        reason: "the check runs the program and the arguments of the Codex hook without a shell"
            .to_owned(),
    }
}

/// `text` as a TOML basic string: in quotes, with `\` and `"` escaped. A
/// `\` left bare in a Windows path is an invalid TOML escape, and Codex then
/// rejects the whole `-c`.
fn toml_basic_string(text: &str) -> String {
    format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
}

#[cfg(test)]
mod tests;
