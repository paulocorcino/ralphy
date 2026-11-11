//! The Telegram config in the daemon store (ADR-0007 D2, amendment of
//! 2026-11-03): `<store>/telegram.toml`, the file the daemon reads too.
//!
//! Reading the file (`TelegramConfig`, `effective_token`, `TOKEN_ENV`) lives in
//! the `ralphy-telegram` crate and is re-exported here, so
//! `crate::telegram::config::*` paths do not change. Writing and removing the
//! file is `ralphy_daemon::telegram_state::store`, which the Settings routes
//! use too.

use std::path::PathBuf;

use anyhow::Result;
use ralphy_daemon::telegram_state::{self, legacy};
pub(crate) use ralphy_telegram::config::{effective_token, TelegramConfig, TOKEN_ENV};

/// Where the config lives: `telegram.toml` in the daemon store.
pub(crate) fn path() -> Result<PathBuf> {
    telegram_state::config_path()
}

/// The config, after the one-time copy of the old file into the store, or
/// `None` when no bot is set up.
pub(crate) fn load() -> Result<Option<TelegramConfig>> {
    if let Err(e) = legacy::take_legacy_config() {
        eprintln!("warning: the old Telegram settings were not copied: {e}");
    }
    TelegramConfig::load_from(&path()?)
}

/// Write `cfg` to the store owner-only, creating the store folder.
pub(crate) fn save(cfg: &TelegramConfig) -> Result<()> {
    telegram_state::store::save_to(cfg, &path()?)
}

/// Remove the stored config, and the old file when this is the operator's
/// store, so the removed bot is not copied back. A missing file is success.
pub(crate) fn delete() -> Result<()> {
    telegram_state::store::remove_from(&ralphy_daemon::auth::store_dir()?)
}

/// Mask a bot token for display, revealing only a short suffix so a `status`
/// readout never leaks the secret.
pub(crate) fn masked_token(token: &str) -> String {
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
    fn masked_token_hides_all_but_suffix() {
        assert_eq!(masked_token("123456789"), "*****6789");
        // Short tokens are fully masked, leaking no suffix.
        assert_eq!(masked_token("abcd"), "****");
        assert_eq!(masked_token("ab"), "**");
        assert_eq!(masked_token(""), "");
    }
}
