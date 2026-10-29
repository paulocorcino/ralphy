//! Reading the global Telegram config (ADR-0007 D2).
//!
//! A single `config.toml` resolved with the `directories` crate
//! (`~/.config/ralphy/config.toml`; `%APPDATA%\ralphy\` on Windows) holds the
//! bot token and the auto-detected `chat_id`. The environment variable
//! `RALPHY_TELEGRAM_TOKEN` overrides the stored token so a run can carry a token
//! without persisting it. This crate only reads the file; the CLI writes it
//! owner-only.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};

/// Environment variable that overrides the stored bot token.
pub const TOKEN_ENV: &str = "RALPHY_TELEGRAM_TOKEN";

/// The persisted Telegram configuration: the bot token and, once `setup` has
/// captured it, the chat the notifier posts to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TelegramConfig {
    /// The bot token issued by BotFather.
    pub token: String,
    /// The chat the bot posts to, auto-detected from an inbound `/start`. `None`
    /// until `setup` captures it.
    #[serde(default)]
    pub chat_id: Option<i64>,
}

impl TelegramConfig {
    /// The on-disk path of `config.toml`, resolved via `directories`.
    pub fn config_path() -> Result<PathBuf> {
        let dirs = ProjectDirs::from("", "", "ralphy")
            .context("could not resolve a config directory for ralphy")?;
        Ok(dirs.config_dir().join("config.toml"))
    }

    /// Load the config from disk, or `None` when no config file exists yet.
    pub fn load() -> Result<Option<TelegramConfig>> {
        Self::load_from(&Self::config_path()?)
    }

    /// Load the config at `path`, or `None` when no file is there. The daemon
    /// reads it through a path it was given, so a test's daemon reads a temp
    /// file and never the operator's.
    pub fn load_from(path: &Path) -> Result<Option<TelegramConfig>> {
        match std::fs::read_to_string(path) {
            Ok(text) => {
                let cfg =
                    toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
                Ok(Some(cfg))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
        }
    }
}

/// The token a run should use: `RALPHY_TELEGRAM_TOKEN` when set and non-empty,
/// otherwise the stored token. `None` when neither supplies one.
pub fn effective_token(stored: Option<&str>) -> Option<String> {
    if let Ok(env) = std::env::var(TOKEN_ENV) {
        if !env.trim().is_empty() {
            return Some(env);
        }
    }
    stored.map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Guard the shared process env: the two env tests never race.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn effective_token_prefers_env_override() {
        let _g = ENV_LOCK.lock().unwrap();
        std::env::set_var(TOKEN_ENV, "from-env");
        let got = effective_token(Some("stored"));
        std::env::remove_var(TOKEN_ENV);
        assert_eq!(got.as_deref(), Some("from-env"));
    }

    #[test]
    fn effective_token_falls_back_to_stored() {
        let _g = ENV_LOCK.lock().unwrap();
        std::env::remove_var(TOKEN_ENV);
        assert_eq!(effective_token(Some("stored")).as_deref(), Some("stored"));
        assert_eq!(effective_token(None), None);
        // An empty env var is ignored, not treated as a token.
        std::env::set_var(TOKEN_ENV, "   ");
        let got = effective_token(Some("stored"));
        std::env::remove_var(TOKEN_ENV);
        assert_eq!(got.as_deref(), Some("stored"));
    }
}
