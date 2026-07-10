//! The daemon's repo registry (docs/adr/0032): the set of repos Ralphy knows
//! about, keyed by `owner/repo` slug, persisted in the global store
//! (`<home>/.ralphy/repos.toml`) — never under a repo-local `.ralphy/`.
//!
//! Pure sync and path-explicit, mirroring `identity`: tests pass a temp path and
//! never mutate the process-global env (the `RALPHY_*_DIR` env-race trap). The
//! CLI (which has `ralphy-core`) computes the slug and calls this store; this
//! module never depends on `ralphy-core` (ADR-0032: the daemon must not import
//! the core). Reachability is computed at read time, never persisted — a stale
//! flag would contradict "never removed automatically" and self-healing.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// One registered repo: just its filesystem path. Reachability is derived, not
/// stored, so a moved repo self-heals and a returned repo un-greys with no write.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoEntry {
    pub path: String,
}

impl RepoEntry {
    /// Whether the stored path currently resolves to a directory. Computed on
    /// each read — an unreachable repo is flagged, never removed.
    pub fn reachable(&self) -> bool {
        Path::new(&self.path).is_dir()
    }
}

/// The persisted registry: slug → entry. The slug carries a `/`, so TOML quotes
/// the key (see the round-trip test).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistryStore {
    #[serde(default)]
    pub repos: BTreeMap<String, RepoEntry>,
}

impl RegistryStore {
    /// Insert or overwrite the entry for `slug`. Overwriting is the self-heal:
    /// a moved repo re-registers under the same slug with its new path.
    pub fn upsert(&mut self, slug: &str, path: &str) {
        self.repos.insert(
            slug.into(),
            RepoEntry {
                path: path.into(),
            },
        );
    }

    /// Remove the entry for `slug`; `true` when one was present (idempotent:
    /// removing an absent slug returns `false`).
    pub fn remove(&mut self, slug: &str) -> bool {
        self.repos.remove(slug).is_some()
    }

    /// The entry for `slug`, if registered.
    pub fn entry(&self, slug: &str) -> Option<&RepoEntry> {
        self.repos.get(slug)
    }
}

/// Load a [`RegistryStore`] from `path`, or `Ok(default())` when the file does
/// not exist yet (no repos registered).
pub fn load_from(path: &Path) -> Result<RegistryStore> {
    match std::fs::read_to_string(path) {
        Ok(text) => toml::from_str(&text).with_context(|| format!("parsing {}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(RegistryStore::default()),
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

/// Write the store to `path` owner-only, creating the parent directory.
pub fn save_to(store: &RegistryStore, path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    let text = toml::to_string_pretty(store).context("serializing repo registry")?;
    std::fs::write(path, text).with_context(|| format!("writing {}", path.display()))?;
    set_owner_only(path)?;
    Ok(())
}

/// The production path of `repos.toml`: `$RALPHY_DAEMON_DIR` when set (tests
/// point it at a temp dir), else `<home>/.ralphy/repos.toml` — the same global
/// store root as `daemon.toml`, never a repo-local `.ralphy/`.
pub fn repos_toml_path() -> Result<PathBuf> {
    if let Some(dir) = std::env::var_os("RALPHY_DAEMON_DIR") {
        return Ok(PathBuf::from(dir).join("repos.toml"));
    }
    let home = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .context("could not resolve a home directory for the repo registry store")?;
    Ok(PathBuf::from(home).join(".ralphy").join("repos.toml"))
}

/// Load the current registry from its production path.
pub fn load_current() -> Result<RegistryStore> {
    load_from(&repos_toml_path()?)
}

/// Restrict a freshly written store file to the owner only (mode `0o600` on
/// unix; the per-user home ACL on Windows), mirroring the identity store.
#[cfg(unix)]
fn set_owner_only(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let perms = std::fs::Permissions::from_mode(0o600);
    std::fs::set_permissions(path, perms)
        .with_context(|| format!("setting owner-only permissions on {}", path.display()))
}

#[cfg(not(unix))]
fn set_owner_only(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upsert_self_heals_same_slug_new_path() {
        let mut store = RegistryStore::default();
        store.upsert("owner/repo", "/old");
        store.upsert("owner/repo", "/new");
        assert_eq!(store.repos.len(), 1, "same slug must not duplicate");
        assert_eq!(store.entry("owner/repo").unwrap().path, "/new");
    }

    #[test]
    fn remove_is_idempotent() {
        let mut store = RegistryStore::default();
        store.upsert("owner/repo", "/some");
        assert!(store.remove("owner/repo"), "first remove reports true");
        assert!(!store.remove("owner/repo"), "second remove reports false");
        assert!(store.entry("owner/repo").is_none());
    }

    #[test]
    fn unreachable_entry_retained_and_flagged() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("repos.toml");
        let mut store = RegistryStore::default();
        store.upsert("owner/gone", "/no/such/path/exists");
        store.upsert("owner/here", &dir.path().to_string_lossy());
        save_to(&store, &path).unwrap();

        let back = load_from(&path).unwrap();
        assert!(
            back.entry("owner/gone").is_some(),
            "an unreachable entry is retained, never removed"
        );
        assert!(!back.entry("owner/gone").unwrap().reachable());
        assert!(back.entry("owner/here").unwrap().reachable());
    }

    #[test]
    fn slug_with_slash_round_trips_through_toml() {
        let mut store = RegistryStore::default();
        store.upsert("owner/repo", "/somewhere");
        let text = toml::to_string_pretty(&store).unwrap();
        let back: RegistryStore = toml::from_str(&text).unwrap();
        assert_eq!(back.entry("owner/repo").unwrap().path, "/somewhere");
    }
}
