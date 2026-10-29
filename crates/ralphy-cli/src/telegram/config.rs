//! Writing the global Telegram config store (ADR-0007 D2).
//!
//! Reading the file (`TelegramConfig`, `effective_token`, `TOKEN_ENV`) lives in
//! the `ralphy-telegram` crate and is re-exported here, so
//! `crate::telegram::config::*` paths do not change. The file is written
//! owner-only (`0o600` on unix; a protected DACL on Windows), which needs the
//! daemon's `owner_only` module, so saving stays in the CLI.

use anyhow::{Context, Result};
pub use ralphy_telegram::config::{effective_token, TelegramConfig, TOKEN_ENV};

/// Write `cfg` to disk owner-only, creating the config directory.
pub fn save(cfg: &TelegramConfig) -> Result<()> {
    save_to(cfg, &TelegramConfig::config_path()?)
}

/// Write `cfg` to `path` owner-only on every platform: mode `0o600` from the
/// moment the file exists on unix, a protected DACL on Windows (ADR-0072 D7).
fn save_to(cfg: &TelegramConfig, path: &std::path::Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        ralphy_daemon::owner_only::create_owner_only_dir(parent)?;
    }
    let text = toml::to_string_pretty(cfg).context("serializing telegram config")?;
    ralphy_daemon::owner_only::write_owner_only(path, text.as_bytes())
}

/// Remove the stored config. A missing file is treated as success.
pub fn delete() -> Result<()> {
    let path = TelegramConfig::config_path()?;
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e).with_context(|| format!("removing {}", path.display())),
    }
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
        save_to(&cfg, &path).unwrap();
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
}
