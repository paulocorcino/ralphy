//! The Telegram config in the daemon store (ADR-0007 D2, amendment of
//! 2026-11-03): `<store>/telegram.toml`, the file the daemon reads too.
//!
//! Reading the file (`TelegramConfig`, `effective_token`, `TOKEN_ENV`) lives in
//! the `ralphy-telegram` crate and is re-exported here, so
//! `crate::telegram::config::*` paths do not change. The file is written
//! owner-only (`0o600` on unix; a protected DACL on Windows), which needs the
//! daemon's `owner_only` module, so saving stays in the CLI.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use ralphy_daemon::telegram_state::{self, legacy};
pub use ralphy_telegram::config::{effective_token, TelegramConfig, TOKEN_ENV};

/// Where the config lives: `telegram.toml` in the daemon store.
pub fn path() -> Result<PathBuf> {
    telegram_state::config_path()
}

/// The config, after the one-time copy of the old file into the store, or
/// `None` when no bot is set up.
pub fn load() -> Result<Option<TelegramConfig>> {
    legacy::take_legacy_config()?;
    TelegramConfig::load_from(&path()?)
}

/// Write `cfg` to the store owner-only, creating the store folder.
pub fn save(cfg: &TelegramConfig) -> Result<()> {
    save_to(cfg, &path()?)
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

/// Remove the stored config, and the old file when this is the operator's
/// store, so the removed bot is not copied back. A missing file is success.
pub fn delete() -> Result<()> {
    let store = ralphy_daemon::auth::store_dir()?;
    let old = ralphy_telegram::config::legacy_path()?;
    delete_in(
        &store,
        &old,
        ralphy_daemon::auth::os_default_store().as_deref(),
    )
}

fn delete_in(store: &Path, old: &Path, operator_store: Option<&Path>) -> Result<()> {
    let path = telegram_state::config_path_in(store);
    match std::fs::remove_file(&path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e).with_context(|| format!("removing {}", path.display())),
    }
    legacy::remove_legacy(store, old, operator_store)?;
    Ok(())
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

    /// `disable` removes the store's config and the old copy, or the next start
    /// would copy the removed bot back.
    #[test]
    fn disable_removes_both_configs() {
        let root = tempfile::tempdir().unwrap();
        let store = root.path().join("store");
        std::fs::create_dir(&store).unwrap();
        let old = root.path().join("config.toml");
        std::fs::write(telegram_state::config_path_in(&store), "token = \"t\"\n").unwrap();
        std::fs::write(&old, "token = \"t\"\n").unwrap();
        delete_in(&store, &old, Some(&store)).unwrap();
        assert!(!telegram_state::config_path_in(&store).exists());
        assert!(!old.exists(), "the old copy goes too");
        delete_in(&store, &old, Some(&store)).expect("nothing left is success");
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
