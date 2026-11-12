//! The `ralphy config` subcommand (ADR-0010): the per-repo
//! `.ralphy/settings.json`, whose keys are [`SUPPORTED_KEYS`].
//!
//! A run knob resolves as: the per-run flag, then `settings.json`, then a
//! hardcoded default ([`resolve`]). The two Copilot effort keys have no flag
//! (#227), and the adapter clamps their level per model (#233). Cursor has no
//! model key: `--model` is mandatory on that vendor, so it has no unset state.
//! Gemini model ids are validated at `set` time only (#257, ADR-0043 D8).

use std::path::PathBuf;

use anyhow::{bail, Result};
use clap::{Args, Subcommand};
use ralphy_agent_claude::ClaudeSettings;
use ralphy_core::{git, Settings, Workspace};

use crate::runlock;

mod resolve;
mod view;
mod write;

pub(crate) use resolve::{
    parse_branch_mode, resolve_assignee, resolve_effort, resolve_opencode_model,
    resolve_optional_model, resolve_remote_control, resolve_str, resolve_u64,
};
#[cfg(test)]
use view::config_json;
pub(crate) use view::get;
#[cfg(test)]
use write::cleartext_remote_host;
pub(crate) use write::{set, unset};

#[derive(Args)]
pub(crate) struct ConfigArgs {
    /// Any folder inside the repo.
    #[arg(long, default_value = ".")]
    pub repo: PathBuf,

    #[command(subcommand)]
    pub command: ConfigCommand,
}

#[derive(Subcommand)]
pub(crate) enum ConfigCommand {
    /// Save a setting in `.ralphy/settings.json`.
    Set {
        /// The key to set: `opencode.model`, `base_branch`, `branch_mode`,
        /// `verify.command` (the command that checks an issue's work when the
        /// plan has no checks), or a Claude setting (`claude.plan_model`,
        /// `claude.plan_effort`, `claude.default_exec_model`,
        /// `claude.exec_effort`, `claude.max_minutes_per_issue`). The model,
        /// effort and time defaults apply only to Claude today.
        // verify.command: ADR-0011.
        key: String,
        /// The value to save.
        value: String,
    },
    /// Remove a setting from `.ralphy/settings.json`.
    Unset {
        /// The setting to remove.
        key: String,
    },
    /// Print every saved setting.
    Get {
        /// Print one JSON object with every setting and its value (`null` when
        /// it is not set).
        #[arg(long)]
        json: bool,
    },
}

/// Dispatch a `config` subcommand.
pub(crate) fn run(args: ConfigArgs) -> Result<()> {
    let repo_root = git::resolve_toplevel(&args.repo)?;
    let ws = Workspace::new(&repo_root);
    match args.command {
        ConfigCommand::Set { key, value } => {
            // Mutate (ADR-0036 §2/§6): refuse while a run holds this repo's lock.
            runlock::guard_run_lock(&ws, "config set", runlock::pid_is_alive)?;
            set(&ws, &key, &value)
        }
        ConfigCommand::Unset { key } => {
            runlock::guard_run_lock(&ws, "config unset", runlock::pid_is_alive)?;
            unset(&ws, &key)
        }
        ConfigCommand::Get { json } => get(&ws, json),
    }
}

/// The single source of truth for every supported `config` key. `help`,
/// validation (`require_known_key`), and the `set`/`unset`/`get` handlers all
/// derive their key set from this registry so they never drift (a registry key
/// lacking a handler arm hits `unreachable!()`, caught by the coverage test).
/// Order matches the `supported_keys_help()` rendering and every `set`/`unset`/
/// `get` arm.
const SUPPORTED_KEYS: &[&str] = &[
    "opencode.model",
    "base_branch",
    "branch_mode",
    "remote_control",
    "queue.assignee",
    "queue.trust_all_comments",
    "verify.command",
    "verify.require_verify_gate",
    "files.encoding",
    "files.upload_max_mb",
    "runs.keep",
    "events.url",
    "events.token",
    "claude.plan_model",
    "claude.plan_effort",
    "claude.default_exec_model",
    "claude.exec_effort",
    "claude.max_minutes_per_issue",
    "claude.console_name",
    "claude.console_classic_screen",
    "claude.run_without_guard_proof_i_understand_the_risk",
    "copilot.plan_model",
    "copilot.exec_model",
    "copilot.plan_effort",
    "copilot.exec_effort",
    "copilot.allow_builtin_mcp_servers_i_understand_the_risk",
    "cursor.allow_codebase_indexing_i_understand_the_risk",
    "antigravity.plan_model",
    "antigravity.exec_model",
    "antigravity.plan_effort",
    "antigravity.exec_effort",
    "antigravity.run_without_guard_proof_i_understand_the_risk",
    "gemini.plan_model",
    "gemini.exec_model",
];

/// The trailing parenthetical the key list carries in `--help`-style docs and the
/// unknown-key error. Kept beside the registry so `supported_keys_help()` is the
/// one place the two are joined.
///
/// The decisions behind the keys: events ADR-0019, verify gate ADR-0011 and
/// ADR-0015, files.encoding ADR-0036, the Copilot MCP key ADR-0041 D7, the
/// Cursor key ADR-0042 D6, the Gemini models ADR-0043 D8.
const SUPPORTED_KEYS_NOTE: &str = "\
(events.url and events.token set where run events are sent. They are saved for \
each repo in ~/.ralphy/events.toml, not in settings.json. \
verify.command is the command that checks an issue's work when the plan has no \
checks. verify.require_verify_gate=true leaves an unchecked issue open for a \
person instead of closing it. \
queue.trust_all_comments=true lets a run read every issue comment; by default \
it reads only comments from owners, members and collaborators. \
runs.keep is how many run folders a repo keeps (default 50; 0 keeps every folder). \
files.encoding is the encoding the workbench uses for a file that is not UTF-8 \
or UTF-16, for example windows-1252 (the default) or shift_jis. \
files.upload_max_mb is the largest file the workbench may upload, in MB \
(default 25, at most 1024). \
The model, effort and time defaults apply only to Claude today; OpenCode's model \
is opencode.model. \
copilot.plan_model, copilot.exec_model, copilot.plan_effort and \
copilot.exec_effort set Copilot's model and effort for each phase. \
copilot.allow_builtin_mcp_servers_i_understand_the_risk=true gives Copilot back \
its built-in GitHub MCP server, which has your GitHub credentials and can open a \
pull request by itself. \
cursor.allow_codebase_indexing_i_understand_the_risk=true lets Cursor work in a \
repository that still allows Cursor to upload its code for indexing. \
claude.console_name=true starts a Claude console in the workbench with its \
console name as the session name, such as wb-fincal-1. Other Claude sessions \
use that name to send it messages. When off, Claude picks the name. \
claude.console_classic_screen=true starts a Claude console in the workbench in \
Claude's classic view, so the conversation stays in the console's text, with a \
scrollbar. When off, Claude follows its own settings. \
claude.run_without_guard_proof_i_understand_the_risk=true lets a Claude session go \
on when Ralphy cannot prove that its hooks run, so the guard may not run. Each such \
session writes a warning to the run. Only `ralphy config set` on this machine can \
turn it on. \
antigravity.plan_model and antigravity.exec_model set the Antigravity model for \
each phase, as one word such as gemini-3.8-flash; antigravity.plan_effort and \
antigravity.exec_effort set its effort. \
antigravity.run_without_guard_proof_i_understand_the_risk=true lets an Antigravity \
session go on when Ralphy cannot prove that its hooks run, so the guard may not run. \
Only `ralphy config set` on this machine can turn it on. \
gemini.plan_model and gemini.exec_model set Gemini's model for each phase; \
without them, Gemini makes one extra, billed call per turn to choose a model.)";

/// Human-readable list of every supported `config` key, derived from
/// [`SUPPORTED_KEYS`] so it never drifts from the validated set. Reused in the
/// unknown-key error and the help notes.
fn supported_keys_help() -> String {
    format!(
        "supported keys: {} {SUPPORTED_KEYS_NOTE}",
        SUPPORTED_KEYS.join(", ")
    )
}

fn require_known_key(key: &str) -> Result<()> {
    if SUPPORTED_KEYS.contains(&key) {
        Ok(())
    } else {
        bail!("unknown config key '{key}'; {}", supported_keys_help())
    }
}

/// The persisted `claude.run_without_guard_proof_i_understand_the_risk` of
/// the repo at `repo`, for the one-shot Claude sessions. A file or section
/// that does not parse warns and reads as `false`, like a run does.
pub(crate) fn claude_guard_proof_waiver(repo: &std::path::Path) -> bool {
    let parsed = Settings::load(&Workspace::new(repo))
        .and_then(|s| s.agent_settings::<ClaudeSettings>(ClaudeSettings::SECTION));
    match parsed {
        Ok(c) => c.run_without_guard_proof_i_understand_the_risk,
        Err(e) => {
            tracing::warn!(error = %e, "unreadable claude settings — the hook proof stays on");
            false
        }
    }
}

#[cfg(test)]
mod set_tests;
#[cfg(test)]
mod test_support;
#[cfg(test)]
mod tests;
