//! The global per-repo events config store `~/.ralphy/events.toml` (ADR-0019).
//!
//! The sink's URL and token live per repo in one global TOML file — keyed by the
//! same `owner/repo` slug the usage ledger uses (ADR-0008) — rather than in the
//! per-repo `.ralphy/settings.json`, so the endpoint travels with the operator
//! across every repo and never lands in a committed (well, gitignored) settings
//! file. The file is written owner-only. `RALPHY_EVENTS_TOKEN` overrides the stored
//! token for a run so a token can be carried without persisting it, mirroring the
//! Telegram store's `RALPHY_TELEGRAM_TOKEN`.

use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Environment variable that overrides the stored per-repo token for a run.
pub const TOKEN_ENV: &str = "RALPHY_EVENTS_TOKEN";

/// Test/override hook for the store's base directory, mirroring the ledger's
/// `RALPHY_USAGE_DIR` (ADR-0008). When set, `events.toml` lives directly under it.
const DIR_ENV: &str = "RALPHY_EVENTS_DIR";

/// Serializes every test that mutates the process-global `RALPHY_EVENTS_DIR` /
/// `RALPHY_EVENTS_TOKEN` env vars — shared across this module's tests and the
/// `config` command's tests so the two never race on the same store.
#[cfg(test)]
pub(crate) static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// One repo's sink configuration: the endpoint URL and (optionally) the bearer
/// token. Either may be absent — a URL with no token posts without an
/// `Authorization` header.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventsEntry {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
}

impl EventsEntry {
    /// Whether the entry holds nothing (both fields cleared) — such an entry is
    /// dropped on `clear` so an empty stanza never lingers.
    fn is_empty(&self) -> bool {
        self.url.is_none() && self.token.is_none()
    }
}

/// The whole store: one [`EventsEntry`] per repo slug.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventsStore {
    #[serde(default)]
    pub repos: BTreeMap<String, EventsEntry>,
}

impl EventsStore {
    /// The store root: `$RALPHY_EVENTS_DIR` when set (tests point it at a temp dir),
    /// else `<home>/.ralphy`. `None` when no home directory can be resolved.
    fn base_dir() -> Option<PathBuf> {
        if let Some(dir) = std::env::var_os(DIR_ENV) {
            return Some(PathBuf::from(dir));
        }
        let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME"))?;
        Some(PathBuf::from(home).join(".ralphy"))
    }

    /// The on-disk path of `events.toml`.
    pub fn config_path() -> Result<PathBuf> {
        Ok(Self::base_dir()
            .context("could not resolve a home directory for the events store")?
            .join("events.toml"))
    }

    /// Load the store from disk, or an empty store when no file exists yet.
    pub fn load() -> Result<EventsStore> {
        let path = Self::config_path()?;
        match std::fs::read_to_string(&path) {
            Ok(text) => {
                toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(EventsStore::default()),
            Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
        }
    }

    /// Write the store to disk owner-only, creating the base directory.
    pub fn save(&self) -> Result<()> {
        let path = Self::config_path()?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        let text = toml::to_string_pretty(self).context("serializing events store")?;
        std::fs::write(&path, text).with_context(|| format!("writing {}", path.display()))?;
        set_owner_only(&path)?;
        Ok(())
    }

    /// The entry for `slug`, if any.
    pub fn entry(&self, slug: &str) -> Option<&EventsEntry> {
        self.repos.get(slug)
    }

    /// Set the endpoint URL for `slug`, creating the entry if absent.
    pub fn set_url(&mut self, slug: &str, url: &str) {
        self.repos.entry(slug.to_string()).or_default().url = Some(url.to_string());
    }

    /// Set the bearer token for `slug`, creating the entry if absent.
    pub fn set_token(&mut self, slug: &str, token: &str) {
        self.repos.entry(slug.to_string()).or_default().token = Some(token.to_string());
    }

    /// Clear one field (`"url"` or `"token"`) of `slug`'s entry, dropping the entry
    /// entirely once both fields are empty so no vacant stanza lingers.
    pub fn clear(&mut self, slug: &str, field: &str) {
        if let Some(e) = self.repos.get_mut(slug) {
            match field {
                "url" => e.url = None,
                "token" => e.token = None,
                _ => {}
            }
            if e.is_empty() {
                self.repos.remove(slug);
            }
        }
    }
}

/// The token a run should use: `RALPHY_EVENTS_TOKEN` when set and non-empty,
/// otherwise the `stored` per-repo token. `None` when neither supplies one.
pub fn effective_token(stored: Option<&str>) -> Option<String> {
    if let Ok(env) = std::env::var(TOKEN_ENV) {
        if !env.trim().is_empty() {
            return Some(env);
        }
    }
    stored.map(str::to_owned)
}

/// Restrict a freshly written store file to the owner only (mode `0o600` on unix;
/// the per-user home ACL on Windows), mirroring the Telegram store.
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    static N: AtomicU32 = AtomicU32::new(0);

    fn temp_dir() -> PathBuf {
        std::env::temp_dir().join(format!(
            "ralphy-events-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ))
    }

    #[test]
    fn round_trips_slug_entry_and_env_override_wins() {
        let _g = ENV_LOCK.lock().unwrap();
        let dir = temp_dir();
        std::env::set_var(DIR_ENV, &dir);
        std::env::remove_var(TOKEN_ENV);

        // Set url + token for a slug, save, reload, and read the entry back.
        let mut store = EventsStore::load().unwrap();
        store.set_url("o/r", "http://example/hook");
        store.set_token("o/r", "sekret");
        store.save().unwrap();

        let back = EventsStore::load().unwrap();
        let entry = back.entry("o/r").expect("entry present");
        assert_eq!(entry.url.as_deref(), Some("http://example/hook"));
        assert_eq!(entry.token.as_deref(), Some("sekret"));

        // effective_token: the stored token when no env override.
        assert_eq!(
            effective_token(entry.token.as_deref()).as_deref(),
            Some("sekret")
        );
        // The env var overrides the stored token for a run.
        std::env::set_var(TOKEN_ENV, "from-env");
        assert_eq!(
            effective_token(entry.token.as_deref()).as_deref(),
            Some("from-env")
        );
        // An empty env var is ignored, falling back to the stored token.
        std::env::set_var(TOKEN_ENV, "   ");
        assert_eq!(
            effective_token(entry.token.as_deref()).as_deref(),
            Some("sekret")
        );

        // clear drops fields and, once empty, the whole entry.
        let mut store = back.clone();
        store.clear("o/r", "token");
        assert!(store.entry("o/r").unwrap().token.is_none());
        store.clear("o/r", "url");
        assert!(store.entry("o/r").is_none(), "empty entry is dropped");

        std::env::remove_var(TOKEN_ENV);
        std::env::remove_var(DIR_ENV);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn slug_key_with_slash_round_trips_through_toml() {
        // A slug carries a `/`; TOML must quote the key so it reloads intact.
        let mut store = EventsStore::default();
        store.set_url("owner/repo", "http://x");
        let text = toml::to_string_pretty(&store).unwrap();
        let back: EventsStore = toml::from_str(&text).unwrap();
        assert_eq!(
            back.entry("owner/repo").and_then(|e| e.url.as_deref()),
            Some("http://x")
        );
    }
}
