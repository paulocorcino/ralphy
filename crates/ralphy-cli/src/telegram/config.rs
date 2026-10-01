//! The global Telegram config store (ADR-0007 D2).
//!
//! A single `config.toml` resolved with the `directories` crate
//! (`~/.config/ralphy/config.toml`; `%APPDATA%\ralphy\` on Windows) holds the
//! bot token and the auto-detected `chat_id`. It is written owner-only (`0o600`
//! on unix; a protected DACL on Windows). The environment variable
//! `RALPHY_TELEGRAM_TOKEN` overrides the stored token so a run can carry a token
//! without persisting it.

use std::path::PathBuf;

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
        let path = Self::config_path()?;
        match std::fs::read_to_string(&path) {
            Ok(text) => {
                let cfg =
                    toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
                Ok(Some(cfg))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
        }
    }

    /// Write the config to disk owner-only, creating the config directory.
    pub fn save(&self) -> Result<()> {
        self.save_to(&Self::config_path()?)
    }

    /// Write the config to `path` owner-only on every platform: mode `0o600`
    /// from the moment the file exists on unix, a protected DACL on Windows
    /// (ADR-0072 D7).
    fn save_to(&self, path: &std::path::Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            ralphy_daemon::owner_only::create_owner_only_dir(parent)?;
        }
        let text = toml::to_string_pretty(self).context("serializing telegram config")?;
        ralphy_daemon::owner_only::write_owner_only(path, text.as_bytes())
    }

    /// Remove the stored config. A missing file is treated as success.
    pub fn delete() -> Result<()> {
        let path = Self::config_path()?;
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e).with_context(|| format!("removing {}", path.display())),
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

/// Mask a bot token for display, revealing only a short suffix so a `status`
/// readout never leaks the secret.
pub fn masked_token(token: &str) -> String {
    const SUFFIX: usize = 4;
    let chars: Vec<char> = token.chars().collect();
    if chars.len() <= SUFFIX {
        return "*".repeat(chars.len());
    }
    let visible: String = chars[chars.len() - SUFFIX..].iter().collect();
    format!("{}{}", "*".repeat(chars.len() - SUFFIX), visible)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The config holds the bot token, so it is owner-only on every platform:
    /// a protected DACL on Windows, mode `0o600` elsewhere.
    #[test]
    fn the_saved_config_is_owner_only() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ralphy").join("config.toml");
        let cfg = TelegramConfig {
            token: "bot-token".into(),
            chat_id: Some(7),
        };
        cfg.save_to(&path).unwrap();
        assert!(ralphy_daemon::owner_only::is_owner_only(&path).unwrap());
        assert!(std::fs::read_to_string(&path)
            .unwrap()
            .contains("bot-token"));
    }

    #[test]
    fn masked_token_hides_all_but_suffix() {
        assert_eq!(masked_token("123456789"), "*****6789");
        // Short tokens are fully masked, leaking no suffix.
        assert_eq!(masked_token("abcd"), "****");
        assert_eq!(masked_token("ab"), "**");
        assert_eq!(masked_token(""), "");
    }

    #[test]
    fn effective_token_prefers_env_override() {
        // Guard the shared process env: serialize via a mutex so the two env
        // tests never race.
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

    use std::sync::Mutex;
    static ENV_LOCK: Mutex<()> = Mutex::new(());
}
