//! The global Telegram config store (ADR-0007 D2).
//!
//! A single `config.toml` resolved with the `directories` crate
//! (`~/.config/ralphy/config.toml`; `%APPDATA%\ralphy\` on Windows) holds the
//! bot token and the auto-detected `chat_id`. It is written owner-only (`0o600`
//! on unix; the per-user `%APPDATA%` ACL on Windows). The environment variable
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
                let cfg = toml::from_str(&text)
                    .with_context(|| format!("parsing {}", path.display()))?;
                Ok(Some(cfg))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
        }
    }

    /// Write the config to disk owner-only, creating the config directory.
    pub fn save(&self) -> Result<()> {
        let path = Self::config_path()?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        let text = toml::to_string_pretty(self).context("serializing telegram config")?;
        std::fs::write(&path, text).with_context(|| format!("writing {}", path.display()))?;
        set_owner_only(&path)?;
        Ok(())
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

/// Restrict a freshly written config file to the owner only.
///
/// On unix this sets mode `0o600`. On Windows the file inherits the per-user
/// `%APPDATA%` ACL and no extra hardening is applied this slice (ADR-0007 D2).
#[cfg(unix)]
fn set_owner_only(path: &std::path::Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let perms = std::fs::Permissions::from_mode(0o600);
    std::fs::set_permissions(path, perms)
        .with_context(|| format!("setting owner-only permissions on {}", path.display()))
}

#[cfg(not(unix))]
fn set_owner_only(_path: &std::path::Path) -> Result<()> {
    Ok(())
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

    #[test]
    fn toml_round_trips_token_and_chat_id() {
        let cfg = TelegramConfig {
            token: "123456:abcdef".to_string(),
            chat_id: Some(42),
        };
        let text = toml::to_string_pretty(&cfg).unwrap();
        let back: TelegramConfig = toml::from_str(&text).unwrap();
        assert_eq!(cfg, back);
        assert_eq!(back.token, "123456:abcdef");
        assert_eq!(back.chat_id, Some(42));
    }

    #[test]
    fn toml_round_trips_without_chat_id() {
        let cfg = TelegramConfig {
            token: "t".to_string(),
            chat_id: None,
        };
        let text = toml::to_string_pretty(&cfg).unwrap();
        let back: TelegramConfig = toml::from_str(&text).unwrap();
        assert_eq!(cfg, back);
        assert_eq!(back.chat_id, None);
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
