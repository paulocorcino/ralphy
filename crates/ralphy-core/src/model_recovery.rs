//! Persisted read-time model recovery map (ADR-0053).

use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

pub const SESSION_MODELS_FILE: &str = "session-models.json";

/// Durable append-only `session_id -> model` facts.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SessionModelMap(BTreeMap<String, String>);

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MergeReport {
    pub added: usize,
    pub conflicts: Vec<ModelConflict>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelConflict {
    pub session_id: String,
    pub stored_model: String,
    pub proposed_model: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockedMerge {
    pub previous: SessionModelMap,
    pub current: SessionModelMap,
    pub report: MergeReport,
}

impl SessionModelMap {
    /// Load a map. A missing file is an empty map; malformed content is an error.
    pub fn load(path: &Path) -> Result<Self> {
        match std::fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .with_context(|| format!("parsing model recovery map {}", path.display())),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => {
                Err(error).with_context(|| format!("reading model recovery map {}", path.display()))
            }
        }
    }

    #[must_use]
    pub fn get(&self, session_id: &str) -> Option<&str> {
        self.0.get(session_id).map(String::as_str)
    }

    #[must_use]
    pub fn entries(&self) -> &BTreeMap<String, String> {
        &self.0
    }

    /// Merge new facts without ever replacing a stored model.
    pub fn merge<I>(&mut self, pairs: I) -> MergeReport
    where
        I: IntoIterator<Item = (String, String)>,
    {
        let mut report = MergeReport::default();
        for (session_id, proposed_model) in pairs {
            match self.0.get(&session_id) {
                None => {
                    self.0.insert(session_id, proposed_model);
                    report.added += 1;
                }
                Some(stored_model) if stored_model != &proposed_model => {
                    report.conflicts.push(ModelConflict {
                        session_id,
                        stored_model: stored_model.clone(),
                        proposed_model,
                    });
                }
                Some(_) => {}
            }
        }
        report
    }

    /// Persist through a same-directory temp file and atomic rename.
    pub fn persist(&self, path: &Path) -> Result<()> {
        self.persist_with(path, |from, to| std::fs::rename(from, to))
    }

    /// Serialize the load-merge-persist transaction across processes.
    pub fn merge_persist_locked<I>(path: &Path, pairs: I) -> Result<LockedMerge>
    where
        I: IntoIterator<Item = (String, String)>,
    {
        Self::merge_persist_locked_with(path, pairs, || Ok(()))
    }

    fn merge_persist_locked_with<I, F>(path: &Path, pairs: I, after_load: F) -> Result<LockedMerge>
    where
        I: IntoIterator<Item = (String, String)>,
        F: FnOnce() -> Result<()>,
    {
        let _lock = MapLock::acquire(path)?;
        let previous = Self::load(path)?;
        after_load()?;
        let mut current = previous.clone();
        let report = current.merge(pairs);
        if report.added > 0 {
            current.persist(path)?;
        }
        Ok(LockedMerge {
            previous,
            current,
            report,
        })
    }

    fn persist_with<F>(&self, path: &Path, rename: F) -> Result<()>
    where
        F: FnOnce(&Path, &Path) -> std::io::Result<()>,
    {
        let parent = path.parent().unwrap_or_else(|| Path::new("."));
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating model recovery directory {}", parent.display()))?;
        let file_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(SESSION_MODELS_FILE);
        let temp = parent.join(format!("{file_name}.{}.tmp", std::process::id()));
        let bytes = serde_json::to_vec_pretty(self).context("serializing model recovery map")?;
        std::fs::write(&temp, bytes)
            .with_context(|| format!("writing model recovery temp file {}", temp.display()))?;
        if let Err(error) = rename(&temp, path) {
            let _ = std::fs::remove_file(&temp);
            return Err(error)
                .with_context(|| format!("replacing model recovery map {}", path.display()));
        }
        Ok(())
    }
}

struct MapLock {
    file: File,
}

impl MapLock {
    fn acquire(map_path: &Path) -> Result<Self> {
        Ok(Self {
            file: Self::acquire_with(map_path, try_lock_exclusive, lock_exclusive)?,
        })
    }

    fn acquire_with<T, B>(map_path: &Path, try_lock: T, block: B) -> Result<File>
    where
        T: FnOnce(&File) -> std::io::Result<bool>,
        B: FnOnce(&File) -> std::io::Result<()>,
    {
        let file = Self::open(map_path)?;
        if !try_lock(&file)
            .with_context(|| format!("trying model recovery map lock {}", map_path.display()))?
        {
            #[cfg(test)]
            if let Some(marker) = std::env::var_os("RALPHY_MODEL_LOCK_CONTENDED_MARKER") {
                std::fs::write(marker, b"contended")
                    .context("writing model recovery contention marker")?;
            }
            block(&file)
                .with_context(|| format!("locking model recovery map {}", map_path.display()))?;
        }
        Ok(file)
    }

    fn open(map_path: &Path) -> Result<File> {
        let path = map_path.with_extension("json.lock");
        let parent = path.parent().unwrap_or_else(|| Path::new("."));
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating model recovery directory {}", parent.display()))?;
        OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .with_context(|| format!("opening model recovery lock {}", path.display()))
    }
}

impl Drop for MapLock {
    fn drop(&mut self) {
        let _ = unlock(&self.file);
    }
}

#[cfg(unix)]
fn lock_exclusive(file: &File) -> std::io::Result<()> {
    use std::os::fd::AsRawFd;

    const LOCK_EX: i32 = 2;
    unsafe extern "C" {
        fn flock(fd: i32, operation: i32) -> i32;
    }
    // SAFETY: `file` owns a valid descriptor for the duration of this call.
    let result = unsafe { flock(file.as_raw_fd(), LOCK_EX) };
    if result == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(unix)]
fn try_lock_exclusive(file: &File) -> std::io::Result<bool> {
    use std::os::fd::AsRawFd;

    const LOCK_EX: i32 = 2;
    const LOCK_NB: i32 = 4;
    unsafe extern "C" {
        fn flock(fd: i32, operation: i32) -> i32;
    }
    // SAFETY: `file` owns a valid descriptor for the duration of this call.
    let result = unsafe { flock(file.as_raw_fd(), LOCK_EX | LOCK_NB) };
    if result == 0 {
        Ok(true)
    } else {
        let error = std::io::Error::last_os_error();
        if error.kind() == std::io::ErrorKind::WouldBlock {
            Ok(false)
        } else {
            Err(error)
        }
    }
}

#[cfg(unix)]
fn unlock(file: &File) -> std::io::Result<()> {
    use std::os::fd::AsRawFd;

    const LOCK_UN: i32 = 8;
    unsafe extern "C" {
        fn flock(fd: i32, operation: i32) -> i32;
    }
    // SAFETY: `file` owns a valid descriptor for the duration of this call.
    let result = unsafe { flock(file.as_raw_fd(), LOCK_UN) };
    if result == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(windows)]
fn lock_exclusive(file: &File) -> std::io::Result<()> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{LockFileEx, LOCKFILE_EXCLUSIVE_LOCK};
    use windows_sys::Win32::System::IO::OVERLAPPED;

    // SAFETY: zero is the documented synchronous OVERLAPPED shape; the file
    // handle remains owned by `MapLock` until the matching unlock.
    let mut overlapped: OVERLAPPED = unsafe { std::mem::zeroed() };
    let result = unsafe {
        LockFileEx(
            file.as_raw_handle() as _,
            LOCKFILE_EXCLUSIVE_LOCK,
            0,
            u32::MAX,
            u32::MAX,
            &mut overlapped,
        )
    };
    if result != 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(windows)]
fn try_lock_exclusive(file: &File) -> std::io::Result<bool> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        LockFileEx, LOCKFILE_EXCLUSIVE_LOCK, LOCKFILE_FAIL_IMMEDIATELY,
    };
    use windows_sys::Win32::System::IO::OVERLAPPED;

    // SAFETY: zero is the documented synchronous OVERLAPPED shape; the handle
    // remains valid while the result is interpreted.
    let mut overlapped: OVERLAPPED = unsafe { std::mem::zeroed() };
    let result = unsafe {
        LockFileEx(
            file.as_raw_handle() as _,
            LOCKFILE_EXCLUSIVE_LOCK | LOCKFILE_FAIL_IMMEDIATELY,
            0,
            u32::MAX,
            u32::MAX,
            &mut overlapped,
        )
    };
    if result != 0 {
        Ok(true)
    } else {
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() == Some(33) {
            Ok(false)
        } else {
            Err(error)
        }
    }
}

#[cfg(windows)]
fn unlock(file: &File) -> std::io::Result<()> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::UnlockFileEx;
    use windows_sys::Win32::System::IO::OVERLAPPED;

    // SAFETY: this matches the range locked by `lock_exclusive`; the handle is valid.
    let mut overlapped: OVERLAPPED = unsafe { std::mem::zeroed() };
    let result = unsafe {
        UnlockFileEx(
            file.as_raw_handle() as _,
            0,
            u32::MAX,
            u32::MAX,
            &mut overlapped,
        )
    };
    if result != 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[must_use]
pub fn session_model_map_path(usage_root: &Path) -> PathBuf {
    usage_root.join(SESSION_MODELS_FILE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn map_round_trip_preserves_exact_pairs() {
        let dir = tempfile::tempdir().unwrap();
        let path = session_model_map_path(dir.path());
        let mut map = SessionModelMap::default();
        map.merge([
            ("session-b".into(), "model-b".into()),
            ("session-a".into(), "model-a".into()),
        ]);
        map.persist(&path).unwrap();
        assert_eq!(SessionModelMap::load(&path).unwrap(), map);
        map.merge([("session-c".into(), "model-c".into())]);
        map.persist(&path).unwrap();
        assert_eq!(SessionModelMap::load(&path).unwrap(), map);
        assert_eq!(map.entries().len(), 3);
    }

    #[test]
    fn merge_is_idempotent_and_old_value_wins_conflicts() {
        let mut map = SessionModelMap::default();
        assert_eq!(map.merge([("session-a".into(), "model-a".into())]).added, 1);
        assert_eq!(
            map.merge([
                ("session-a".into(), "model-a".into()),
                ("session-a".into(), "replacement".into()),
                ("session-b".into(), "model-b".into()),
            ]),
            MergeReport {
                added: 1,
                conflicts: vec![ModelConflict {
                    session_id: "session-a".into(),
                    stored_model: "model-a".into(),
                    proposed_model: "replacement".into(),
                }],
            }
        );
        assert_eq!(map.get("session-a"), Some("model-a"));
        assert_eq!(map.entries().len(), 2);
    }

    #[test]
    fn atomic_write_failure_preserves_previous_map_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let path = session_model_map_path(dir.path());
        let original = br#"{"session-a":"model-a"}"#;
        std::fs::write(&path, original).unwrap();
        let mut map = SessionModelMap::load(&path).unwrap();
        map.merge([("session-b".into(), "model-b".into())]);

        let error = map
            .persist_with(&path, |_, _| {
                Err(std::io::Error::other("injected rename failure"))
            })
            .unwrap_err();
        assert!(error.to_string().contains("replacing model recovery map"));
        assert_eq!(std::fs::read(&path).unwrap(), original);
        let leftovers: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .flatten()
            .filter(|entry| entry.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty());
    }

    fn wait_for(path: &Path) {
        let started = std::time::Instant::now();
        while !path.exists() {
            assert!(
                started.elapsed() < std::time::Duration::from_secs(10),
                "timed out waiting for {}",
                path.display()
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    #[test]
    fn locked_merge_child() {
        let Some(root) = std::env::var_os("RALPHY_MODEL_LOCK_CHILD_ROOT") else {
            return;
        };
        let session = std::env::var("RALPHY_MODEL_LOCK_CHILD_SESSION").unwrap();
        let root = PathBuf::from(root);
        if std::env::var_os("RALPHY_MODEL_LOCK_CHILD_TRY").is_some() {
            let file = MapLock::open(&session_model_map_path(&root)).unwrap();
            assert!(
                !try_lock_exclusive(&file).unwrap(),
                "nonblocking lock unexpectedly succeeded"
            );
            std::fs::write(root.join(format!("try-blocked-{session}")), b"blocked").unwrap();
            return;
        }
        let ready = root.join(format!("{session}.ready"));
        let release = root.join(format!("{session}.release"));
        SessionModelMap::merge_persist_locked_with(
            &session_model_map_path(&root),
            [(session.clone(), format!("model-{session}"))],
            || {
                std::fs::write(&ready, b"ready")?;
                wait_for(&release);
                Ok(())
            },
        )
        .unwrap();
    }

    #[test]
    fn separate_process_transactions_are_serialized() {
        let dir = tempfile::tempdir().unwrap();
        let path = session_model_map_path(dir.path());
        let spawn = |session: &str, try_only: bool, contended_marker: Option<&Path>| {
            let mut command = std::process::Command::new(std::env::current_exe().unwrap());
            command
                .args([
                    "--exact",
                    "model_recovery::tests::locked_merge_child",
                    "--nocapture",
                ])
                .env("RALPHY_MODEL_LOCK_CHILD_ROOT", dir.path())
                .env("RALPHY_MODEL_LOCK_CHILD_SESSION", session);
            if try_only {
                command.env("RALPHY_MODEL_LOCK_CHILD_TRY", "1");
            }
            if let Some(marker) = contended_marker {
                command.env("RALPHY_MODEL_LOCK_CONTENDED_MARKER", marker);
            }
            command.spawn().unwrap()
        };
        let ready_a = dir.path().join("session-a.ready");
        let ready_b = dir.path().join("session-b.ready");
        let contended_b = dir.path().join("session-b.contended");
        let mut first = spawn("session-a", false, None);
        wait_for(&ready_a);
        let mut probe_a = spawn("probe-a", true, None);
        assert!(probe_a.wait().unwrap().success());
        assert!(dir.path().join("try-blocked-probe-a").exists());
        let mut second = spawn("session-b", false, Some(&contended_b));
        wait_for(&contended_b);
        assert!(
            !ready_b.exists(),
            "contended child passed load before release"
        );
        std::fs::write(dir.path().join("session-a.release"), b"release").unwrap();
        assert!(first.wait().unwrap().success());
        wait_for(&ready_b);
        let mut probe_b = spawn("probe-b", true, None);
        assert!(probe_b.wait().unwrap().success());
        assert!(
            dir.path().join("try-blocked-probe-b").exists(),
            "blocking acquisition returned without owning the OS lock"
        );
        std::fs::write(dir.path().join("session-b.release"), b"release").unwrap();
        assert!(second.wait().unwrap().success());

        let map = SessionModelMap::load(&path).unwrap();
        assert_eq!(map.entries().len(), 2);
        assert_eq!(map.get("session-a"), Some("model-session-a"));
        assert_eq!(map.get("session-b"), Some("model-session-b"));
    }

    #[test]
    fn contended_acquire_always_calls_the_blocking_operation() {
        let dir = tempfile::tempdir().unwrap();
        let map_path = session_model_map_path(dir.path());
        let blocked = std::cell::Cell::new(false);

        let file = MapLock::acquire_with(
            &map_path,
            |_| Ok(false),
            |_| {
                blocked.set(true);
                Ok(())
            },
        )
        .unwrap();

        assert!(blocked.get());
        drop(file);
    }

    #[test]
    fn uncontended_acquire_never_calls_the_blocking_operation() {
        let dir = tempfile::tempdir().unwrap();
        let map_path = session_model_map_path(dir.path());
        let file = MapLock::acquire_with(
            &map_path,
            |_| Ok(true),
            |_| panic!("blocking operation called after successful try-lock"),
        )
        .unwrap();

        drop(file);
    }
}
