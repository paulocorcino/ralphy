//! Presence lockfile for `ralphy run` (`.ralphy/run.lock`).
//!
//! The concurrency policy lives in the *invocation*, not the repo: every run
//! writes this lock for its lifetime as a signal, never a mutex — manual runs
//! never block each other. `--if-idle` (a scheduler's invocation) defers to a
//! live lock and exits 0; without the flag a live lock only produces a warning.
//!
//! Release is RAII ([`RunLockGuard`]'s `Drop`), which covers every normal and
//! `?`-propagated exit of `run_cmd`. There is deliberately no Ctrl-C/signal
//! handler: a killed process leaves the file behind, and stale-PID takeover is
//! the recovery mechanism — a crash or reboot never silences a schedule.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// The lock's payload: who holds it and since when (RFC 3339, local offset).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LockInfo {
    pub pid: u32,
    pub started_at: String,
}

/// What a lockfile inspection found.
#[derive(Debug)]
pub enum LockState {
    /// No lockfile.
    Free,
    /// Lockfile present and its PID is a living process (not our own).
    HeldAlive(LockInfo),
    /// Lockfile present but its PID is dead — take over.
    Stale(LockInfo),
    /// Lockfile present but unparseable — treated like stale.
    Corrupt,
}

/// Read and classify the lockfile. Liveness is injected so tests never need a
/// real second process (same pattern as `check_agents_present` in main.rs).
pub fn inspect(path: &Path, is_alive: impl Fn(u32) -> bool) -> LockState {
    let raw = match fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(_) => return LockState::Free,
    };
    let info: LockInfo = match serde_json::from_str(&raw) {
        Ok(info) => info,
        Err(_) => return LockState::Corrupt,
    };
    // Our own PID in the file can only be a leftover from PID reuse after a
    // crash — the current process hasn't acquired yet.
    if info.pid != std::process::id() && is_alive(info.pid) {
        LockState::HeldAlive(info)
    } else {
        LockState::Stale(info)
    }
}

/// Write the lock (over anything stale or corrupt) and return the guard that
/// removes it on drop.
pub fn acquire(path: &Path) -> Result<RunLockGuard> {
    let info = LockInfo {
        pid: std::process::id(),
        started_at: chrono::Local::now().to_rfc3339(),
    };
    let payload = serde_json::to_string(&info)?;
    fs::write(path, payload)
        .with_context(|| format!("could not write run lock {}", path.display()))?;
    Ok(RunLockGuard {
        path: path.to_path_buf(),
    })
}

/// Removes the lockfile when dropped. Failures only warn — never panic in
/// Drop, and a leftover file is recovered by stale takeover anyway.
pub struct RunLockGuard {
    path: PathBuf,
}

impl Drop for RunLockGuard {
    fn drop(&mut self) {
        if let Err(e) = fs::remove_file(&self.path) {
            if e.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!(error = %e, path = %self.path.display(), "could not remove run.lock");
            }
        }
    }
}

/// Production liveness predicate for [`inspect`].
#[cfg(unix)]
pub fn pid_is_alive(pid: u32) -> bool {
    // Signal 0 probes without sending: 0 = alive, EPERM = alive but not ours.
    let r = unsafe { libc::kill(pid as libc::pid_t, 0) };
    r == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

/// Production liveness predicate for [`inspect`].
#[cfg(windows)]
pub fn pid_is_alive(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, ERROR_ACCESS_DENIED, STILL_ACTIVE};
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            // A process we can see but not open is still a live process —
            // conservative: never take over a lock we can't inspect.
            return std::io::Error::last_os_error().raw_os_error()
                == Some(ERROR_ACCESS_DENIED as i32);
        }
        // An exited process can still be opened while a handle to it is held
        // elsewhere; STILL_ACTIVE separates the two.
        let mut code: u32 = 0;
        let alive = GetExitCodeProcess(handle, &mut code) != 0 && code == STILL_ACTIVE as u32;
        CloseHandle(handle);
        alive
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// Hand-rolled unique temp dir (same idiom as `tmp_ws` in config.rs).
    fn tmp_lock(name: &str) -> PathBuf {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "ralphy-runlock-{}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed),
            name
        ));
        fs::create_dir_all(&dir).unwrap();
        dir.join("run.lock")
    }

    #[test]
    fn acquire_writes_lock_with_pid_and_time() {
        let path = tmp_lock("acquire");
        let _guard = acquire(&path).unwrap();
        let info: LockInfo = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(info.pid, std::process::id());
        assert!(chrono::DateTime::parse_from_rfc3339(&info.started_at).is_ok());
    }

    #[test]
    fn guard_drop_removes_lock() {
        let path = tmp_lock("drop");
        let guard = acquire(&path).unwrap();
        assert!(path.exists());
        drop(guard);
        assert!(!path.exists());
    }

    #[test]
    fn inspect_free_when_absent() {
        let path = tmp_lock("absent");
        assert!(matches!(inspect(&path, |_| true), LockState::Free));
    }

    #[test]
    fn inspect_held_when_pid_alive() {
        let path = tmp_lock("held");
        let stored = LockInfo {
            pid: 4_000_000, // never our own PID (Windows PIDs stay far below)
            started_at: "2026-07-02T10:00:00-03:00".into(),
        };
        fs::write(&path, serde_json::to_string(&stored).unwrap()).unwrap();
        match inspect(&path, |pid| pid == 4_000_000) {
            LockState::HeldAlive(info) => {
                assert_eq!(info.pid, 4_000_000);
                assert_eq!(info.started_at, stored.started_at);
            }
            other => panic!("expected HeldAlive, got {other:?}"),
        }
    }

    #[test]
    fn inspect_stale_when_pid_dead_and_takeover_succeeds() {
        let path = tmp_lock("stale");
        let stored = LockInfo {
            pid: 4_000_001,
            started_at: "2026-07-02T10:00:00-03:00".into(),
        };
        fs::write(&path, serde_json::to_string(&stored).unwrap()).unwrap();
        assert!(matches!(
            inspect(&path, |_| false),
            LockState::Stale(LockInfo { pid: 4_000_001, .. })
        ));
        // Takeover: acquiring over the stale lock rewrites it with our PID.
        let _guard = acquire(&path).unwrap();
        let info: LockInfo = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(info.pid, std::process::id());
    }

    #[test]
    fn inspect_corrupt_on_garbage_and_takeover_succeeds() {
        let path = tmp_lock("corrupt");
        fs::write(&path, "not json").unwrap();
        assert!(matches!(inspect(&path, |_| true), LockState::Corrupt));
        let _guard = acquire(&path).unwrap();
        assert!(matches!(inspect(&path, |_| false), LockState::Stale(_)));
    }

    #[test]
    fn own_pid_treated_as_stale() {
        let path = tmp_lock("ownpid");
        let stored = LockInfo {
            pid: std::process::id(),
            started_at: "2026-07-02T10:00:00-03:00".into(),
        };
        fs::write(&path, serde_json::to_string(&stored).unwrap()).unwrap();
        // Even with an always-alive predicate, our own PID is a leftover.
        assert!(matches!(inspect(&path, |_| true), LockState::Stale(_)));
    }

    #[test]
    fn pid_is_alive_detects_own_process() {
        assert!(pid_is_alive(std::process::id()));
    }
}
