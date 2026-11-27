//! The Codex hook helpers: the launch arguments, the command line and the
//! project check.

use std::path::Path;

use super::{shell_quote, HookCommand, HookForm};

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
