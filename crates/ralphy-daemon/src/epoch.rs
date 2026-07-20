//! The session epoch (ADR-0032 amendment §B): a monotonic counter mixed into
//! every session cookie's MAC. Bumping it changes the MAC of ALL outstanding
//! cookies at once, so they stop verifying — real server-side invalidation
//! (logout, token re-mint, TOTP revoke, disabling require-login) WITHOUT a
//! server-side session store. The counter is one integer, not a session table.
//!
//! Persisted in the global store (`daemon-session-epoch`, mode 0600) beside
//! `daemon-token`, so it survives a restart. The live value is an in-memory
//! atomic backed by that file: reads are lock-free (every cookie verify reads
//! it), a bump writes through to disk. Path-explicit like `auth`/`totp`: tests
//! pass a temp path and never touch the process-global env.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use anyhow::{Context, Result};

use crate::auth;

/// The `daemon-session-epoch` path inside `dir`.
pub fn epoch_path_in(dir: &Path) -> PathBuf {
    dir.join("daemon-session-epoch")
}

/// The production path of `daemon-session-epoch`. Mirrors [`auth::token_path`].
pub fn epoch_path() -> Result<PathBuf> {
    Ok(epoch_path_in(&auth::store_dir()?))
}

/// Read the persisted epoch, or `0` when the file is absent (never bumped). A
/// malformed file also reads as `0` — the only cost is that cookies signed under
/// a higher epoch stop verifying, which fails closed (re-login), never open.
pub fn load_epoch_from(path: &Path) -> Result<u64> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(text.trim().parse().unwrap_or(0)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(0),
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

/// Write `epoch` to `path` owner-only, creating the parent directory.
pub fn save_epoch_to(epoch: u64, path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    std::fs::write(path, epoch.to_string())
        .with_context(|| format!("writing {}", path.display()))?;
    auth::set_owner_only(path)?;
    Ok(())
}

/// A live, shared session epoch: an in-memory atomic backed by a store file.
/// Cheap to clone (an `Arc` + a `PathBuf`); every [`SessionAuth`](crate::auth::SessionAuth)
/// holds one and reads it on each cookie verify.
#[derive(Clone)]
pub struct SessionEpoch {
    current: Arc<AtomicU64>,
    path: PathBuf,
}

impl SessionEpoch {
    /// Load the epoch from `path` into a shared atomic (the boot path).
    pub fn load(path: PathBuf) -> Result<SessionEpoch> {
        let current = load_epoch_from(&path)?;
        Ok(SessionEpoch {
            current: Arc::new(AtomicU64::new(current)),
            path,
        })
    }

    /// The current epoch (lock-free read).
    pub fn get(&self) -> u64 {
        self.current.load(Ordering::Relaxed)
    }

    /// Increment the epoch and persist it, invalidating every outstanding cookie.
    /// Returns the new value. The atomic is bumped first (so concurrent verifies
    /// immediately reject old cookies) and then written through to disk.
    pub fn bump(&self) -> Result<u64> {
        let next = self.current.fetch_add(1, Ordering::Relaxed) + 1;
        save_epoch_to(next, &self.path)?;
        Ok(next)
    }

    /// A detached epoch starting at 0, backed by a unique throwaway temp path so
    /// a `bump` still writes somewhere harmless. For a `Localhost`/`fixed` auth
    /// state that has no real session store (tests, the frictionless default).
    pub fn in_memory_detached() -> SessionEpoch {
        let path = std::env::temp_dir().join(format!("ralphy-epoch-{}", ulid::Ulid::new()));
        SessionEpoch {
            current: Arc::new(AtomicU64::new(0)),
            path,
        }
    }

    /// A detached epoch for tests with an explicit start value and path.
    #[cfg(test)]
    pub fn in_memory(start: u64, path: PathBuf) -> SessionEpoch {
        SessionEpoch {
            current: Arc::new(AtomicU64::new(start)),
            path,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_epoch_from_missing_is_zero() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(load_epoch_from(&dir.path().join("absent")).unwrap(), 0);
    }

    #[test]
    fn save_then_load_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = epoch_path_in(dir.path());
        save_epoch_to(7, &path).unwrap();
        assert_eq!(load_epoch_from(&path).unwrap(), 7);
    }

    #[test]
    fn malformed_epoch_reads_as_zero() {
        let dir = tempfile::tempdir().unwrap();
        let path = epoch_path_in(dir.path());
        std::fs::write(&path, "not-a-number").unwrap();
        assert_eq!(load_epoch_from(&path).unwrap(), 0, "fails closed to 0");
    }

    #[test]
    fn bump_increments_and_persists() {
        let dir = tempfile::tempdir().unwrap();
        let path = epoch_path_in(dir.path());
        let epoch = SessionEpoch::load(path.clone()).unwrap();
        assert_eq!(epoch.get(), 0);
        assert_eq!(epoch.bump().unwrap(), 1);
        assert_eq!(epoch.get(), 1);
        // Persisted: a fresh load sees the bumped value.
        assert_eq!(load_epoch_from(&path).unwrap(), 1);
        assert_eq!(SessionEpoch::load(path).unwrap().get(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn saved_epoch_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = epoch_path_in(dir.path());
        save_epoch_to(3, &path).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "epoch file must be mode 0600");
    }
}
