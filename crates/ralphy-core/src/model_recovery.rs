//! Persisted read-time model recovery map (ADR-0053).

use std::collections::BTreeMap;
use std::fs::OpenOptions;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
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
        let _lock = MapLock::acquire(path)?;
        let previous = Self::load(path)?;
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
    path: PathBuf,
}

impl MapLock {
    fn acquire(map_path: &Path) -> Result<Self> {
        const WAIT: Duration = Duration::from_secs(5);
        const STALE: Duration = Duration::from_secs(30);
        let path = map_path.with_extension("json.lock");
        let parent = path.parent().unwrap_or_else(|| Path::new("."));
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating model recovery directory {}", parent.display()))?;
        let started = Instant::now();
        loop {
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(_) => return Ok(Self { path }),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    let stale = std::fs::metadata(&path)
                        .and_then(|metadata| metadata.modified())
                        .and_then(|modified| modified.elapsed().map_err(std::io::Error::other))
                        .is_ok_and(|age| age >= STALE);
                    if stale {
                        match std::fs::remove_file(&path) {
                            Ok(()) => continue,
                            Err(remove_error)
                                if remove_error.kind() == std::io::ErrorKind::NotFound =>
                            {
                                continue;
                            }
                            Err(_) => {}
                        }
                    }
                    if started.elapsed() >= WAIT {
                        bail!(
                            "timed out waiting for model recovery map lock {}",
                            path.display()
                        );
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(error) => {
                    return Err(error).with_context(|| {
                        format!("creating model recovery lock {}", path.display())
                    });
                }
            }
        }
    }
}

impl Drop for MapLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
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

    #[test]
    fn concurrent_locked_merges_preserve_every_fact() {
        let dir = tempfile::tempdir().unwrap();
        let path = session_model_map_path(dir.path());
        let threads: Vec<_> = (0..8)
            .map(|index| {
                let path = path.clone();
                std::thread::spawn(move || {
                    SessionModelMap::merge_persist_locked(
                        &path,
                        [(format!("session-{index}"), format!("model-{index}"))],
                    )
                    .unwrap();
                })
            })
            .collect();
        for thread in threads {
            thread.join().unwrap();
        }
        let map = SessionModelMap::load(&path).unwrap();
        assert_eq!(map.entries().len(), 8);
        for index in 0..8 {
            assert_eq!(
                map.get(&format!("session-{index}")),
                Some(format!("model-{index}").as_str())
            );
        }
    }
}
