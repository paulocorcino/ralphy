//! `daemon.pid` — which process is serving, so `ralphy daemon restart` can end
//! it (ADR-0056 §8).
//!
//! Pure, sync and path-explicit like every other store module: tests pass a temp
//! path and never mutate the process-global env. The file is advisory, never a
//! lock — a stale one names a pid that is simply not alive, and the reader
//! checks liveness rather than trusting the file.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// The `daemon.pid` path inside `dir`.
pub fn pid_path_in(dir: &Path) -> PathBuf {
    dir.join("daemon.pid")
}

/// The production path of `daemon.pid`, alongside the rest of the daemon store.
pub fn pid_path() -> Result<PathBuf> {
    Ok(pid_path_in(&crate::auth::store_dir()?))
}

/// Record this process as the serving daemon.
pub fn write_in(dir: &Path, pid: u32) -> Result<()> {
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let path = pid_path_in(dir);
    std::fs::write(&path, format!("{pid}\n")).with_context(|| format!("writing {}", path.display()))
}

/// Forget the recorded pid. Absent is the same as removed — an already-clean
/// store is not an error.
pub fn clear_in(dir: &Path) {
    let path = pid_path_in(dir);
    if let Err(e) = std::fs::remove_file(&path) {
        if e.kind() != std::io::ErrorKind::NotFound {
            tracing::warn!(path = %path.display(), error = %e, "could not clear the daemon pid file");
        }
    }
}

/// The pid a daemon last recorded, or `None` when there is no readable one.
/// Nothing here decides whether it is alive; that is the caller's syscall.
pub fn read_in(dir: &Path) -> Option<u32> {
    let text = std::fs::read_to_string(pid_path_in(dir)).ok()?;
    text.trim().parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ralphy-pidfile-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn a_recorded_pid_reads_back() {
        let dir = scratch("roundtrip");
        write_in(&dir, 4242).expect("write");
        assert_eq!(read_in(&dir), Some(4242));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_absent_or_unreadable_file_is_no_pid() {
        let dir = scratch("absent");
        assert_eq!(read_in(&dir), None);
        std::fs::create_dir_all(&dir).expect("dir");
        std::fs::write(pid_path_in(&dir), "not a pid\n").expect("write");
        assert_eq!(read_in(&dir), None, "a corrupt file names nobody");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn clearing_is_idempotent() {
        let dir = scratch("clear");
        write_in(&dir, 7).expect("write");
        clear_in(&dir);
        clear_in(&dir);
        assert_eq!(read_in(&dir), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
