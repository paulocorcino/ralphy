//! The headless `agy` child: its argv (ADR-0086 D2), its environment (D7) and
//! the NDJSON line a prompt becomes on stdin.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// The binary name, used when the locator finds no `agy`, so the spawn error
/// names the vendor.
const NAME: &str = "agy";

/// What one run is pointed at: the owned root, and the model and effort words.
#[derive(Debug, Clone, Copy)]
pub(crate) struct RunFlags<'a> {
    pub(crate) root: &'a Path,
    pub(crate) model: Option<&'a str>,
    pub(crate) effort: Option<&'a str>,
}

/// The `agy` program: the locator's path, else the bare name.
pub(crate) fn agy_program() -> PathBuf {
    ralphy_proc_util::antigravity::locate_agy().unwrap_or_else(|| PathBuf::from(NAME))
}

/// The `--gemini_dir` flag that points `agy` at the owned `root`.
pub(crate) fn root_flag(root: &Path) -> OsString {
    let mut flag = OsString::from("--gemini_dir=");
    flag.push(root);
    flag
}

/// `program` with no sign-in variable of this process: every `agy` child
/// starts here.
pub(crate) fn agy_command(program: &Path) -> Command {
    scrubbed(program, std::env::vars_os().map(|(name, _)| name))
}

fn scrubbed(program: &Path, parent: impl Iterator<Item = OsString>) -> Command {
    let mut cmd = Command::new(program);
    for name in parent.filter(|name| is_sign_in_variable(name)) {
        cmd.env_remove(name);
    }
    cmd
}

/// The headless `agy` command of a run in `work_dir`, with the real environment.
pub(crate) fn build_agy_command(program: &Path, work_dir: &Path, flags: RunFlags<'_>) -> Command {
    let parent = std::env::vars_os().map(|(name, _)| name);
    build_with(program, parent, work_dir, flags)
}

/// [`build_agy_command`] over the program and the parent's variable names.
///
/// Every flag is stated, the default ones too (ADR-0040 C11). The prompt is
/// never on argv: `-p` drops stdin. `--mode` and `--sandbox` stay off (D14,
/// D15), Ralphy's budgets are the deadline (no `--print-timeout`), and remote
/// control is never offered (D12).
fn build_with(
    program: &Path,
    parent: impl Iterator<Item = OsString>,
    work_dir: &Path,
    flags: RunFlags<'_>,
) -> Command {
    let mut cmd = scrubbed(program, parent);
    cmd.current_dir(work_dir)
        .arg(root_flag(flags.root))
        .args(["--output-format", "stream-json"])
        .args(["--input-format", "stream-json"])
        .arg("--dangerously-skip-permissions")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(model) = flags.model {
        cmd.args(["--model", model]);
    }
    if let Some(level) = crate::effort::effort_flag(flags.model, flags.effort) {
        cmd.args(["--effort", level]);
    }
    cmd
}

/// Whether the child must not see `name`: an API key or Application Default
/// Credentials sign-in, which the first version does not support (D7).
///
/// Whole namespaces are matched, so a variable `agy` reads later is removed by
/// default. The match ignores case: Windows reads the environment that way.
fn is_sign_in_variable(name: &std::ffi::OsStr) -> bool {
    let upper = name.to_string_lossy().to_ascii_uppercase();
    upper.starts_with("GEMINI_") || upper.starts_with("GOOGLE_") || upper == "AGY_ADC_AUTH"
}

/// One user turn as the NDJSON line `--input-format stream-json` reads,
/// ending in one newline.
pub(crate) fn user_message(text: &str) -> String {
    let line = serde_json::json!({
        "event": "user",
        "message": { "role": "user", "content": text },
    });
    format!("{line}\n")
}

#[cfg(test)]
mod tests;
