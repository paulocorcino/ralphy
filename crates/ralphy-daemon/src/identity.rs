//! The daemon's identity store (docs/adr/0032 §8): a mint-once `daemon_id`, an
//! operator-chosen `name`, and a cosmetic `avatar`, persisted in the global
//! store (`<home>/.ralphy/daemon.toml`) — never under a repo-local `.ralphy/`.
//!
//! Pure sync: mint, persist, and validate live here; the async routes in
//! `lib.rs` read a loaded [`Identity`] but never touch this module's I/O on the
//! request path. The store API is path-explicit so tests pass a temp path and
//! never mutate the process-global env (the `RALPHY_*_DIR` env-race trap).

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use ulid::Ulid;

/// The daemon's persisted identity. `id` is minted exactly once and survives
/// every rename or avatar change (mint-once); `name`/`avatar` are mutable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Identity {
    pub id: Ulid,
    pub name: String,
    pub avatar: String,
}

/// The cosmetic avatar pool (ADR-0032 §8): picked by number from this list, not
/// entered as a raw emoji. Non-unique, presented 1-indexed to the operator.
pub const AVATARS: &[&str] = &[
    "🐙", "🐺", "🦊", "🦉", "🦅", "🐝", "🦫", "🦈", "🐢", "🐉", "🦂", "🦭",
];

/// The avatar at 1-indexed position `n`, or `None` when out of range (`n == 0`
/// or past the list). The operator picks by the number shown, which starts at 1.
pub fn avatar_by_number(n: usize) -> Option<&'static str> {
    if n == 0 {
        return None;
    }
    AVATARS.get(n - 1).copied()
}

/// The status line "avatar name" (e.g. "🐙 anvil") shown by `ralphy daemon
/// status` and the page header.
pub fn format_status_line(id: &Identity) -> String {
    format!("{} {}", id.avatar, id.name)
}

/// Names reserved because they collide with Ralphy's command vocabulary
/// (ADR-0032 §6/§8 + CONTEXT.md): a daemon name must stay unambiguous to the
/// models that parse the vocabulary. Indicative — the dispatcher slice may
/// re-home this list to a single authoritative source (see plan Caveats).
pub const RESERVED: &[&str] = &[
    "run",
    "triage",
    "queue",
    "status",
    "session",
    "sessions",
    "open",
    "close",
    "reattach",
    "list",
    "add",
    "remove",
    "forge",
    "issue",
    "thread",
    "label",
    "branch",
    "daemon",
    "install",
    "uninstall",
    "enroll",
    "console",
    "setup",
];

/// Why a proposed daemon name was refused, each carrying an operator-facing
/// message via `Display`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NameError {
    Empty,
    InvalidChars,
    Reserved(String),
}

impl std::fmt::Display for NameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NameError::Empty => write!(f, "name must not be empty"),
            NameError::InvalidChars => write!(
                f,
                "name may only contain lowercase letters, digits, and hyphens"
            ),
            NameError::Reserved(term) => write!(
                f,
                "'{term}' is reserved (it collides with Ralphy's command vocabulary) — pick another name"
            ),
        }
    }
}

impl std::error::Error for NameError {}

/// Normalize and validate a proposed name: trim, lowercase, then reject empty,
/// any char outside `[a-z0-9-]`, or a match against [`RESERVED`]. On success
/// returns the canonical (lowercase) form.
pub fn validate_name(raw: &str) -> std::result::Result<String, NameError> {
    let normalized = raw.trim().to_ascii_lowercase();
    if normalized.is_empty() {
        return Err(NameError::Empty);
    }
    if !normalized
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        return Err(NameError::InvalidChars);
    }
    if let Some(term) = RESERVED.iter().find(|r| normalized.eq_ignore_ascii_case(r)) {
        return Err(NameError::Reserved((*term).to_string()));
    }
    Ok(normalized)
}

/// Suggest a default name from the machine's hostname: the segment before the
/// first `.`, lowercased and stripped to `[a-z0-9-]`. Falls back to `"ralphy"`
/// when the result is empty or itself reserved.
pub fn suggest_name(hostname: &str) -> String {
    let stem: String = hostname
        .split('.')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase()
        .chars()
        .filter(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == '-')
        .collect();
    if stem.is_empty() || RESERVED.iter().any(|r| stem.eq_ignore_ascii_case(r)) {
        return "ralphy".to_string();
    }
    stem
}

/// Load an [`Identity`] from `path`, or `Ok(None)` when the file does not exist
/// yet (an un-baptized daemon).
pub fn load_from(path: &Path) -> Result<Option<Identity>> {
    match std::fs::read_to_string(path) {
        Ok(text) => {
            let id =
                toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
            Ok(Some(id))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

/// Write `id` to `path` owner-only, creating the parent directory.
pub fn save_to(id: &Identity, path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    let text = toml::to_string_pretty(id).context("serializing daemon identity")?;
    std::fs::write(path, text).with_context(|| format!("writing {}", path.display()))?;
    set_owner_only(path)?;
    Ok(())
}

/// Baptize (or re-baptize) the daemon at `path`: mint-once semantics. Any
/// existing file's `id` is KEPT (a new ULID is minted only when none exists);
/// `name`/`avatar` are overwritten; the result is saved and returned. A rename
/// or avatar change never re-mints the `id`.
pub fn baptize(path: &Path, name: String, avatar: String) -> Result<Identity> {
    let id = match load_from(path)? {
        Some(existing) => existing.id,
        None => Ulid::new(),
    };
    let identity = Identity { id, name, avatar };
    save_to(&identity, path)?;
    Ok(identity)
}

/// The production path of `daemon.toml`: `$RALPHY_DAEMON_DIR` when set (tests
/// point it at a temp dir), else `<home>/.ralphy/daemon.toml` — the same global
/// store root as `events.toml`, never a repo-local `.ralphy/`.
pub fn daemon_toml_path() -> Result<PathBuf> {
    if let Some(dir) = std::env::var_os("RALPHY_DAEMON_DIR") {
        return Ok(PathBuf::from(dir).join("daemon.toml"));
    }
    let home = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .context("could not resolve a home directory for the daemon identity store")?;
    Ok(PathBuf::from(home).join(".ralphy").join("daemon.toml"))
}

/// Load the current daemon identity from its production path.
pub fn load_current() -> Result<Option<Identity>> {
    load_from(&daemon_toml_path()?)
}

/// Restrict a freshly written store file to the owner only (mode `0o600` on
/// unix; the per-user home ACL on Windows), mirroring the events store.
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

    /// Serializes tests that mutate the process-global home / `RALPHY_DAEMON_DIR`
    /// env vars so they never race on the resolved path.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn mint_once_id_is_stable() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("daemon.toml");
        let first = baptize(&path, "anvil".into(), "🐙".into()).unwrap();
        let second = baptize(&path, "renamed".into(), "🐺".into()).unwrap();
        assert_eq!(first.id, second.id, "id must survive a re-baptism");
        assert_eq!(second.name, "renamed");
        assert_eq!(second.avatar, "🐺");
    }

    #[test]
    fn reserved_names_refused() {
        assert!(matches!(validate_name("run"), Err(NameError::Reserved(_))));
        assert!(matches!(
            validate_name("queue"),
            Err(NameError::Reserved(_))
        ));
        assert!(matches!(
            validate_name("forge"),
            Err(NameError::Reserved(_))
        ));
        assert_eq!(validate_name("anvil").unwrap(), "anvil");
    }

    #[test]
    fn validate_name_normalizes_and_rejects_bad_chars() {
        assert_eq!(validate_name("  Anvil ").unwrap(), "anvil");
        assert!(matches!(validate_name(""), Err(NameError::Empty)));
        assert!(matches!(
            validate_name("has space"),
            Err(NameError::InvalidChars)
        ));
    }

    #[test]
    fn suggest_name_strips_domain() {
        assert_eq!(suggest_name("MyBox.example.com"), "mybox");
        assert_eq!(suggest_name("run"), "ralphy", "reserved stem falls back");
        assert_eq!(suggest_name("...."), "ralphy", "empty stem falls back");
    }

    #[test]
    fn avatar_by_number_is_one_indexed() {
        assert_eq!(avatar_by_number(1), Some(AVATARS[0]));
        assert_eq!(avatar_by_number(0), None);
        assert_eq!(avatar_by_number(AVATARS.len() + 1), None);
        assert_eq!(
            avatar_by_number(AVATARS.len()),
            Some(AVATARS[AVATARS.len() - 1])
        );
    }

    #[test]
    fn identity_round_trips_to_disk() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("daemon.toml");
        let original = baptize(&path, "anvil".into(), "🐙".into()).unwrap();
        let loaded = load_from(&path).unwrap().expect("just saved");
        assert_eq!(original, loaded);
    }

    #[test]
    fn load_from_missing_is_none() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("absent.toml");
        assert_eq!(load_from(&path).unwrap(), None);
    }

    #[test]
    fn daemon_toml_path_under_global_store() {
        let _g = ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let prev_daemon = std::env::var_os("RALPHY_DAEMON_DIR");
        let prev_profile = std::env::var_os("USERPROFILE");
        let prev_home = std::env::var_os("HOME");
        std::env::remove_var("RALPHY_DAEMON_DIR");
        std::env::set_var("USERPROFILE", dir.path());
        std::env::set_var("HOME", dir.path());

        let path = daemon_toml_path().unwrap();
        assert!(
            path.ends_with(Path::new(".ralphy").join("daemon.toml")),
            "path must live at <home>/.ralphy/daemon.toml, got {}",
            path.display()
        );
        assert!(
            path.starts_with(dir.path()),
            "path must be under the temp home"
        );

        // Restore the process-global env for other tests.
        match prev_daemon {
            Some(v) => std::env::set_var("RALPHY_DAEMON_DIR", v),
            None => std::env::remove_var("RALPHY_DAEMON_DIR"),
        }
        match prev_profile {
            Some(v) => std::env::set_var("USERPROFILE", v),
            None => std::env::remove_var("USERPROFILE"),
        }
        match prev_home {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
    }
}
