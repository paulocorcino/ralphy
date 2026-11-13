//! `ralphy config set` and `config unset`: write one key to
//! `.ralphy/settings.json`, or to the global events store for `events.*`.

use anyhow::{anyhow, bail, Context, Result};
use ralphy_agent_antigravity::AntigravitySettings;
use ralphy_agent_claude::ClaudeSettings;
use ralphy_agent_copilot::CopilotSettings;
use ralphy_agent_cursor::CursorSettings;
use ralphy_agent_opencode::OpenCodeSettings;
use ralphy_core::{git, gitignore, Effort, Settings, Workspace};

use super::{parse_branch_mode, require_known_key};

/// Load-mutate-store the Claude section of the settings. The section is opaque
/// JSON to the core; a malformed section is a hard error here (unlike run-time
/// resolution, which warns and defaults) so `config set` fails loud.
fn with_claude(s: &mut Settings, f: impl FnOnce(&mut ClaudeSettings)) -> Result<()> {
    let mut c: ClaudeSettings = s.agent_settings(ClaudeSettings::SECTION)?;
    f(&mut c);
    s.set_agent_settings(ClaudeSettings::SECTION, &c)
}

/// Load-mutate-store the `OpenCode` section; same contract as [`with_claude`].
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

/// Load-mutate-store the Antigravity section; same contract as [`with_claude`].
fn with_antigravity(s: &mut Settings, f: impl FnOnce(&mut AntigravitySettings)) -> Result<()> {
    let mut a: AntigravitySettings = s.agent_settings(AntigravitySettings::SECTION)?;
    f(&mut a);
    s.set_agent_settings(AntigravitySettings::SECTION, &a)
}

/// Load-mutate-store the Cursor section; same contract as [`with_claude`].
fn with_cursor(s: &mut Settings, f: impl FnOnce(&mut CursorSettings)) -> Result<()> {
    let mut c: CursorSettings = s.agent_settings(CursorSettings::SECTION)?;
    f(&mut c);
    s.set_agent_settings(CursorSettings::SECTION, &c)
}

pub(crate) fn set(ws: &Workspace, key: &str, value: &str) -> Result<()> {
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
    outln!("{key} = {value}");
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
                outln!(
                    "warning: events.url is plain http — the events bearer will travel \
                     in cleartext to {host}"
                );
            }
            store.set_url(&slug, value);
        }
        "events.token" => store.set_token(&slug, value),
        _ => unreachable!(),
    }
    store.save()?;
    // Never echo the secret token back to stdout — mask it like `config get`.
    if key == "events.token" {
        outln!("{key} = {}", crate::telegram::config::masked_token(value));
    } else {
        outln!("{key} = {value}");
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
                .ok_or_else(|| {
                    anyhow!(
                        "files.upload_max_mb must be a whole number of MB, at least 1, \
                         got '{value}'"
                    )
                })?;
            s.files.upload_max_mb = Some(n);
        }
        "runs.keep" => {
            let n = value.parse::<u64>().with_context(|| {
                format!(
                    "runs.keep must be a non-negative integer (0 keeps every run folder), \
                     got '{value}'"
                )
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
/// `cursor.*`).
fn set_agent_key(s: &mut Settings, key: &str, value: &str) -> Result<()> {
    match key {
        "opencode.model" => with_opencode(s, |o| o.model = Some(value.to_owned()))?,
        "claude.plan_model" => with_claude(s, |c| c.plan_model = Some(value.to_owned()))?,
        "claude.plan_effort" | "claude.exec_effort" => set_claude_effort(s, key, value)?,
        "claude.default_exec_model" => {
            with_claude(s, |c| c.default_exec_model = Some(value.to_owned()))?;
        }
        "claude.max_minutes_per_issue" => {
            let n = value.parse::<u64>().with_context(|| {
                format!(
                    "claude.max_minutes_per_issue must be a non-negative integer \
                     (0 disables the per-issue cap), got '{value}'"
                )
            })?;
            with_claude(s, |c| c.max_minutes_per_issue = Some(n))?;
        }
        "claude.console_name" => {
            let b = value
                .parse::<bool>()
                .with_context(|| format!("{key} must be 'true' or 'false', got '{value}'"))?;
            with_claude(s, |c| c.console_name = b)?;
        }
        "claude.console_classic_screen" => {
            let b = value
                .parse::<bool>()
                .with_context(|| format!("{key} must be 'true' or 'false', got '{value}'"))?;
            with_claude(s, |c| c.console_classic_screen = b)?;
        }
        // The escape of the hook proof (ADR-0072 D6): local-only, the daemon
        // refuses to set it.
        "claude.run_without_guard_proof_i_understand_the_risk" => {
            let b = value
                .parse::<bool>()
                .with_context(|| format!("{key} must be 'true' or 'false', got '{value}'"))?;
            with_claude(s, |c| c.run_without_guard_proof_i_understand_the_risk = b)?;
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
            with_copilot(s, |c| c.allow_builtin_mcp_servers_i_understand_the_risk = b)?;
        }
        "antigravity.plan_model" | "antigravity.exec_model" => {
            set_antigravity_model(s, key, value)?;
        }
        "antigravity.plan_effort" | "antigravity.exec_effort" => {
            set_antigravity_effort(s, key, value)?;
        }
        // The escape of the hook proof: local-only, the daemon refuses to set it.
        "antigravity.run_without_guard_proof_i_understand_the_risk" => {
            let b = value
                .parse::<bool>()
                .with_context(|| format!("{key} must be 'true' or 'false', got '{value}'"))?;
            with_antigravity(s, |a| a.run_without_guard_proof_i_understand_the_risk = b)?;
        }
        // Same reasoning, a different capability (ADR-0042 D6): the hatch lets a
        // run upload the operator's repository to the vendor. Ralphy never denies
        // the capability — it denies a SILENT one.
        "cursor.allow_codebase_indexing_i_understand_the_risk" => {
            let b = value
                .parse::<bool>()
                .with_context(|| format!("{key} must be 'true' or 'false', got '{value}'"))?;
            with_cursor(s, |c| c.allow_codebase_indexing_i_understand_the_risk = b)?;
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

/// Set `antigravity.plan_effort` or `antigravity.exec_effort` to a core effort level.
fn set_antigravity_effort(s: &mut Settings, key: &str, value: &str) -> Result<()> {
    value.parse::<Effort>().map_err(anyhow::Error::msg)?;
    let plan = key == "antigravity.plan_effort";
    with_antigravity(s, |a| {
        let slot = if plan {
            &mut a.plan_effort
        } else {
            &mut a.exec_effort
        };
        *slot = Some(value.to_owned());
    })
}

/// One word of `[a-z0-9.-]` that starts with `[a-z0-9]`, so the id is never
/// read as a flag of `agy`.
fn is_agy_model_id(value: &str) -> bool {
    let id_char = |c: char| c.is_ascii_lowercase() || c.is_ascii_digit();
    value.starts_with(id_char) && value.chars().all(|c| id_char(c) || c == '.' || c == '-')
}

/// Set `antigravity.plan_model` or `antigravity.exec_model`. The id list changes
/// with each agy update, so only the shape is checked by [`is_agy_model_id`].
/// The CLI refuses an unknown id itself.
fn set_antigravity_model(s: &mut Settings, key: &str, value: &str) -> Result<()> {
    let value = value.trim();
    if !is_agy_model_id(value) {
        bail!(
            "{key} must be one model id of lowercase letters, digits, '.' and '-', \
             starting with a letter or digit, got '{value}'"
        );
    }
    let plan = key == "antigravity.plan_model";
    with_antigravity(s, |a| {
        let slot = if plan {
            &mut a.plan_model
        } else {
            &mut a.exec_model
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

/// Remove one key of a `gemini` section that an older version stored; the section goes
/// with its last key.
fn is_gemini_field(field: &str) -> bool {
    matches!(field, "plan_model" | "exec_model")
}

fn unset_stored_gemini_key(s: &mut Settings, field: &str) {
    if let Some(section) = s.extra.get_mut("gemini").and_then(|v| v.as_object_mut()) {
        section.remove(field);
        if section.is_empty() {
            s.extra.remove("gemini");
        }
    }
}

pub(crate) fn unset(ws: &Workspace, key: &str) -> Result<()> {
    if let Some(field) = key.strip_prefix("gemini.").filter(|f| is_gemini_field(f)) {
        let mut s = Settings::load(ws)?;
        unset_stored_gemini_key(&mut s, field);
        s.save(ws)?;
        outln!("{key}: unset");
        return Ok(());
    }
    require_known_key(key)?;
    // The events sink knobs are cleared in the global store, not settings.json.
    if key == "events.url" || key == "events.token" {
        let slug = git::project_slug(ws.repo_root());
        let mut store = crate::events::config::EventsStore::load()?;
        // The field name is the key's suffix (`url` / `token`).
        store.clear(&slug, key.split_at("events.".len()).1);
        store.save()?;
        outln!("{key}: unset");
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
            with_claude(&mut s, |c| c.console_classic_screen = false)?;
        }
        "claude.run_without_guard_proof_i_understand_the_risk" => with_claude(&mut s, |c| {
            c.run_without_guard_proof_i_understand_the_risk = false;
        })?,
        "copilot.plan_model" => with_copilot(&mut s, |c| c.plan_model = None)?,
        "copilot.exec_model" => with_copilot(&mut s, |c| c.exec_model = None)?,
        "copilot.plan_effort" => with_copilot(&mut s, |c| c.plan_effort = None)?,
        "copilot.exec_effort" => with_copilot(&mut s, |c| c.exec_effort = None)?,
        "copilot.allow_builtin_mcp_servers_i_understand_the_risk" => with_copilot(&mut s, |c| {
            c.allow_builtin_mcp_servers_i_understand_the_risk = false;
        })?,
        "cursor.allow_codebase_indexing_i_understand_the_risk" => with_cursor(&mut s, |c| {
            c.allow_codebase_indexing_i_understand_the_risk = false;
        })?,
        "antigravity.plan_model" => with_antigravity(&mut s, |a| a.plan_model = None)?,
        "antigravity.exec_model" => with_antigravity(&mut s, |a| a.exec_model = None)?,
        "antigravity.plan_effort" => with_antigravity(&mut s, |a| a.plan_effort = None)?,
        "antigravity.exec_effort" => with_antigravity(&mut s, |a| a.exec_effort = None)?,
        "antigravity.run_without_guard_proof_i_understand_the_risk" => {
            with_antigravity(&mut s, |a| {
                a.run_without_guard_proof_i_understand_the_risk = false;
            })?;
        }
        _ => unreachable!(),
    }
    s.save(ws)?;
    outln!("{key}: unset");
    Ok(())
}

/// The host of an `http://` URL that is not loopback — the one shape of
/// `events.url` under which the bearer travels in cleartext. `None` for
/// `https://`, for a loopback host (`localhost`, `127.*`, `[::1]`), or for
/// anything that is not a URL (the sink's own validation owns that). Pure
/// string work: no `url` crate for one scheme check.
pub(super) fn cleartext_remote_host(url: &str) -> Option<&str> {
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
