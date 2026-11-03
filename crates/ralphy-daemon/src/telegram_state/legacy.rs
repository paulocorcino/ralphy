//! The one-time copy of the Telegram config into the daemon store (ADR-0007
//! D2, amendment of 2026-11-03). The config used to live where `directories`
//! puts it; it now lives in the store. Only the operator's own store takes the
//! old file: a test's store must never get the operator's bot. The old file
//! is kept, because an older binary, after a rollback, reads only that file.

use std::path::Path;

use anyhow::{Context, Result};

use super::config_path_in;
use crate::auth::{os_default_store, same_dir, store_dir};

/// What the copy came to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Moved {
    /// The old file was copied into the store.
    Copied,
    /// The store has a config of its own; the old file is not read.
    StoreHasConfig,
    /// There is no old file.
    NoLegacy,
    /// The store is not the operator's own; the old file is never read.
    NotOperatorStore,
}

/// Whether `store` is the operator's own store.
fn is_operator_store(store: &Path, operator_store: Option<&Path>) -> bool {
    operator_store.is_some_and(|operator| same_dir(store, operator))
}

/// Copy the config at `legacy` into `store`, owner-only, when `store` is the
/// operator's own store and has no config yet.
pub fn move_legacy(store: &Path, legacy: &Path, operator_store: Option<&Path>) -> Result<Moved> {
    if !is_operator_store(store, operator_store) {
        return Ok(Moved::NotOperatorStore);
    }
    let target = config_path_in(store);
    if target.exists() {
        return Ok(Moved::StoreHasConfig);
    }
    let bytes = match std::fs::read(legacy) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Moved::NoLegacy),
        Err(e) => return Err(e).with_context(|| format!("reading {}", legacy.display())),
    };
    crate::owner_only::write_owner_only(&target, &bytes)
        .with_context(|| format!("copying the Telegram config to {}", target.display()))?;
    Ok(Moved::Copied)
}

/// Remove the old file when `store` is the operator's own store, so a bot the
/// operator removed is not copied back. Returns whether a file was removed.
pub fn remove_legacy(store: &Path, legacy: &Path, operator_store: Option<&Path>) -> Result<bool> {
    if !is_operator_store(store, operator_store) {
        return Ok(false);
    }
    match std::fs::remove_file(legacy) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e).with_context(|| format!("removing {}", legacy.display())),
    }
}

/// [`move_legacy`] on this machine: the store of `auth::store_dir`, the old
/// file `directories` names, and the store the operating system names. A
/// store that is not the operator's never looks for the old file, so a test
/// that moved the home folder, where `directories` names no folder, still
/// reads its own store.
pub fn take_legacy_config() -> Result<Moved> {
    let store = store_dir()?;
    let operator_store = os_default_store();
    if !is_operator_store(&store, operator_store.as_deref()) {
        return Ok(Moved::NotOperatorStore);
    }
    let legacy = ralphy_telegram::config::legacy_path()?;
    move_legacy(&store, &legacy, operator_store.as_deref())
}

#[cfg(test)]
mod tests {
    use super::*;

    const BOT: &str = "token = \"123:abc\"\nchat_id = 42\n";

    /// A store, the operator's store, and an old config with a bot.
    fn layout() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
        let root = tempfile::tempdir().expect("a temp dir");
        let store = root.path().join("store");
        std::fs::create_dir(&store).expect("the store");
        let legacy = root.path().join("config.toml");
        std::fs::write(&legacy, BOT).expect("the old config");
        (root, store, legacy)
    }

    #[test]
    fn a_scratch_store_never_takes_the_legacy_config() {
        let (root, store, legacy) = layout();
        let operator = root.path().join("operator");
        std::fs::create_dir(&operator).expect("the operator's store");
        let moved = move_legacy(&store, &legacy, Some(&operator)).expect("the copy");
        assert_eq!(moved, Moved::NotOperatorStore);
        assert!(!config_path_in(&store).exists(), "a test store gets no bot");
        assert_eq!(
            move_legacy(&store, &legacy, None).expect("no home"),
            Moved::NotOperatorStore
        );
        assert!(!remove_legacy(&store, &legacy, Some(&operator)).expect("no removal"));
        assert!(
            legacy.exists(),
            "a test store never removes the operator's file"
        );
    }

    #[test]
    fn the_operator_store_takes_the_legacy_config_owner_only() {
        let (_root, store, legacy) = layout();
        let moved = move_legacy(&store, &legacy, Some(&store)).expect("the copy");
        assert_eq!(moved, Moved::Copied);
        let target = config_path_in(&store);
        assert_eq!(std::fs::read_to_string(&target).expect("the copy"), BOT);
        assert!(crate::owner_only::is_owner_only(&target).expect("the mode"));
        assert!(legacy.exists(), "the old file stays for a rollback");
    }

    #[test]
    fn a_store_config_is_never_overwritten() {
        let (_root, store, legacy) = layout();
        std::fs::write(config_path_in(&store), "token = \"mine\"\n").expect("the store's own");
        let moved = move_legacy(&store, &legacy, Some(&store)).expect("the copy");
        assert_eq!(moved, Moved::StoreHasConfig);
        let kept = std::fs::read_to_string(config_path_in(&store)).expect("the store's config");
        assert_eq!(kept, "token = \"mine\"\n");
    }

    #[test]
    fn no_old_file_copies_nothing_and_removing_the_old_file_needs_the_operator_store() {
        let (_root, store, legacy) = layout();
        assert!(remove_legacy(&store, &legacy, Some(&store)).expect("the removal"));
        assert!(!legacy.exists());
        assert!(!remove_legacy(&store, &legacy, Some(&store)).expect("nothing left"));
        let moved = move_legacy(&store, &legacy, Some(&store)).expect("the copy");
        assert_eq!(moved, Moved::NoLegacy);
        assert!(!config_path_in(&store).exists());
    }

    /// Windows compares folders without case: the operator's store named in
    /// another case is the same store.
    #[cfg(windows)]
    #[test]
    fn the_operator_store_matches_in_any_case() {
        let (_root, store, legacy) = layout();
        let other_case = std::path::PathBuf::from(store.to_string_lossy().to_uppercase());
        let moved = move_legacy(&store, &legacy, Some(&other_case)).expect("the copy");
        assert_eq!(moved, Moved::Copied);
    }
}
