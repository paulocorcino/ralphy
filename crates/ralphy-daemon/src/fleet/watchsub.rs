//! Buffered peer-side subscriptions for long-polled tree watches.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use anyhow::{bail, Result};

use crate::watch;

#[cfg(test)]
mod tests;

pub const IDLE_EXPIRY: Duration = Duration::from_secs(90);

pub struct WatchSubs {
    subs: Mutex<BTreeMap<String, Sub>>,
    watchers: Arc<watch::WatcherManager>,
}

struct Sub {
    repo: String,
    paths: BTreeSet<String>,
    rx: Option<watch::DirtyRx>,
    last_poll: Instant,
    watchers: Arc<watch::WatcherManager>,
}

impl Drop for Sub {
    fn drop(&mut self) {
        for rel in &self.paths {
            self.watchers.unwatch(&self.repo, rel);
        }
    }
}

impl WatchSubs {
    pub fn new(watchers: Arc<watch::WatcherManager>) -> Self {
        Self {
            subs: Mutex::new(BTreeMap::new()),
            watchers,
        }
    }

    fn lock(&self) -> MutexGuard<'_, BTreeMap<String, Sub>> {
        self.subs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub fn subscribe(&self, sub_id: &str, repo: &str, root: &Path, paths: &[String]) -> Result<()> {
        let normalized: BTreeSet<String> = paths.iter().map(|path| watch::norm_rel(path)).collect();
        if normalized
            .iter()
            .any(|path| path.split('/').any(|part| part == ".."))
        {
            bail!("watch path escapes the repo root");
        }

        let mut subs = self.lock();
        if let Some(sub) = subs.get_mut(sub_id) {
            if sub.repo != repo {
                bail!("subscription id already names another repo");
            }
            let added: Vec<String> = normalized.difference(&sub.paths).cloned().collect();
            for path in added {
                let rx = self.watchers.watch(repo, root, &path)?;
                if sub.rx.is_none() {
                    sub.rx = Some(rx);
                }
                sub.paths.insert(path);
            }
            sub.last_poll = Instant::now();
            return Ok(());
        }

        let mut held = BTreeSet::new();
        let mut rx = None;
        for path in normalized {
            match self.watchers.watch(repo, root, &path) {
                Ok(new_rx) => {
                    if rx.is_none() {
                        rx = Some(new_rx);
                    }
                    held.insert(path);
                }
                Err(error) => {
                    for path in &held {
                        self.watchers.unwatch(repo, path);
                    }
                    return Err(error);
                }
            }
        }
        subs.insert(
            sub_id.to_string(),
            Sub {
                repo: repo.to_string(),
                paths: held,
                rx,
                last_poll: Instant::now(),
                watchers: self.watchers.clone(),
            },
        );
        Ok(())
    }

    pub fn take_buffered(&self, sub_id: &str) -> Vec<(String, String)> {
        let mut subs = self.lock();
        let Some(sub) = subs.get_mut(sub_id) else {
            return Vec::new();
        };
        sub.last_poll = Instant::now();
        let Some(rx) = sub.rx.as_mut() else {
            return Vec::new();
        };
        drain(rx, &sub.paths)
    }

    pub async fn wait(&self, sub_id: &str, timeout: Duration) -> Vec<(String, String)> {
        let (mut rx, paths) = {
            let mut subs = self.lock();
            let Some(sub) = subs.get_mut(sub_id) else {
                return Vec::new();
            };
            sub.last_poll = Instant::now();
            let Some(rx) = sub.rx.take() else {
                return Vec::new();
            };
            (rx, sub.paths.clone())
        };

        let mut dirty = drain(&mut rx, &paths);
        if dirty.is_empty() {
            if let Ok(Ok(item)) = tokio::time::timeout(timeout, rx.recv()).await {
                if paths.contains(&item.1) {
                    dirty.push(item);
                }
                dirty.extend(drain(&mut rx, &paths));
            }
        }

        let mut subs = self.lock();
        if let Some(sub) = subs.get_mut(sub_id) {
            sub.last_poll = Instant::now();
            if sub.rx.is_none() {
                sub.rx = Some(rx);
            }
        }
        dirty
    }

    pub fn close(&self, sub_id: &str) {
        self.lock().remove(sub_id);
    }

    pub fn sweep(&self, idle: Duration) {
        let now = Instant::now();
        self.lock()
            .retain(|_, sub| now.duration_since(sub.last_poll) <= idle);
    }
}

fn drain(rx: &mut watch::DirtyRx, paths: &BTreeSet<String>) -> Vec<(String, String)> {
    let mut dirty = Vec::new();
    loop {
        match rx.try_recv() {
            Ok(item) if paths.contains(&item.1) => dirty.push(item),
            Ok(_) => {}
            Err(tokio::sync::broadcast::error::TryRecvError::Lagged(_)) => continue,
            Err(_) => break,
        }
    }
    dirty
}
