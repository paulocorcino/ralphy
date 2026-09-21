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

use anyhow::{anyhow, bail, Result};
use clap::{Args, Subcommand};
use ralphy_agent_claude::ClaudeSettings;
use ralphy_agent_copilot::CopilotSettings;
use ralphy_agent_cursor::CursorSettings;
use ralphy_agent_gemini::GeminiSettings;
use ralphy_agent_opencode::OpenCodeSettings;
use ralphy_core::{git, gitignore, BranchMode, Effort, Settings, Workspace};

use crate::runlock;

#[derive(Args)]
pub struct ConfigArgs {
    /// Any path inside the target repo; resolved to its git toplevel.
    #[arg(long, default_value = ".")]
    pub repo: PathBuf,

    #[command(subcommand)]
    pub command: ConfigCommand,
}

#[derive(Subcommand)]
pub enum ConfigCommand {
    /// Persist a config key in `.ralphy/settings.json`.
    Set {
        /// The config key: `opencode.model`, `base_branch`, `branch_mode`,
        /// `verify.command` (the per-repo fallback verify gate, ADR-0011), or a
        /// Claude-only knob (`claude.plan_model`, `claude.plan_effort`,
        /// `claude.default_exec_model`, `claude.exec_effort`,
        /// `claude.max_minutes_per_issue`). The model/effort/budget defaults are
        /// Claude-only today (Codex deferred).
        key: String,
        /// The value to store.
        value: String,
    },
    /// Clear a config key from `.ralphy/settings.json`.
    Unset {
        /// The config key to clear.
        key: String,
    },
    /// Print all persisted config values.
    Get {
        /// Emit a single JSON object mapping every key to its resolved value
        /// (or JSON `null`); the daemon's config Query verb reads this.
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
    "verify.command",
    "verify.require_verify_gate",
    "events.url",
    "events.token",
    "claude.plan_model",
    "claude.plan_effort",
    "claude.default_exec_model",
    "claude.exec_effort",
    "claude.max_minutes_per_issue",
    "claude.console_name",
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
const SUPPORTED_KEYS_NOTE: &str = "\
(events.url/events.token configure the CloudEvents sink and are stored per repo \
in the global ~/.ralphy/events.toml, never in settings.json, ADR-0019; \
verify.command is the per-repo fallback verify gate, ADR-0011; \
verify.require_verify_gate=true parks a gateless issue for a human \
instead of closing it, ADR-0015; \
model/effort/budget defaults are Claude-only today \
(Codex deferred; OpenCode's model lives under opencode.model, #47); \
Copilot's per-phase models and reasoning effort live under copilot.plan_model / copilot.exec_model / copilot.plan_effort / copilot.exec_effort, #232/#233; \
copilot.allow_builtin_mcp_servers_i_understand_the_risk=true is the D7 escape \
hatch that hands Copilot back its credentialled builtin GitHub MCP server, \
which can open a PR on its own, #234; cursor.allow_codebase_indexing_i_understand_the_risk=true lets a Cursor run proceed in a repository that has not opted out of the vendor's codebase upload, ADR-0042 D6/#243; \
claude.console_name=true lets the workbench name a Claude console it opens (`--name wb-<repo>-<hex>`) instead of leaving the CLI to name itself; gemini.plan_model / gemini.exec_model pin a model per phase — unpinned, Gemini \
routes and pays a SECOND, billed routing call per turn, ADR-0043 D8/#257)";

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
        let slug = git::project_slug(ws.repo_root());
        let mut store = crate::events::config::EventsStore::load()?;
        match key {
            "events.url" => store.set_url(&slug, value),
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
        return Ok(());
    }
    let mut s = Settings::load(ws)?;
    match key {
        "opencode.model" => with_opencode(&mut s, |o| o.model = Some(value.to_owned()))?,
        "verify.command" => s.verify.command = Some(value.to_owned()),
        "verify.require_verify_gate" => {
            let b = value.parse::<bool>().map_err(|_| {
                anyhow!("verify.require_verify_gate must be 'true' or 'false', got '{value}'")
            })?;
            s.verify.require_verify_gate = Some(b);
        }
        "base_branch" => s.base_branch = Some(value.to_owned()),
        "queue.assignee" => s.queue.assignee = Some(value.to_owned()),
        "branch_mode" => {
            // Validate through the shared parser; store the canonical lowercase
            // string so resolution and `config get` see one form.
            parse_branch_mode(value)?;
            s.branch_mode = Some(value.to_owned());
        }
        "remote_control" => {
            let b = value
                .parse::<bool>()
                .map_err(|_| anyhow!("remote_control must be 'true' or 'false', got '{value}'"))?;
            s.remote_control = Some(b);
        }
        "claude.plan_model" => with_claude(&mut s, |c| c.plan_model = Some(value.to_owned()))?,
        "claude.plan_effort" | "claude.exec_effort" => {
            value.parse::<Effort>().map_err(anyhow::Error::msg)?;
            let plan = key == "claude.plan_effort";
            with_claude(&mut s, |c| {
                let slot = if plan {
                    &mut c.plan_effort
                } else {
                    &mut c.exec_effort
                };
                *slot = Some(value.to_owned());
            })?
        }
        "claude.default_exec_model" => {
            with_claude(&mut s, |c| c.default_exec_model = Some(value.to_owned()))?
        }
        "claude.max_minutes_per_issue" => {
            let n = value.parse::<u64>().map_err(|_| {
                anyhow!("claude.max_minutes_per_issue must be a non-negative integer (0 disables the per-issue cap), got '{value}'")
            })?;
            with_claude(&mut s, |c| c.max_minutes_per_issue = Some(n))?;
        }
        "claude.console_name" => {
            let b = value
                .parse::<bool>()
                .map_err(|_| anyhow!("{key} must be 'true' or 'false', got '{value}'"))?;
            with_claude(&mut s, |c| c.console_name = b)?
        }
        "copilot.plan_model" => with_copilot(&mut s, |c| c.plan_model = Some(value.to_owned()))?,
        "copilot.exec_model" => with_copilot(&mut s, |c| c.exec_model = Some(value.to_owned()))?,
        // Validated here, not at run time: an unrankable level is silently
        // dropped by the adapter's clamp, so an unvalidated typo would persist as
        // a setting that `config get` reports as set and that does nothing.
        "copilot.plan_effort" | "copilot.exec_effort" => {
            if !ralphy_agent_copilot::is_known_effort(value) {
                bail!("{key} must be a Copilot reasoning-effort level, got '{value}'");
            }
            let plan = key == "copilot.plan_effort";
            with_copilot(&mut s, |c| {
                let slot = if plan {
                    &mut c.plan_effort
                } else {
                    &mut c.exec_effort
                };
                *slot = Some(value.to_owned());
            })?
        }
        // The verbosity IS the safety feature (ADR-0041 D7): the hatch gives
        // Copilot back a credentialled MCP server that can open a PR on its own.
        "copilot.allow_builtin_mcp_servers_i_understand_the_risk" => {
            let b = value
                .parse::<bool>()
                .map_err(|_| anyhow!("{key} must be 'true' or 'false', got '{value}'"))?;
            with_copilot(&mut s, |c| {
                c.allow_builtin_mcp_servers_i_understand_the_risk = b
            })?
        }
        // Validated HERE and only here: a persisted id is refused at
        // configuration time rather than mid-run, while `--plan-model`/
        // `--exec-model` stay unfiltered so a stale local list cannot block an id
        // the vendor has started serving (ADR-0043 D8 amendment, #257).
        //
        // The stored value is TRIMMED: `is_pinnable_model` trims before matching,
        // so persisting the raw string would accept ` gemini-2.5-pro `, report it
        // valid from `config get`, and 404 on every run.
        "gemini.plan_model" | "gemini.exec_model" => {
            let value = value.trim();
            if !ralphy_agent_gemini::is_pinnable_model(value) {
                bail!(
                    "{key} must be a Gemini model id the CLI still serves, got '{value}'; valid: {}",
                    ralphy_agent_gemini::PINNABLE_MODELS.join(", ")
                );
            }
            let plan = key == "gemini.plan_model";
            with_gemini(&mut s, |g| {
                if plan {
                    g.plan_model = Some(value.to_owned());
                } else {
                    g.exec_model = Some(value.to_owned());
                }
            })?
        }
        // Same reasoning, a different capability (ADR-0042 D6): the hatch lets a
        // run upload the operator's repository to the vendor. Ralphy never denies
        // the capability — it denies a SILENT one.
        "cursor.allow_codebase_indexing_i_understand_the_risk" => {
            let b = value
                .parse::<bool>()
                .map_err(|_| anyhow!("{key} must be 'true' or 'false', got '{value}'"))?;
            with_cursor(&mut s, |c| {
                c.allow_codebase_indexing_i_understand_the_risk = b
            })?
        }
        _ => unreachable!(),
    }
    s.save(ws)?;
    gitignore::ensure_ralphy_ignored(ws.repo_root())?;
    println!("{key} = {value}");
    Ok(())
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
        "queue.assignee" => s.queue.assignee = None,
        "branch_mode" => s.branch_mode = None,
        "remote_control" => s.remote_control = None,
        "claude.plan_model" => with_claude(&mut s, |c| c.plan_model = None)?,
        "claude.plan_effort" => with_claude(&mut s, |c| c.plan_effort = None)?,
        "claude.default_exec_model" => with_claude(&mut s, |c| c.default_exec_model = None)?,
        "claude.exec_effort" => with_claude(&mut s, |c| c.exec_effort = None)?,
        "claude.max_minutes_per_issue" => with_claude(&mut s, |c| c.max_minutes_per_issue = None)?,
        "claude.console_name" => with_claude(&mut s, |c| c.console_name = false)?,
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

pub fn get(ws: &Workspace, json: bool) -> Result<()> {
    if json {
        println!("{}", serde_json::to_string(&config_json(ws)?)?);
        return Ok(());
    }
    let s = Settings::load(ws)?;
    let opencode: OpenCodeSettings = s.agent_settings(OpenCodeSettings::SECTION)?;
    let claude: ClaudeSettings = s.agent_settings(ClaudeSettings::SECTION)?;
    let copilot: CopilotSettings = s.agent_settings(CopilotSettings::SECTION)?;
    let cursor: CursorSettings = s.agent_settings(CursorSettings::SECTION)?;
    let gemini: GeminiSettings = s.agent_settings(GeminiSettings::SECTION)?;
    print_str("opencode.model", opencode.model);
    print_str("verify.command", s.verify.command);
    match s.verify.require_verify_gate {
        Some(b) => println!("verify.require_verify_gate = {b}"),
        None => println!("verify.require_verify_gate: not set"),
    }
    print_str("base_branch", s.base_branch);
    print_str("branch_mode", s.branch_mode);
    match s.remote_control {
        Some(b) => println!("remote_control = {b}"),
        None => println!("remote_control: not set"),
    }
    print_str("queue.assignee", s.queue.assignee);
    print_str("claude.plan_model", claude.plan_model);
    print_str("claude.plan_effort", claude.plan_effort);
    print_str("claude.default_exec_model", claude.default_exec_model);
    print_str("claude.exec_effort", claude.exec_effort);
    match claude.max_minutes_per_issue {
        Some(n) => println!("claude.max_minutes_per_issue = {n}"),
        None => println!("claude.max_minutes_per_issue: not set"),
    }
    println!("claude.console_name = {}", claude.console_name);
    print_str("copilot.plan_model", copilot.plan_model);
    print_str("copilot.exec_model", copilot.exec_model);
    print_str("copilot.plan_effort", copilot.plan_effort);
    print_str("copilot.exec_effort", copilot.exec_effort);
    println!(
        "copilot.allow_builtin_mcp_servers_i_understand_the_risk = {}",
        copilot.allow_builtin_mcp_servers_i_understand_the_risk
    );
    println!(
        "cursor.allow_codebase_indexing_i_understand_the_risk = {}",
        cursor.allow_codebase_indexing_i_understand_the_risk
    );
    print_str("gemini.plan_model", gemini.plan_model);
    print_str("gemini.exec_model", gemini.exec_model);
    // The CloudEvents sink knobs come from the global per-repo store, printed for
    // the current repo's slug (the token masked).
    let slug = git::project_slug(ws.repo_root());
    let events = crate::events::config::EventsStore::load().unwrap_or_default();
    let entry = events.entry(&slug);
    print_str("events.url", entry.and_then(|e| e.url.clone()));
    match entry.and_then(|e| e.token.as_deref()) {
        Some(t) => println!(
            "events.token = {}",
            crate::telegram::config::masked_token(t)
        ),
        None => println!("events.token: not set"),
    }
    Ok(())
}

/// Build the resolved-config JSON object the daemon's config Query verb reads:
/// every supported key mapped to its resolved value or JSON `null`. `events.token`
/// is MASKED (never echoed in full), mirroring the human `get`. Keys match
/// [`SUPPORTED_KEYS`]; the daemon parses this verbatim.
fn config_json(ws: &Workspace) -> Result<serde_json::Value> {
    let s = Settings::load(ws)?;
    let opencode: OpenCodeSettings = s.agent_settings(OpenCodeSettings::SECTION)?;
    let claude: ClaudeSettings = s.agent_settings(ClaudeSettings::SECTION)?;
    let copilot: CopilotSettings = s.agent_settings(CopilotSettings::SECTION)?;
    let cursor: CursorSettings = s.agent_settings(CursorSettings::SECTION)?;
    let gemini: GeminiSettings = s.agent_settings(GeminiSettings::SECTION)?;
    let slug = git::project_slug(ws.repo_root());
    let events = crate::events::config::EventsStore::load().unwrap_or_default();
    let entry = events.entry(&slug);
    let masked_token = entry
        .and_then(|e| e.token.as_deref())
        .map(crate::telegram::config::masked_token);
    Ok(serde_json::json!({
        "opencode.model": opencode.model,
        "base_branch": s.base_branch,
        "branch_mode": s.branch_mode,
        "remote_control": s.remote_control,
        "queue.assignee": s.queue.assignee,
        "verify.command": s.verify.command,
        "verify.require_verify_gate": s.verify.require_verify_gate,
        "events.url": entry.and_then(|e| e.url.clone()),
        "events.token": masked_token,
        "claude.plan_model": claude.plan_model,
        "claude.plan_effort": claude.plan_effort,
        "claude.default_exec_model": claude.default_exec_model,
        "claude.exec_effort": claude.exec_effort,
        "claude.max_minutes_per_issue": claude.max_minutes_per_issue,
        "claude.console_name": claude.console_name,
        "copilot.plan_model": copilot.plan_model,
        "copilot.exec_model": copilot.exec_model,
        "copilot.plan_effort": copilot.plan_effort,
        "copilot.exec_effort": copilot.exec_effort,
        "copilot.allow_builtin_mcp_servers_i_understand_the_risk":
            copilot.allow_builtin_mcp_servers_i_understand_the_risk,
        "cursor.allow_codebase_indexing_i_understand_the_risk":
            cursor.allow_codebase_indexing_i_understand_the_risk,
        "gemini.plan_model": gemini.plan_model,
        "gemini.exec_model": gemini.exec_model,
    }))
}

/// Print one `key = value` / `key: not set` line for an optional string knob,
/// treating an empty string as unset.
fn print_str(key: &str, value: Option<String>) {
    match value.filter(|v| !v.is_empty()) {
        Some(v) => println!("{key} = {v}"),
        None => println!("{key}: not set"),
    }
}

/// Resolve a vendor-neutral optional model knob (ADR-0010). Precedence: `flag`,
/// then the persisted `settings.json` value, then `None` (the adapter resolves
/// its own default). Empty strings on either source are treated as unset.
pub fn resolve_optional_model(flag: Option<String>, persisted: Option<String>) -> Option<String> {
    flag.filter(|s| !s.is_empty())
        .or_else(|| persisted.filter(|s| !s.is_empty()))
}

/// Resolve the OpenCode execution model from the per-run flag and the
/// persisted setting (ADR-0010). Precedence: `exec_model` flag > persisted
/// `opencode.model` > `None` (OpenCode resolves its own default). Empty
/// strings are treated as unset.
pub fn resolve_opencode_model(
    exec_model: Option<String>,
    persisted: Option<String>,
) -> Option<String> {
    resolve_optional_model(exec_model, persisted)
}

/// Resolve a string-valued run knob (ADR-0010). Precedence: per-run `flag` >
/// persisted `settings.json` value > hardcoded `default`. Empty strings on
/// either source are treated as unset so they fall through to the next slot.
pub fn resolve_str(flag: Option<String>, persisted: Option<String>, default: &str) -> String {
    flag.filter(|s| !s.is_empty())
        .or_else(|| persisted.filter(|s| !s.is_empty()))
        .unwrap_or_else(|| default.to_owned())
}

/// Resolve one phase's neutral effort. Persisted strings are parsed even when
/// supplied outside `config set`, so hand-edited settings fail before adapter construction.
pub fn resolve_effort(
    flag: Option<Effort>,
    persisted: Option<String>,
    default: Option<Effort>,
) -> Result<Option<Effort>> {
    if let Some(flag) = flag {
        return Ok(Some(flag));
    }
    match persisted.filter(|value| !value.is_empty()) {
        Some(value) => value
            .parse::<Effort>()
            .map(Some)
            .map_err(anyhow::Error::msg),
        None => Ok(default),
    }
}

/// Resolve the effective queue assignee filter. Precedence:
/// `--assignee X` (non-empty) > `--no-assignee` (forces `None`) >
/// persisted `queue.assignee` (non-empty) > `None` (no filter). Empty strings on
/// either source are treated as unset, matching [`resolve_str`].
pub fn resolve_assignee(
    flag: Option<&str>,
    no_assignee: bool,
    persisted: Option<&str>,
) -> Option<String> {
    if let Some(a) = flag.filter(|s| !s.is_empty()) {
        return Some(a.to_owned());
    }
    if no_assignee {
        return None;
    }
    persisted.filter(|s| !s.is_empty()).map(str::to_owned)
}

/// Resolve a `u64`-valued run knob (ADR-0010). Precedence: per-run `flag` >
/// persisted `settings.json` value > hardcoded `default`.
pub fn resolve_u64(flag: Option<u64>, persisted: Option<u64>, default: u64) -> u64 {
    flag.or(persisted).unwrap_or(default)
}

/// Resolve the effective Remote Control switch (#148). Precedence:
/// `--remote-control` > `--no-remote-control` > persisted `remote_control` >
/// `false` (OFF by default).
pub fn resolve_remote_control(
    remote_control: bool,
    no_remote_control: bool,
    persisted: Option<bool>,
) -> bool {
    if remote_control {
        true
    } else if no_remote_control {
        false
    } else {
        persisted.unwrap_or(false)
    }
}

/// Parse a persisted/`config set` `branch_mode` string into the core enum.
/// Accepts the lowercase canonical forms `"new"` / `"current"`; any other value
/// is a hard error so an invalid setting fails loud rather than silently
/// resolving to a default. The single validation path shared by `config set`
/// and run-time resolution keeps `ralphy-core`'s [`BranchMode`] serde-free.
pub fn parse_branch_mode(value: &str) -> Result<BranchMode> {
    match value {
        "new" => Ok(BranchMode::New),
        "current" => Ok(BranchMode::Current),
        other => bail!("branch_mode must be 'new' or 'current', got '{other}'"),
    }
}

#[cfg(test)]
mod tests;
