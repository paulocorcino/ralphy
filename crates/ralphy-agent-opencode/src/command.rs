//! Building the headless `opencode run` invocation both `plan` and `execute`
//! go through.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use ralphy_adapter_support::DENIED_FORGE_WRITES;

/// The located `opencode` (its installer folder included), else the bare name so
/// a spawn error names the vendor.
pub(crate) fn resolve_opencode_program() -> OsString {
    ralphy_proc_util::opencode::locate_opencode()
        .map_or_else(|| OsString::from("opencode"), PathBuf::into_os_string)
}

/// Build the headless `opencode run` command both `plan` and `execute` go through
/// — the single point that fixes the invocation, always passes
/// `--dangerously-skip-permissions` (the headless-hang guard, ADR-0005 D5) and
/// `--format json`, omits `-m` unless the operator set one (D4), passes
/// `--variant` only when set (D3), injects `OPENCODE_CONFIG_CONTENT` (see
/// [`opencode_config`]), runs in `root`, and defensively removes both
/// `ANTHROPIC_API_KEY` and `OPENAI_API_KEY` so an inherited key can't switch
/// the run to metered API billing (D6). The prompt is written on stdin.
pub(crate) fn build_opencode_command(
    model: Option<&str>,
    variant: Option<&str>,
    root: &Path,
    skills_dir: Option<&Path>,
) -> Command {
    command_with(model, variant, root, skills_dir, None)
}

/// The state plugin of a run (ADR-0085 D2): the plugin file `OpenCode` loads,
/// and the status file the plugin appends to.
pub(crate) struct StatePlugin<'a> {
    pub(crate) plugin: &'a Path,
    pub(crate) status: &'a Path,
}

/// [`build_opencode_command`] for a plan or an execute, which also loads the
/// state plugin and points it at the status file.
pub(crate) fn build_watched_opencode_command(
    model: Option<&str>,
    variant: Option<&str>,
    root: &Path,
    skills_dir: Option<&Path>,
    state: &StatePlugin<'_>,
) -> Command {
    command_with(model, variant, root, skills_dir, Some(state))
}

fn command_with(
    model: Option<&str>,
    variant: Option<&str>,
    root: &Path,
    skills_dir: Option<&Path>,
    state: Option<&StatePlugin<'_>>,
) -> Command {
    // The located path: on Windows `opencode` ships as an npm `.cmd` shim with no
    // `.exe`, which a bare `Command::new("opencode")` cannot find.
    let mut cmd = Command::new(resolve_opencode_program());
    cmd.arg("run")
        .arg("--format")
        .arg("json")
        .arg("--dangerously-skip-permissions")
        // Route opencode's own logs (logfmt) onto stderr at ERROR level. Some
        // providers (Z.ai `zai-coding-plan`/GLM) never emit a `{type:"error"}`
        // JSON event on a quota block: their ai-sdk treats the `AI_APICallError:
        // Usage limit reached` as a *retryable* stream error and loops on backoff,
        // printing it only to the server log — so the `--format json` stream stays
        // silent and the run stalls until the wall timeout. `--print-logs` brings
        // that line onto the stderr we already drain, where `parse_opencode_log_limit`
        // can see it and classify the run as `Limit` instead of a mute `Timeout`
        // (observed live 2026-07-11, FinCal #71, glm-5.2). ERROR keeps the combined
        // log lean; the quota line is logged at ERROR.
        .arg("--print-logs")
        .arg("--log-level")
        .arg("ERROR");
    if let Some(m) = model {
        cmd.arg("-m").arg(m);
    }
    if let Some(v) = variant {
        cmd.arg("--variant").arg(v);
    }
    cmd.current_dir(root)
        .env("PWD", root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env(
            "OPENCODE_CONFIG_CONTENT",
            opencode_config(skills_dir, state.map(|s| s.plugin)),
        )
        .env_remove("ANTHROPIC_API_KEY")
        .env_remove("OPENAI_API_KEY");
    if let Some(state) = state {
        cmd.env(crate::status::STATUS_ENV, state.status);
    }
    cmd
}

// The daemon names a console's plugin the same way, in
// `ralphy-daemon/src/agent_state/opencode.rs`: it imports no adapter crate
// (ADR-0059 §5).
/// The `plugin` entry that names `path`: a `file:///` URL with forward
/// slashes, the form `OpenCode` loaded on Windows and on Linux (measured with
/// `OpenCode` 1.18.4 and 1.19.0). A Windows verbatim prefix is dropped.
pub(crate) fn plugin_entry(path: &Path) -> String {
    let text = path.to_string_lossy().replace('\\', "/");
    let text = text.strip_prefix("//?/").unwrap_or(&text);
    format!("file:///{}", text.trim_start_matches('/'))
}

/// The JSON injected as `OPENCODE_CONFIG_CONTENT`: `skills.paths` when the
/// session has materialized skills (plan and execute, ADR-0005 D7), and in
/// every session a `permission.bash` map that denies the forge writes
/// (ADR-0072 D6, ADR-0005 D5 amendment). opencode applies an explicit `deny`
/// even under `--dangerously-skip-permissions`, and the refused call reaches
/// the model as a tool error; the run goes on (measured with opencode 1.18.32).
/// The map has no `"*"` entry, so the operator's own bash rules still apply.
/// A plan or an execute also names Ralphy's state plugin under `plugin`;
/// `OpenCode` joins that list with the operator's own plugins.
fn opencode_config(skills_dir: Option<&Path>, plugin: Option<&Path>) -> String {
    let mut bash = serde_json::Map::new();
    for words in DENIED_FORGE_WRITES {
        let command = words.join(" ");
        bash.insert(format!("{command} *"), "deny".into());
        bash.insert(command, "deny".into());
    }
    let mut config = serde_json::Map::new();
    config.insert("permission".into(), serde_json::json!({ "bash": bash }));
    if let Some(dir) = skills_dir {
        // Canonicalized for robustness; on failure the path is used as-is.
        let abs = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
        config.insert("skills".into(), serde_json::json!({ "paths": [abs] }));
    }
    if let Some(plugin) = plugin {
        config.insert("plugin".into(), serde_json::json!([plugin_entry(plugin)]));
    }
    serde_json::Value::Object(config).to_string()
}

#[cfg(test)]
mod tests;
