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

/// Why a [`WatchSubs::wait`] came back. Diagnostic only — it never reaches the
/// wire; the poll route logs it, because from outside every empty answer looks
/// the same and attributing a hot poll loop otherwise needs a packet capture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaitOutcome {
    /// At least one dir this subscription watches changed.
    Dirty,
    /// The deadline passed with nothing for this subscription.
    Timeout,
    /// The repo's watcher was torn down mid-wait.
    Closed,
    /// The subscription held no receiver — no watchable dir, or another wait has
    /// it. Waited out the deadline anyway.
    NoReceiver,
    /// No such subscription. Waited out the deadline anyway.
    NoSub,
}

impl WaitOutcome {
    /// The label for a log line.
    pub fn as_str(self) -> &'static str {
        match self {
            WaitOutcome::Dirty => "dirty",
            WaitOutcome::Timeout => "timeout",
            WaitOutcome::Closed => "closed",
            WaitOutcome::NoReceiver => "no-receiver",
            WaitOutcome::NoSub => "no-sub",
        }
    }
}

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

    /// Hold `paths` for `sub_id`, creating the subscription if it is new.
    ///
    /// A path that cannot be watched — almost always a directory that has since
    /// been deleted or renamed — is SKIPPED, not fatal. Refusing the whole set for
    /// one stale entry took the whole subscription down: the caller got no
    /// `tree.dirty` for the dirs that were still there, so the tree quietly
    /// stopped updating, and the route answered its long poll in milliseconds,
    /// which on the peer transport is a connection per millisecond. The skipped
    /// path is left unheld, so a dir that comes back is picked up by the next
    /// poll. Only a path escaping the root and a subscription id that changes repo
    /// stay refusals — those are caller bugs, not the world moving.
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
                match self.watchers.watch(repo, root, &path) {
                    Ok(rx) => {
                        if sub.rx.is_none() {
                            sub.rx = Some(rx);
                        }
                        sub.paths.insert(path);
                    }
                    Err(error) => skipped(repo, &path, &error),
                }
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
                Err(error) => skipped(repo, &path, &error),
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

    /// Wait up to `timeout` for a change on one of THIS subscription's dirs.
    ///
    /// NEVER answers early empty-handed. A nudge for a dir this subscription does
    /// not hold is skipped and the wait CONTINUES to its deadline; so does a
    /// subscription with no receiver to wait on. That is the whole contract, and
    /// it is load-bearing rather than tidy: the caller is a long poll whose
    /// transport pays one TCP connection per round trip (ADR-0052 §2), so every
    /// early empty answer costs the polling host an ephemeral port for the length
    /// of its `TIME_WAIT`. The broadcast is per-REPO and carries every watched
    /// dir, so a root subscription on an active repo used to be woken — and
    /// answered empty — about once a second by `.ralphy/runstate` writes it never
    /// watched. That drained a Windows host's 16k port pool against its WSL peer
    /// until the peer was simply unreachable and the file tree went blank
    /// (2026-09-01).
    ///
    /// The returned [`WaitOutcome`] is why it came back, for the caller's log —
    /// from outside, a fast empty answer and a settled one look identical, which
    /// is what made that triage reach for `tcpdump`.
    pub async fn wait(
        &self,
        sub_id: &str,
        timeout: Duration,
    ) -> (Vec<(String, String)>, WaitOutcome) {
        let deadline = Instant::now() + timeout;
        // The guard is dropped with this block: nothing here may be held across
        // the awaits below.
        let taken = {
            let mut subs = self.lock();
            subs.get_mut(sub_id).map(|sub| {
                sub.last_poll = Instant::now();
                (sub.rx.take(), sub.paths.clone())
            })
        };
        let (mut rx, paths) = match taken {
            Some((Some(rx), paths)) => (rx, paths),
            // No receiver to wait on — the subscription holds no watchable dir, or
            // another in-flight wait has the receiver. Either way there is nothing
            // to deliver, which is exactly the case that must not answer fast.
            Some((None, _)) => {
                tokio::time::sleep_until(deadline.into()).await;
                return (Vec::new(), WaitOutcome::NoReceiver);
            }
            None => {
                tokio::time::sleep_until(deadline.into()).await;
                return (Vec::new(), WaitOutcome::NoSub);
            }
        };

        let mut dirty = drain(&mut rx, &paths);
        let mut outcome = WaitOutcome::Timeout;
        while dirty.is_empty() {
            let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
                break;
            };
            match tokio::time::timeout(remaining, rx.recv()).await {
                Ok(Ok(item)) => {
                    if paths.contains(&item.1) {
                        dirty.push(item);
                        dirty.extend(drain(&mut rx, &paths));
                    }
                    // Not ours: loop. This is the skip the doc comment is about.
                }
                // The backlog overflowed, so a nudge of ours may have been among
                // the dropped ones — re-drain and keep waiting if it was not.
                Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(_))) => {
                    dirty.extend(drain(&mut rx, &paths));
                }
                // The repo's watcher was torn down; nothing more can arrive, and
                // sleeping out the deadline would only delay the caller's
                // re-subscribe (which rebuilds the watcher).
                Ok(Err(tokio::sync::broadcast::error::RecvError::Closed)) => {
                    outcome = WaitOutcome::Closed;
                    break;
                }
                Err(_) => break,
            }
        }
        if !dirty.is_empty() {
            outcome = WaitOutcome::Dirty;
        }

        let mut subs = self.lock();
        if let Some(sub) = subs.get_mut(sub_id) {
            sub.last_poll = Instant::now();
            if sub.rx.is_none() {
                sub.rx = Some(rx);
            }
        }
        (dirty, outcome)
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

/// One watch that could not be armed. Logged rather than propagated (see
/// [`WatchSubs::subscribe`]) — and logged at WARN, because a dir the browser
/// believes it is watching that nobody is watching is worth a line.
fn skipped(repo: &str, path: &str, error: &anyhow::Error) {
    tracing::warn!(
        repo = %repo,
        path = %path,
        error = %format!("{error:#}"),
        "skipping a watch this subscription cannot arm; the rest of the set still holds"
    );
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
