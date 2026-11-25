//! Building the headless `opencode run` invocation both `plan` and `execute`
//! go through.

use std::path::Path;
use std::process::{Command, Stdio};

use ralphy_adapter_support::DENIED_FORGE_WRITES;
use ralphy_proc_util::opencode::{LocatedOpenCode, OpenCodeMajor};
use tracing::info;

/// The `OpenCode` of this host and its checked version: the program comes from
/// the shared locator, and a version Ralphy does not support is an error.
pub(crate) fn checked_opencode() -> anyhow::Result<LocatedOpenCode> {
    ralphy_proc_util::opencode::located_opencode().map_err(anyhow::Error::new)
}

/// The model and variant the operator set, and the session's folder and skills.
#[derive(Clone, Copy)]
pub(crate) struct RunArgs<'a> {
    pub(crate) model: Option<&'a str>,
    pub(crate) variant: Option<&'a str>,
    pub(crate) root: &'a Path,
    pub(crate) skills_dir: Option<&'a Path>,
}

/// Build the headless `opencode run` command of a session, with the argv of
/// the located major (ADR-0094 D3): the JSON stream, the permission flag
/// (`--dangerously-skip-permissions` on 1.x, `--standalone --auto` on 2.x),
/// the logs on stderr, `-m` only when set, the deny map and skills in
/// `OPENCODE_CONFIG_CONTENT`, the folder, and both API keys removed (ADR-0005
/// D6). The prompt is written on stdin.
pub(crate) fn build_opencode_command(located: &LocatedOpenCode, run: RunArgs<'_>) -> Command {
    command_with(located, run, None)
}

/// The state plugin of a run (ADR-0085 D2): the plugin file `OpenCode` loads,
/// and the status file the plugin appends to.
pub(crate) struct StatePlugin<'a> {
    pub(crate) plugin: &'a Path,
    pub(crate) status: &'a Path,
}

/// [`build_opencode_command`] for a plan or an execute. On 1.x it also loads
/// the state plugin and points it at the status file; 2.x loads no plugin
/// file (ADR-0094 D5a).
pub(crate) fn build_watched_opencode_command(
    located: &LocatedOpenCode,
    run: RunArgs<'_>,
    state: &StatePlugin<'_>,
) -> Command {
    command_with(located, run, Some(state))
}

/// `opencode models`; on 2.x with `--standalone`, measured to start no
/// service (ADR-0094 D6e).
pub(crate) fn build_models_command(located: &LocatedOpenCode) -> Command {
    let mut cmd = Command::new(&located.program);
    cmd.arg("models");
    match located.version.major {
        OpenCodeMajor::V1 => {}
        OpenCodeMajor::V2 => {
            cmd.arg("--standalone");
        }
    }
    cmd
}

fn command_with(
    located: &LocatedOpenCode,
    run: RunArgs<'_>,
    state: Option<&StatePlugin<'_>>,
) -> Command {
    let mut cmd = Command::new(&located.program);
    let state = match located.version.major {
        OpenCodeMajor::V1 => {
            v1_args(&mut cmd, run);
            state
        }
        OpenCodeMajor::V2 => {
            v2_args(&mut cmd, run);
            None
        }
    };
    cmd.current_dir(run.root)
        .env("PWD", run.root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env(
            "OPENCODE_CONFIG_CONTENT",
            opencode_config(run.skills_dir, state.map(|s| s.plugin)),
        )
        .env_remove("ANTHROPIC_API_KEY")
        .env_remove("OPENAI_API_KEY");
    if let Some(state) = state {
        cmd.env(crate::status::STATUS_ENV, state.status);
    }
    cmd
}

// `--print-logs` brings a provider quota block that never reaches the JSON
// stream onto stderr, where `parse_opencode_log_limit` sees it (FinCal #71).
fn v1_args(cmd: &mut Command, run: RunArgs<'_>) {
    cmd.args(["run", "--format", "json", "--dangerously-skip-permissions"])
        .args(["--print-logs", "--log-level", "ERROR"]);
    if let Some(m) = run.model {
        cmd.arg("-m").arg(m);
    }
    if let Some(v) = run.variant {
        cmd.arg("--variant").arg(v);
    }
}

fn v2_args(cmd: &mut Command, run: RunArgs<'_>) {
    cmd.args(["run", "--format", "json", "--standalone", "--auto"])
        .args(["--print-logs", "--log-level", "error"]);
    match (run.model, run.variant) {
        (Some(m), Some(v)) => {
            cmd.arg("-m").arg(format!("{m}#{v}"));
        }
        (Some(m), None) => {
            cmd.arg("-m").arg(m);
        }
        (None, Some(v)) => {
            info!(
                variant = v,
                "OpenCode 2 takes a variant only with a model; not passed"
            );
        }
        (None, None) => {}
    }
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
/// 2.x reads the same key as its `shell` rules and refuses the call under
/// `--auto` (measured with 2.3.2 on Windows).
/// The map has no `"*"` entry, so the operator's own bash rules still apply.
/// A 1.x plan or execute also names Ralphy's state plugin under `plugin`;
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
pub(crate) mod tests;
