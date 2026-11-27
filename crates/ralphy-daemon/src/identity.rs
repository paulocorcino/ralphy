//! The daemon's identity store (docs/adr/0032 §8): a mint-once `daemon_id`, an
//! operator-chosen `name`, and a cosmetic `avatar`, persisted in the global
//! store (`<home>/.ralphy/daemon.toml`) — never under a repo-local `.ralphy/`.
//!
//! Pure sync: mint, persist, and validate live here; the async routes in
//! `routes/` read a loaded [`Identity`] but never touch this module's I/O on the
//! request path. The store API is path-explicit so tests pass a temp path and
//! never mutate the process-global env (the `RALPHY_*_DIR` env-race trap).

use std::ffi::OsString;
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
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum NameError {
    #[error("name must not be empty")]
    Empty,
    #[error("name may only contain lowercase letters, digits, and hyphens")]
    InvalidChars,
    #[error(
        "'{0}' is reserved (it collides with Ralphy's command vocabulary) — pick another name"
    )]
    Reserved(String),
}

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
        return Err(NameError::Reserved((*term).to_owned()));
    }
    Ok(normalized)
}

/// Suggest a default name from the machine's hostname: the segment before the
/// first `.`, lowercased and stripped to `[a-z0-9-]`. Falls back to `"ralphy"`
/// when the result is empty or itself reserved.
pub fn suggest_name(hostname: &str) -> String {
    let stem = name_stem(hostname);
    if stem.is_empty() || RESERVED.iter().any(|r| stem.eq_ignore_ascii_case(r)) {
        return "ralphy".to_owned();
    }
    stem
}

/// The part of a machine or account name that a daemon name may hold: the
/// segment before the first `.`, lowercased and stripped to `[a-z0-9-]`.
fn name_stem(raw: &str) -> String {
    raw.split('.')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase()
        .chars()
        .filter(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == '-')
        .collect()
}

/// The name a host daemon gets when the operator gives none: `host-user`, or
/// `host` alone when the user is empty. When the result is taken (ignoring
/// case) or not valid, `-2`, `-3` and so on are tried until one is free.
/// `None` when the host part is empty.
pub fn default_name(host: &str, user: Option<&str>, taken: &[String]) -> Option<String> {
    let host = name_stem(host);
    let user = name_stem(user.unwrap_or(""));
    let base = match (host.is_empty(), user.is_empty()) {
        (true, _) => return None,
        (false, true) => host,
        (false, false) => format!("{host}-{user}"),
    };
    let free = |n: &str| !taken.iter().any(|t| t.eq_ignore_ascii_case(n));
    if let Ok(n) = validate_name(&base) {
        if free(&n) {
            return Some(n);
        }
    }
    (2..).find_map(|i| {
        validate_name(&format!("{base}-{i}"))
            .ok()
            .filter(|n| free(n))
    })
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
        crate::owner_only::create_owner_only_dir(parent)?;
    }
    let text = toml::to_string_pretty(id).context("serializing daemon identity")?;
    crate::owner_only::write_owner_only(path, text.as_bytes())
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
    let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME"));
    daemon_toml_path_for(std::env::var_os("RALPHY_DAEMON_DIR"), home)
}

fn daemon_toml_path_for(daemon_dir: Option<OsString>, home: Option<OsString>) -> Result<PathBuf> {
    if let Some(dir) = daemon_dir {
        return Ok(PathBuf::from(dir).join("daemon.toml"));
    }
    let home = home.context("could not resolve a home directory for the daemon identity store")?;
    Ok(PathBuf::from(home).join(".ralphy").join("daemon.toml"))
}

/// Load the current daemon identity from its production path.
pub fn load_current() -> Result<Option<Identity>> {
    load_from(&daemon_toml_path()?)
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn validate_name_table() {
        let rows: [(&str, std::result::Result<&str, NameError>); 7] = [
            ("run", Err(NameError::Reserved("run".into()))),
            ("queue", Err(NameError::Reserved("queue".into()))),
            ("forge", Err(NameError::Reserved("forge".into()))),
            ("anvil", Ok("anvil")),
            ("  Anvil ", Ok("anvil")),
            ("", Err(NameError::Empty)),
            ("has space", Err(NameError::InvalidChars)),
        ];
        for (raw, want) in rows {
            let want = want.map(str::to_owned);
            assert_eq!(validate_name(raw), want, "{raw:?}");
        }
    }

    #[test]
    fn suggest_name_strips_domain() {
        assert_eq!(suggest_name("MyBox.example.com"), "mybox");
        assert_eq!(suggest_name("run"), "ralphy", "reserved stem falls back");
        assert_eq!(suggest_name("...."), "ralphy", "empty stem falls back");
    }

    type Row<'a> = (
        &'a str,
        &'a str,
        Option<&'a str>,
        &'a [String],
        Option<&'a str>,
    );

    #[test]
    fn default_name_table() {
        let taken = vec!["vps-ralphy1".to_owned(), "VPS-ralphy1-2".to_owned()];
        let rows: [Row<'_>; 7] = [
            (
                "joined",
                "vps.example.com",
                Some("Ralphy1"),
                &[],
                Some("vps-ralphy1"),
            ),
            ("no user", "vps", None, &[], Some("vps")),
            ("empty user", "vps", Some(""), &[], Some("vps")),
            (
                "taken",
                "vps",
                Some("ralphy1"),
                &taken,
                Some("vps-ralphy1-3"),
            ),
            ("reserved", "run", None, &[], Some("run-2")),
            ("empty host", "", Some("ralphy1"), &[], None),
            ("dots host", "...", Some("ralphy1"), &[], None),
        ];
        for (name, host, user, taken, want) in rows {
            let want = want.map(str::to_owned);
            assert_eq!(default_name(host, user, taken), want, "{name}");
        }
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

    /// A `daemon.toml` as a baptized daemon has it on disk. The id is kept for
    /// life and names the daemon in the fleet, so a ulid update that changes how
    /// it parses or prints fails here instead of renaming a peer (#443). The
    /// u128 comes from a Crockford base32 decode in Python, not from the crate.
    #[test]
    fn a_daemon_toml_written_by_an_earlier_build_still_loads() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("daemon.toml");
        let stored = "id = \"01ARZ3NDEKTSV4RRFFQ69G5FAV\"\nname = \"anvil\"\navatar = \"🐙\"\n";
        std::fs::write(&path, stored).unwrap();

        let loaded = load_from(&path).unwrap().expect("a stored identity");
        assert_eq!(
            loaded.id.0,
            1_777_027_686_520_646_174_104_517_696_511_196_507
        );
        assert_eq!(loaded.id.timestamp_ms(), 1_469_922_850_259);
        assert_eq!(loaded.id.to_string(), "01ARZ3NDEKTSV4RRFFQ69G5FAV");

        let again = baptize(&path, "anvil".into(), "🐙".into()).unwrap();
        assert_eq!(again.id, loaded.id, "a re-baptism keeps the stored id");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), stored);
    }

    #[test]
    fn load_from_missing_is_none() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("absent.toml");
        assert_eq!(load_from(&path).unwrap(), None);
    }

    #[test]
    fn daemon_toml_path_under_global_store() {
        let dir = tempfile::tempdir().unwrap();
        let path = daemon_toml_path_for(None, Some(dir.path().into())).unwrap();
        assert_eq!(path, dir.path().join(".ralphy").join("daemon.toml"));
        let over = daemon_toml_path_for(Some("d".into()), Some(dir.path().into())).unwrap();
        assert_eq!(over, Path::new("d").join("daemon.toml"));
        assert!(daemon_toml_path_for(None, None).is_err());
    }
}
