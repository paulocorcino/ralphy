//! Writing and removing the Telegram config of a daemon store: the one writer
//! that `ralphy telegram` and the Settings routes share (ADR-0085, amendment
//! of 2026-11-07, B4). The file holds the bot token, so it is owner-only on
//! every platform (ADR-0072 D7).

use std::path::Path;

use anyhow::{Context, Result};
use ralphy_telegram::config::TelegramConfig;

use super::{config_path_in, legacy};

/// Write `cfg` to `path` owner-only, creating its folder: mode `0o600` from
/// the moment the file exists on unix, a protected DACL on Windows.
pub fn save_to(cfg: &TelegramConfig, path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        crate::owner_only::create_owner_only_dir(parent)?;
    }
    let text = toml::to_string_pretty(cfg).context("serializing telegram config")?;
    crate::owner_only::write_owner_only(path, text.as_bytes())
}

/// Remove the config of `store`, and the old file `old` when `store` is the
/// operator's store, so the removed bot is not copied back. A missing file
/// is success.
pub fn remove_in(store: &Path, old: &Path, operator_store: Option<&Path>) -> Result<()> {
    let path = config_path_in(store);
    match std::fs::remove_file(&path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e).with_context(|| format!("removing {}", path.display())),
    }
    legacy::remove_legacy(store, old, operator_store)?;
    Ok(())
}

/// [`remove_in`] for `store`, with the old file and the operator's store of
/// this machine.
pub fn remove_from(store: &Path) -> Result<()> {
    let old = ralphy_telegram::config::legacy_path()?;
    remove_in(store, &old, crate::auth::os_default_store().as_deref())
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
        assert!(crate::owner_only::is_owner_only(&path).unwrap());
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
        std::fs::write(config_path_in(&store), "token = \"t\"\n").unwrap();
        std::fs::write(&old, "token = \"t\"\n").unwrap();
        remove_in(&store, &old, Some(&store)).unwrap();
        assert!(!config_path_in(&store).exists());
        assert!(!old.exists(), "the old copy goes too");
        remove_in(&store, &old, Some(&store)).expect("nothing left is success");
    }
}
