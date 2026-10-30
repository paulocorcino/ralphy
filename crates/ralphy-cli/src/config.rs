//! The `ralphy config` subcommand (ADR-0010).
//!
//! Manages per-repo `.ralphy/settings.json`. Supported keys: `opencode.model`
//! (OpenCode execution-model default, #47), the agent-agnostic `base_branch` and
//! `branch_mode`, the Claude-only run defaults under `claude.*`
//! (`plan_model`, `plan_effort`, `default_exec_model`, `exec_effort`,
//! `max_minutes_per_issue`), and the Copilot per-phase model overrides under
//! `copilot.*` (`plan_model`, `exec_model`, #232; `plan_effort`, `exec_effort`,
//! #233 — a requested level, CLAMPED per model by the adapter before it reaches
//! argv). Cursor carries exactly one key,
//! `cursor.allow_codebase_indexing_i_understand_the_risk` (#243, ADR-0042 D6) —
//! it has no persisted model keys, because `--model` is mandatory on that vendor
//! and so has no "unset" state to persist. Gemini carries the two per-phase pins
//! `gemini.plan_model` / `gemini.exec_model` (#257, ADR-0043 D8), validated
//! against the vendor's id set at `set` time only. The budget knob stays Claude-only today — a Codex equivalent is
//! deferred. Each resolves with the same precedence: per-run flag then
//! `settings.json` then a hardcoded default — except the two Copilot effort keys,
//! which have no flag at all (#227 owns whether `--plan-effort`/`--exec-effort`
//! become every adapter's vocabulary). Copilot's default is `None`, omitting the
//! flag.

use std::path::PathBuf;

use anyhow::{anyhow, bail, Context, Result};
use clap::{Args, Subcommand};
use ralphy_agent_claude::ClaudeSettings;
use ralphy_agent_copilot::CopilotSettings;
use ralphy_agent_cursor::CursorSettings;
use ralphy_agent_gemini::GeminiSettings;
use ralphy_agent_opencode::OpenCodeSettings;
use ralphy_core::{git, gitignore, Effort, Settings, Workspace};

use crate::runlock;

mod resolve;
mod view;

pub use resolve::{
    parse_branch_mode, resolve_assignee, resolve_effort, resolve_opencode_model,
    resolve_optional_model, resolve_remote_control, resolve_str, resolve_u64,
};
#[cfg(test)]
use view::config_json;
pub use view::get;

#[derive(Args)]
pub struct ConfigArgs {
    /// Any folder inside the repo.
    #[arg(long, default_value = ".")]
    pub repo: PathBuf,

    #[command(subcommand)]
    pub command: ConfigCommand,
}

#[derive(Subcommand)]
pub enum ConfigCommand {
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
pub fn run(args: ConfigArgs) -> Result<()> {
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
runs.keep is how many run folders a repo keeps (default 50; 0 keeps every folder). files.encoding is the encoding the workbench uses for a file that is not UTF-8 \
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
scrollbar. When off, Claude follows its own settings. claude.run_without_guard_proof_i_understand_the_risk=true lets a Claude session go on when Ralphy cannot prove that its hooks run, so the guard may not run. Each such session writes a warning to the run. Only `ralphy config set` on this machine can turn it on. \
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

/// Load-mutate-store the Claude section of the settings. The section is opaque
/// JSON to the core; a malformed section is a hard error here (unlike run-time
/// resolution, which warns and defaults) so `config set` fails loud.
fn with_claude(s: &mut Settings, f: impl FnOnce(&mut ClaudeSettings)) -> Result<()> {
    let mut c: ClaudeSettings = s.agent_settings(ClaudeSettings::SECTION)?;
    f(&mut c);
    s.set_agent_settings(ClaudeSettings::SECTION, &c)
}

/// Load-mutate-store the OpenCode section; same contract as [`with_claude`].
fn with_opencode(s: &mut Settings, f: impl FnOnce(&mut OpenCodeSettings)) -> Result<()> {
    let mut o: OpenCodeSettings = s.agent_settings(OpenCodeSettings::SECTION)?;
    f(&mut o);
    s.set_agent_settings(OpenCodeSettings::SECTION, &o)
}

/// Load-mutate-store the Copilot section; same contract as [`with_claude`].
fn with_copilot(s: &mut Settings, f: impl FnOnce(&mut CopilotSettings)) -> Result<()> {
    let mut c: CopilotSettings = s.agent_settings(CopilotSettings::SECTION)?;
    f(&mut c);
    s.set_agent_settings(CopilotSettings::SECTION, &c)
}

/// Load-mutate-store the Cursor section; same contract as [`with_claude`].
fn with_cursor(s: &mut Settings, f: impl FnOnce(&mut CursorSettings)) -> Result<()> {
    let mut c: CursorSettings = s.agent_settings(CursorSettings::SECTION)?;
    f(&mut c);
    s.set_agent_settings(CursorSettings::SECTION, &c)
}

/// Load-mutate-store the Gemini section; same contract as [`with_claude`].
fn with_gemini(s: &mut Settings, f: impl FnOnce(&mut GeminiSettings)) -> Result<()> {
    let mut g: GeminiSettings = s.agent_settings(GeminiSettings::SECTION)?;
    f(&mut g);
    s.set_agent_settings(GeminiSettings::SECTION, &g)
}

pub fn set(ws: &Workspace, key: &str, value: &str) -> Result<()> {
    require_known_key(key)?;
    if value.trim().is_empty() {
        bail!("value for '{key}' must not be empty — use `config unset {key}` to clear it");
    }
    // The CloudEvents sink knobs live in the global per-repo store
    // (`~/.ralphy/events.toml`), keyed by slug — never in `.ralphy/settings.json`
    // (ADR-0019). Route them there and return before the settings path.
    if key == "events.url" || key == "events.token" {
        return set_events_key(ws, key, value);
    }
    let mut s = Settings::load(ws)?;
    set_core_key(&mut s, key, value)?;
    s.save(ws)?;
    gitignore::ensure_ralphy_ignored(ws.repo_root())?;
    println!("{key} = {value}");
    Ok(())
}

/// Save `events.url` or `events.token` in the global store for this repo.
fn set_events_key(ws: &Workspace, key: &str, value: &str) -> Result<()> {
    let slug = git::project_slug(ws.repo_root());
    let mut store = crate::events::config::EventsStore::load()?;
    match key {
        "events.url" => {
            if let Some(host) = cleartext_remote_host(value) {
                // Warn, never refuse: the daemon is TLS-aware, not
                // TLS-enforcing (ADR-0032 amendment, audit F6/F14) — an
                // `http://` sink on a LAN is the operator's call, and this
                // line is what makes it an informed one.
                println!("warning: events.url is plain http — the events bearer will travel in cleartext to {host}");
            }
            store.set_url(&slug, value);
        }
        "events.token" => store.set_token(&slug, value),
        _ => unreachable!(),
    }
    store.save()?;
    // Never echo the secret token back to stdout — mask it like `config get`.
    if key == "events.token" {
        println!("{key} = {}", crate::telegram::config::masked_token(value));
    } else {
        println!("{key} = {value}");
    }
    Ok(())
}

/// Set a key of the core settings; a key of an agent section goes on to
/// [`set_agent_key`].
fn set_core_key(s: &mut Settings, key: &str, value: &str) -> Result<()> {
    match key {
        "verify.command" => s.verify.command = Some(value.to_owned()),
        "verify.require_verify_gate" => {
            let b = value.parse::<bool>().with_context(|| {
                format!("verify.require_verify_gate must be 'true' or 'false', got '{value}'")
            })?;
            s.verify.require_verify_gate = Some(b);
        }
        "base_branch" => s.base_branch = Some(value.to_owned()),
        "files.encoding" => s.files.encoding = Some(value.to_owned()),
        "files.upload_max_mb" => {
            let n = value
                .parse::<u64>()
                .ok()
                .filter(|n| *n >= 1)
                .ok_or_else(|| anyhow!("files.upload_max_mb must be a whole number of MB, at least 1, got '{value}'"))?;
            s.files.upload_max_mb = Some(n);
        }
        "runs.keep" => {
            let n = value.parse::<u64>().with_context(|| {
                format!("runs.keep must be a non-negative integer (0 keeps every run folder), got '{value}'")
            })?;
            s.runs.keep = Some(n);
        }
        "queue.assignee" => s.queue.assignee = Some(value.to_owned()),
        "queue.trust_all_comments" => {
            let b = value.parse::<bool>().with_context(|| {
                format!("queue.trust_all_comments must be 'true' or 'false', got '{value}'")
            })?;
            s.queue.trust_all_comments = Some(b);
        }
        "branch_mode" => {
            // Validate through the shared parser; store the canonical lowercase
            // string so resolution and `config get` see one form.
            parse_branch_mode(value)?;
            s.branch_mode = Some(value.to_owned());
        }
        "remote_control" => {
            let b = value.parse::<bool>().with_context(|| {
                format!("remote_control must be 'true' or 'false', got '{value}'")
            })?;
            s.remote_control = Some(b);
        }
        _ => set_agent_key(s, key, value)?,
    }
    Ok(())
}

/// Set a key of an agent section (`opencode.*`, `claude.*`, `copilot.*`,
/// `gemini.*`, `cursor.*`).
fn set_agent_key(s: &mut Settings, key: &str, value: &str) -> Result<()> {
    match key {
        "opencode.model" => with_opencode(s, |o| o.model = Some(value.to_owned()))?,
        "claude.plan_model" => with_claude(s, |c| c.plan_model = Some(value.to_owned()))?,
        "claude.plan_effort" | "claude.exec_effort" => set_claude_effort(s, key, value)?,
        "claude.default_exec_model" => {
            with_claude(s, |c| c.default_exec_model = Some(value.to_owned()))?
        }
        "claude.max_minutes_per_issue" => {
            let n = value.parse::<u64>().with_context(|| {
                format!("claude.max_minutes_per_issue must be a non-negative integer (0 disables the per-issue cap), got '{value}'")
            })?;
            with_claude(s, |c| c.max_minutes_per_issue = Some(n))?;
        }
        "claude.console_name" => {
            let b = value
                .parse::<bool>()
                .with_context(|| format!("{key} must be 'true' or 'false', got '{value}'"))?;
            with_claude(s, |c| c.console_name = b)?
        }
        "claude.console_classic_screen" => {
            let b = value
                .parse::<bool>()
                .with_context(|| format!("{key} must be 'true' or 'false', got '{value}'"))?;
            with_claude(s, |c| c.console_classic_screen = b)?
        }
        // The escape of the hook proof (ADR-0072 D6): local-only, the daemon
        // refuses to set it.
        "claude.run_without_guard_proof_i_understand_the_risk" => {
            let b = value
                .parse::<bool>()
                .with_context(|| format!("{key} must be 'true' or 'false', got '{value}'"))?;
            with_claude(s, |c| c.run_without_guard_proof_i_understand_the_risk = b)?
        }
        "copilot.plan_model" => with_copilot(s, |c| c.plan_model = Some(value.to_owned()))?,
        "copilot.exec_model" => with_copilot(s, |c| c.exec_model = Some(value.to_owned()))?,
        "copilot.plan_effort" | "copilot.exec_effort" => set_copilot_effort(s, key, value)?,
        // The verbosity IS the safety feature (ADR-0041 D7): the hatch gives
        // Copilot back a credentialled MCP server that can open a PR on its own.
        "copilot.allow_builtin_mcp_servers_i_understand_the_risk" => {
            let b = value
                .parse::<bool>()
                .with_context(|| format!("{key} must be 'true' or 'false', got '{value}'"))?;
            with_copilot(s, |c| c.allow_builtin_mcp_servers_i_understand_the_risk = b)?
        }
        "gemini.plan_model" | "gemini.exec_model" => set_gemini_model(s, key, value)?,
        // Same reasoning, a different capability (ADR-0042 D6): the hatch lets a
        // run upload the operator's repository to the vendor. Ralphy never denies
        // the capability — it denies a SILENT one.
        "cursor.allow_codebase_indexing_i_understand_the_risk" => {
            let b = value
                .parse::<bool>()
                .with_context(|| format!("{key} must be 'true' or 'false', got '{value}'"))?;
            with_cursor(s, |c| c.allow_codebase_indexing_i_understand_the_risk = b)?
        }
        _ => unreachable!(),
    }
    Ok(())
}

/// Set `claude.plan_effort` or `claude.exec_effort` to a core effort level.
fn set_claude_effort(s: &mut Settings, key: &str, value: &str) -> Result<()> {
    value.parse::<Effort>().map_err(anyhow::Error::msg)?;
    let plan = key == "claude.plan_effort";
    with_claude(s, |c| {
        let slot = if plan {
            &mut c.plan_effort
        } else {
            &mut c.exec_effort
        };
        *slot = Some(value.to_owned());
    })
}

/// Set `copilot.plan_effort` or `copilot.exec_effort`.
///
/// Validated here, not at run time: an unrankable level is silently
/// dropped by the adapter's clamp, so an unvalidated typo would persist as
/// a setting that `config get` reports as set and that does nothing.
fn set_copilot_effort(s: &mut Settings, key: &str, value: &str) -> Result<()> {
    if !ralphy_agent_copilot::is_known_effort(value) {
        bail!("{key} must be a Copilot reasoning-effort level, got '{value}'");
    }
    let plan = key == "copilot.plan_effort";
    with_copilot(s, |c| {
        let slot = if plan {
            &mut c.plan_effort
        } else {
            &mut c.exec_effort
        };
        *slot = Some(value.to_owned());
    })
}

/// Set `gemini.plan_model` or `gemini.exec_model`.
///
/// Validated HERE and only here: a persisted id is refused at
/// configuration time rather than mid-run, while `--plan-model`/
/// `--exec-model` stay unfiltered so a stale local list cannot block an id
/// the vendor has started serving (ADR-0043 D8 amendment, #257).
///
/// The stored value is TRIMMED: `is_pinnable_model` trims before matching,
/// so persisting the raw string would accept ` gemini-2.5-pro `, report it
/// valid from `config get`, and 404 on every run.
fn set_gemini_model(s: &mut Settings, key: &str, value: &str) -> Result<()> {
    let value = value.trim();
    if !ralphy_agent_gemini::is_pinnable_model(value) {
        bail!(
            "{key} must be a Gemini model id the CLI still serves, got '{value}'; valid: {}",
            ralphy_agent_gemini::PINNABLE_MODELS.join(", ")
        );
    }
    let plan = key == "gemini.plan_model";
    with_gemini(s, |g| {
        if plan {
            g.plan_model = Some(value.to_owned());
        } else {
            g.exec_model = Some(value.to_owned());
        }
    })
}

pub fn unset(ws: &Workspace, key: &str) -> Result<()> {
    require_known_key(key)?;
    // The events sink knobs are cleared in the global store, not settings.json.
    if key == "events.url" || key == "events.token" {
        let slug = git::project_slug(ws.repo_root());
        let mut store = crate::events::config::EventsStore::load()?;
        // The field name is the key's suffix (`url` / `token`).
        store.clear(&slug, &key["events.".len()..]);
        store.save()?;
        println!("{key}: unset");
        return Ok(());
    }
    let mut s = Settings::load(ws)?;
    match key {
        "opencode.model" => with_opencode(&mut s, |o| o.model = None)?,
        "verify.command" => s.verify.command = None,
        "verify.require_verify_gate" => s.verify.require_verify_gate = None,
        "base_branch" => s.base_branch = None,
        "files.encoding" => s.files.encoding = None,
        "files.upload_max_mb" => s.files.upload_max_mb = None,
        "runs.keep" => s.runs.keep = None,
        "queue.assignee" => s.queue.assignee = None,
        "queue.trust_all_comments" => s.queue.trust_all_comments = None,
        "branch_mode" => s.branch_mode = None,
        "remote_control" => s.remote_control = None,
        "claude.plan_model" => with_claude(&mut s, |c| c.plan_model = None)?,
        "claude.plan_effort" => with_claude(&mut s, |c| c.plan_effort = None)?,
        "claude.default_exec_model" => with_claude(&mut s, |c| c.default_exec_model = None)?,
        "claude.exec_effort" => with_claude(&mut s, |c| c.exec_effort = None)?,
        "claude.max_minutes_per_issue" => with_claude(&mut s, |c| c.max_minutes_per_issue = None)?,
        "claude.console_name" => with_claude(&mut s, |c| c.console_name = false)?,
        "claude.console_classic_screen" => {
            with_claude(&mut s, |c| c.console_classic_screen = false)?
        }
        "claude.run_without_guard_proof_i_understand_the_risk" => with_claude(&mut s, |c| {
            c.run_without_guard_proof_i_understand_the_risk = false
        })?,
        "copilot.plan_model" => with_copilot(&mut s, |c| c.plan_model = None)?,
        "copilot.exec_model" => with_copilot(&mut s, |c| c.exec_model = None)?,
        "copilot.plan_effort" => with_copilot(&mut s, |c| c.plan_effort = None)?,
        "copilot.exec_effort" => with_copilot(&mut s, |c| c.exec_effort = None)?,
        "copilot.allow_builtin_mcp_servers_i_understand_the_risk" => with_copilot(&mut s, |c| {
            c.allow_builtin_mcp_servers_i_understand_the_risk = false
        })?,
        "cursor.allow_codebase_indexing_i_understand_the_risk" => with_cursor(&mut s, |c| {
            c.allow_codebase_indexing_i_understand_the_risk = false
        })?,
        "gemini.plan_model" => with_gemini(&mut s, |g| g.plan_model = None)?,
        "gemini.exec_model" => with_gemini(&mut s, |g| g.exec_model = None)?,
        _ => unreachable!(),
    }
    s.save(ws)?;
    println!("{key}: unset");
    Ok(())
}

/// The host of an `http://` URL that is not loopback — the one shape of
/// `events.url` under which the bearer travels in cleartext. `None` for
/// `https://`, for a loopback host (`localhost`, `127.*`, `[::1]`), or for
/// anything that is not a URL (the sink's own validation owns that). Pure
/// string work: no `url` crate for one scheme check.
fn cleartext_remote_host(url: &str) -> Option<&str> {
    let rest = url.trim().strip_prefix("http://")?;
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = authority.rsplit('@').next().unwrap_or(authority);
    let host = if let Some(v6) = host.strip_prefix('[') {
        v6.split(']').next().unwrap_or(v6)
    } else {
        host.split(':').next().unwrap_or(host)
    };
    let loopback = host.eq_ignore_ascii_case("localhost")
        || host.starts_with("127.")
        || host == "::1"
        || host.is_empty();
    (!loopback).then_some(host)
}

#[cfg(test)]
mod set_tests;
#[cfg(test)]
mod test_support;
#[cfg(test)]
mod tests;
