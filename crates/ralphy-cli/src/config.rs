//! The `ralphy config` subcommand (ADR-0010).
//!
//! Manages per-repo `.ralphy/settings.json`. Currently supports one key:
//! `opencode.model` — the persistent execution-model default for OpenCode.

use std::path::PathBuf;

use anyhow::{bail, Result};
use clap::{Args, Subcommand};
use ralphy_core::{git, gitignore, Settings, Workspace};

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
        /// The config key (currently only `opencode.model`).
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

fn require_known_key(key: &str) -> Result<()> {
    match key {
        "opencode.model" => Ok(()),
        other => bail!("unknown config key '{other}'; supported: opencode.model"),
    }
}

pub fn set(ws: &Workspace, key: &str, value: &str) -> Result<()> {
    require_known_key(key)?;
    if value.trim().is_empty() {
        bail!("value for '{key}' must not be empty — use `config unset {key}` to clear it");
    }
    let mut s = Settings::load(ws)?;
    match key {
        "opencode.model" => s.opencode.model = Some(value.to_owned()),
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
        "opencode.model" => s.opencode.model = None,
        _ => unreachable!(),
    }
    s.save(ws)?;
    println!("{key}: unset");
    Ok(())
}

pub fn get(ws: &Workspace) -> Result<()> {
    let s = Settings::load(ws)?;
    match s.opencode.model.filter(|m| !m.is_empty()) {
        Some(m) => println!("opencode.model = {m}"),
        None => println!("opencode.model: not set"),
    }
    Ok(())
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

    // --- config handler round-trip ---

    #[test]
    fn handler_round_trip() {
        let (ws, dir) = tmp_ws("handler-round-trip");

        // set stores the value.
        set(&ws, "opencode.model", "kimi-for-coding/k2p7").unwrap();
        let s = Settings::load(&ws).unwrap();
        assert_eq!(s.opencode.model.as_deref(), Some("kimi-for-coding/k2p7"));

        // unset clears it.
        unset(&ws, "opencode.model").unwrap();
        let s = Settings::load(&ws).unwrap();
        assert_eq!(s.opencode.model, None);

        // Unknown key errors.
        let err = set(&ws, "bad.key", "x").unwrap_err();
        assert!(err.to_string().contains("unknown config key"));

        fs::remove_dir_all(&dir).ok();
    }
}
