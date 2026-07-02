//! The `ralphy config` subcommand (ADR-0010).
//!
//! Manages per-repo `.ralphy/settings.json`. Supported keys: `opencode.model`
//! (OpenCode execution-model default, #47), the agent-agnostic `base_branch` and
//! `branch_mode`, and the Claude-only run defaults under `claude.*`
//! (`plan_model`, `plan_effort`, `default_exec_model`, `exec_effort`,
//! `max_minutes_per_issue`). The model/effort/budget knobs are Claude-only
//! today — a Codex equivalent is deferred. Each resolves with the same
//! precedence: per-run flag > `settings.json` > hardcoded default.

use std::path::PathBuf;

use anyhow::{anyhow, bail, Result};
use clap::{Args, Subcommand};
use ralphy_agent_claude::ClaudeSettings;
use ralphy_agent_opencode::OpenCodeSettings;
use ralphy_core::{git, gitignore, BranchMode, Settings, Workspace};

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
    Get,
}

/// Dispatch a `config` subcommand.
pub fn run(args: ConfigArgs) -> Result<()> {
    let repo_root = git::resolve_toplevel(&args.repo)?;
    let ws = Workspace::new(&repo_root);
    match args.command {
        ConfigCommand::Set { key, value } => set(&ws, &key, &value),
        ConfigCommand::Unset { key } => unset(&ws, &key),
        ConfigCommand::Get => get(&ws),
    }
}

/// Human-readable list of every supported `config` key, reused both in
/// `--help`-style docs and in the unknown-key error so the two never drift. The
/// model/effort/budget knobs are Claude-only in the current wiring (ADR-0010).
pub const SUPPORTED_KEYS_HELP: &str = "supported keys: \
opencode.model, base_branch, branch_mode, verify.command, \
verify.require_verify_gate, \
claude.plan_model, claude.plan_effort, claude.default_exec_model, \
claude.exec_effort, claude.max_minutes_per_issue \
(verify.command is the per-repo fallback verify gate, ADR-0011; \
verify.require_verify_gate=true parks a gateless issue for a human \
instead of closing it, ADR-0015; \
model/effort/budget defaults are Claude-only today \
(Codex deferred; OpenCode's model lives under opencode.model, #47))";

fn require_known_key(key: &str) -> Result<()> {
    match key {
        "opencode.model"
        | "base_branch"
        | "branch_mode"
        | "verify.command"
        | "verify.require_verify_gate"
        | "claude.plan_model"
        | "claude.plan_effort"
        | "claude.default_exec_model"
        | "claude.exec_effort"
        | "claude.max_minutes_per_issue" => Ok(()),
        other => bail!("unknown config key '{other}'; {SUPPORTED_KEYS_HELP}"),
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

pub fn set(ws: &Workspace, key: &str, value: &str) -> Result<()> {
    require_known_key(key)?;
    if value.trim().is_empty() {
        bail!("value for '{key}' must not be empty — use `config unset {key}` to clear it");
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
        "branch_mode" => {
            // Validate through the shared parser; store the canonical lowercase
            // string so resolution and `config get` see one form.
            parse_branch_mode(value)?;
            s.branch_mode = Some(value.to_owned());
        }
        "claude.plan_model" => with_claude(&mut s, |c| c.plan_model = Some(value.to_owned()))?,
        "claude.plan_effort" => with_claude(&mut s, |c| c.plan_effort = Some(value.to_owned()))?,
        "claude.default_exec_model" => {
            with_claude(&mut s, |c| c.default_exec_model = Some(value.to_owned()))?
        }
        "claude.exec_effort" => with_claude(&mut s, |c| c.exec_effort = Some(value.to_owned()))?,
        "claude.max_minutes_per_issue" => {
            let n = value.parse::<u64>().map_err(|_| {
                anyhow!("claude.max_minutes_per_issue must be a non-negative integer (0 disables the per-issue cap), got '{value}'")
            })?;
            with_claude(&mut s, |c| c.max_minutes_per_issue = Some(n))?;
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
    let mut s = Settings::load(ws)?;
    match key {
        "opencode.model" => with_opencode(&mut s, |o| o.model = None)?,
        "verify.command" => s.verify.command = None,
        "verify.require_verify_gate" => s.verify.require_verify_gate = None,
        "base_branch" => s.base_branch = None,
        "branch_mode" => s.branch_mode = None,
        "claude.plan_model" => with_claude(&mut s, |c| c.plan_model = None)?,
        "claude.plan_effort" => with_claude(&mut s, |c| c.plan_effort = None)?,
        "claude.default_exec_model" => with_claude(&mut s, |c| c.default_exec_model = None)?,
        "claude.exec_effort" => with_claude(&mut s, |c| c.exec_effort = None)?,
        "claude.max_minutes_per_issue" => with_claude(&mut s, |c| c.max_minutes_per_issue = None)?,
        _ => unreachable!(),
    }
    s.save(ws)?;
    println!("{key}: unset");
    Ok(())
}

pub fn get(ws: &Workspace) -> Result<()> {
    let s = Settings::load(ws)?;
    let opencode: OpenCodeSettings = s.agent_settings(OpenCodeSettings::SECTION)?;
    let claude: ClaudeSettings = s.agent_settings(ClaudeSettings::SECTION)?;
    print_str("opencode.model", opencode.model);
    print_str("verify.command", s.verify.command);
    match s.verify.require_verify_gate {
        Some(b) => println!("verify.require_verify_gate = {b}"),
        None => println!("verify.require_verify_gate: not set"),
    }
    print_str("base_branch", s.base_branch);
    print_str("branch_mode", s.branch_mode);
    print_str("claude.plan_model", claude.plan_model);
    print_str("claude.plan_effort", claude.plan_effort);
    print_str("claude.default_exec_model", claude.default_exec_model);
    print_str("claude.exec_effort", claude.exec_effort);
    match claude.max_minutes_per_issue {
        Some(n) => println!("claude.max_minutes_per_issue = {n}"),
        None => println!("claude.max_minutes_per_issue: not set"),
    }
    Ok(())
}

/// Print one `key = value` / `key: not set` line for an optional string knob,
/// treating an empty string as unset.
fn print_str(key: &str, value: Option<String>) {
    match value.filter(|v| !v.is_empty()) {
        Some(v) => println!("{key} = {v}"),
        None => println!("{key}: not set"),
    }
}

/// Resolve the OpenCode execution model from the per-run flag and the
/// persisted setting (ADR-0010). Precedence: `exec_model` flag > persisted
/// `opencode.model` > `None` (OpenCode resolves its own default). Empty
/// strings are treated as unset.
pub fn resolve_opencode_model(
    exec_model: Option<String>,
    persisted: Option<String>,
) -> Option<String> {
    exec_model
        .filter(|s| !s.is_empty())
        .or_else(|| persisted.filter(|s| !s.is_empty()))
}

/// Resolve a string-valued run knob (ADR-0010). Precedence: per-run `flag` >
/// persisted `settings.json` value > hardcoded `default`. Empty strings on
/// either source are treated as unset so they fall through to the next slot.
pub fn resolve_str(flag: Option<String>, persisted: Option<String>, default: &str) -> String {
    flag.filter(|s| !s.is_empty())
        .or_else(|| persisted.filter(|s| !s.is_empty()))
        .unwrap_or_else(|| default.to_owned())
}

/// Resolve a `u64`-valued run knob (ADR-0010). Precedence: per-run `flag` >
/// persisted `settings.json` value > hardcoded `default`.
pub fn resolve_u64(flag: Option<u64>, persisted: Option<u64>, default: u64) -> u64 {
    flag.or(persisted).unwrap_or(default)
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
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicU32, Ordering};

    static N: AtomicU32 = AtomicU32::new(0);

    fn tmp_ws(name: &str) -> (Workspace, PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "ralphy-config-{}-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed),
            name
        ));
        fs::create_dir_all(&dir).unwrap();
        let ws = Workspace::new(&dir);
        (ws, dir)
    }

    // --- resolve_opencode_model precedence ---

    #[test]
    fn flag_wins_over_persisted() {
        assert_eq!(
            resolve_opencode_model(Some("flag".into()), Some("persisted".into())),
            Some("flag".into())
        );
    }

    #[test]
    fn persisted_used_when_flag_absent() {
        assert_eq!(
            resolve_opencode_model(None, Some("kimi-for-coding/k2p7".into())),
            Some("kimi-for-coding/k2p7".into())
        );
    }

    #[test]
    fn both_unset_returns_none() {
        assert_eq!(resolve_opencode_model(None, None), None);
    }

    #[test]
    fn empty_flag_falls_through_to_persisted() {
        assert_eq!(
            resolve_opencode_model(Some("".into()), Some("k2p7".into())),
            Some("k2p7".into())
        );
    }

    // --- resolve_str / resolve_u64 precedence ---

    #[test]
    fn resolve_str_flag_wins() {
        assert_eq!(
            resolve_str(Some("flag".into()), Some("persisted".into()), "default"),
            "flag"
        );
    }

    #[test]
    fn resolve_str_persisted_when_flag_absent_or_empty() {
        assert_eq!(
            resolve_str(None, Some("persisted".into()), "default"),
            "persisted"
        );
        assert_eq!(
            resolve_str(Some("".into()), Some("persisted".into()), "default"),
            "persisted"
        );
        // An empty persisted value also falls through to the default.
        assert_eq!(resolve_str(None, Some("".into()), "default"), "default");
    }

    #[test]
    fn resolve_str_byte_for_byte_default() {
        // Absent flag AND absent setting yield today's hardcoded value verbatim.
        assert_eq!(resolve_str(None, None, "origin/main"), "origin/main");
    }

    #[test]
    fn resolve_u64_flag_wins_then_persisted_then_default() {
        assert_eq!(resolve_u64(Some(10), Some(20), 90), 10);
        assert_eq!(resolve_u64(None, Some(20), 90), 20);
        assert_eq!(resolve_u64(None, None, 90), 90);
    }

    // --- parse_branch_mode ---

    #[test]
    fn parse_branch_mode_ok_arms() {
        assert_eq!(parse_branch_mode("new").unwrap(), BranchMode::New);
        assert_eq!(parse_branch_mode("current").unwrap(), BranchMode::Current);
    }

    #[test]
    fn parse_branch_mode_rejects_unknown() {
        let err = parse_branch_mode("sideways").unwrap_err();
        assert!(
            err.to_string().contains("must be 'new' or 'current'"),
            "got: {err}"
        );
    }

    // --- config handler round-trip ---

    #[test]
    fn handler_round_trip() {
        let (ws, dir) = tmp_ws("handler-round-trip");

        // set stores the value.
        set(&ws, "opencode.model", "kimi-for-coding/k2p7").unwrap();
        let s = Settings::load(&ws).unwrap();
        let o: OpenCodeSettings = s.agent_settings(OpenCodeSettings::SECTION).unwrap();
        assert_eq!(o.model.as_deref(), Some("kimi-for-coding/k2p7"));

        // unset clears it.
        unset(&ws, "opencode.model").unwrap();
        let s = Settings::load(&ws).unwrap();
        let o: OpenCodeSettings = s.agent_settings(OpenCodeSettings::SECTION).unwrap();
        assert_eq!(o.model, None);

        // Unknown key errors.
        let err = set(&ws, "bad.key", "x").unwrap_err();
        assert!(err.to_string().contains("unknown config key"));

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn new_keys_handler_round_trip() {
        let (ws, dir) = tmp_ws("new-keys-handler");

        set(&ws, "base_branch", "origin/dev").unwrap();
        set(&ws, "claude.max_minutes_per_issue", "45").unwrap();
        set(&ws, "branch_mode", "current").unwrap();

        let s = Settings::load(&ws).unwrap();
        let c: ClaudeSettings = s.agent_settings(ClaudeSettings::SECTION).unwrap();
        assert_eq!(s.base_branch.as_deref(), Some("origin/dev"));
        assert_eq!(c.max_minutes_per_issue, Some(45));
        assert_eq!(s.branch_mode.as_deref(), Some("current"));

        unset(&ws, "base_branch").unwrap();
        unset(&ws, "claude.max_minutes_per_issue").unwrap();
        unset(&ws, "branch_mode").unwrap();
        let s = Settings::load(&ws).unwrap();
        let c: ClaudeSettings = s.agent_settings(ClaudeSettings::SECTION).unwrap();
        assert_eq!(s.base_branch, None);
        assert_eq!(c.max_minutes_per_issue, None);
        assert_eq!(s.branch_mode, None);

        // Invalid branch_mode and non-integer budget are rejected at set time.
        let err = set(&ws, "branch_mode", "sideways").unwrap_err();
        assert!(
            err.to_string().contains("must be 'new' or 'current'"),
            "got: {err}"
        );
        let err = set(&ws, "claude.max_minutes_per_issue", "abc").unwrap_err();
        assert!(
            err.to_string().contains("must be a non-negative integer"),
            "got: {err}"
        );
        // `0` is a valid value — it disables the per-issue cap.
        set(&ws, "claude.max_minutes_per_issue", "0").unwrap();
        let c: ClaudeSettings = Settings::load(&ws)
            .unwrap()
            .agent_settings(ClaudeSettings::SECTION)
            .unwrap();
        assert_eq!(c.max_minutes_per_issue, Some(0));

        fs::remove_dir_all(&dir).ok();
    }

    /// A `.ralphy/settings.json` written by a pre-#79 binary (typed vendor
    /// fields in core) must still parse, resolve with the ADR-0010 precedence
    /// (flag > settings > default), and survive a typed save without losing
    /// the vendor sections or unknown peer keys.
    #[test]
    fn previous_version_settings_file_still_parses_with_precedence() {
        let (ws, dir) = tmp_ws("back-compat");

        // Byte-for-byte shape a pre-#79 `save` produced (typed fields first,
        // including sections a config touch left present).
        let raw = r#"{
  "opencode": { "model": "kimi-for-coding/k2p7" },
  "base_branch": "origin/dev",
  "branch_mode": "current",
  "claude": { "plan_model": "opus", "max_minutes_per_issue": 45 },
  "verify": { "command": "cargo test" },
  "future_key": 123
}"#;
        fs::create_dir_all(ws.ralphy_dir()).unwrap();
        fs::write(ws.settings_path(), raw).unwrap();

        let s = Settings::load(&ws).unwrap();
        let c: ClaudeSettings = s.agent_settings(ClaudeSettings::SECTION).unwrap();
        let o: OpenCodeSettings = s.agent_settings(OpenCodeSettings::SECTION).unwrap();
        assert_eq!(o.model.as_deref(), Some("kimi-for-coding/k2p7"));
        assert_eq!(c.plan_model.as_deref(), Some("opus"));
        assert_eq!(c.max_minutes_per_issue, Some(45));
        assert_eq!(s.base_branch.as_deref(), Some("origin/dev"));
        assert_eq!(s.branch_mode.as_deref(), Some("current"));
        assert_eq!(s.verify.command.as_deref(), Some("cargo test"));

        // ADR-0010 precedence: flag > settings > default.
        assert_eq!(
            resolve_str(Some("flag".into()), c.plan_model.clone(), "default"),
            "flag"
        );
        assert_eq!(resolve_str(None, c.plan_model.clone(), "default"), "opus");
        assert_eq!(resolve_u64(None, c.max_minutes_per_issue, 90), 45);
        // A field the file never set falls through to the hardcoded default.
        assert_eq!(
            resolve_str(None, c.default_exec_model.clone(), "sonnet"),
            "sonnet"
        );

        // A typed save keeps the vendor sections and the unknown peer key.
        s.save(&ws).unwrap();
        let back = fs::read_to_string(ws.settings_path()).unwrap();
        for needle in ["opencode", "kimi-for-coding/k2p7", "claude", "plan_model", "future_key"] {
            assert!(back.contains(needle), "missing '{needle}' after save:\n{back}");
        }

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn help_notes_claude_only() {
        assert!(SUPPORTED_KEYS_HELP.contains("Claude-only today"));
    }

    #[test]
    fn verify_command_round_trip() {
        let (ws, dir) = tmp_ws("verify-command");

        set(&ws, "verify.command", "cargo test").unwrap();
        let s = Settings::load(&ws).unwrap();
        assert_eq!(s.verify.command.as_deref(), Some("cargo test"));

        unset(&ws, "verify.command").unwrap();
        let s = Settings::load(&ws).unwrap();
        assert_eq!(s.verify.command, None);

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn help_lists_verify_command() {
        assert!(SUPPORTED_KEYS_HELP.contains("verify.command"));
    }
}
