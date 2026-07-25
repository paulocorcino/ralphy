//! The directory reader: classify every document in a repo's snapshot dir
//! (ADR-0047 §6/§7/§8).

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::document::{snapshot_dir, RunSnapshot, SNAPSHOT_VERSION};

/// A document the reader will not render as a run, and why. An unreadable run
/// is a fact, not an absence — it is reported, never silently dropped.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UnreadableRun {
    pub runid: String,
    pub reason: String,
}

impl UnreadableRun {
    fn new(runid: &str, reason: &str) -> Self {
        Self {
            runid: runid.into(),
            reason: reason.into(),
        }
    }
}

/// What one pass over a repo's snapshot directory found.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RunListing {
    pub live: Vec<RunSnapshot>,
    pub unreadable: Vec<UnreadableRun>,
}

/// List `repo_root`'s live runs, sweeping orphans as it goes.
///
/// `is_alive` is injected so tests never need a second process (the run lock's
/// pattern); production passes `ralphy_proc_util::pid_is_alive`. A document
/// whose pid is dead is an orphan: deleted and not reported. A malformed one,
/// or one written by a newer ralphy, is reported and LEFT on disk — deleting a
/// live newer run's document would be destructive.
///
/// An ABSENT directory is an empty listing (a repo that has never run is not an
/// error); any OTHER directory-read failure — a permission error, the path
/// being a file — is reported, because ADR-0047 §6 requires "no runs" and
/// "could not read runs" to stay distinguishable.
pub fn list_runs(repo_root: &Path, is_alive: impl Fn(u32) -> bool) -> RunListing {
    let mut listing = RunListing::default();
    let dir = snapshot_dir(repo_root);
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return listing,
        Err(e) => {
            listing
                .unreadable
                .push(UnreadableRun::new("runstate", &format!("unreadable: {e}")));
            return listing;
        }
    };
    for entry in entries {
        let path = match entry {
            Ok(entry) => entry.path(),
            Err(e) => {
                listing
                    .unreadable
                    .push(UnreadableRun::new("runstate", &format!("unreadable: {e}")));
                continue;
            }
        };
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_string();
        let raw = match std::fs::read_to_string(&path) {
            Ok(raw) => raw,
            Err(_) => {
                listing
                    .unreadable
                    .push(UnreadableRun::new(&name, "unreadable"));
                continue;
            }
        };
        // `v` is read before the struct so a document a newer ralphy retyped is
        // refused as such rather than misreported as malformed.
        let value: serde_json::Value = match serde_json::from_str(&raw) {
            Ok(value) => value,
            Err(_) => {
                listing
                    .unreadable
                    .push(UnreadableRun::new(&name, "malformed"));
                continue;
            }
        };
        match value.get("v").and_then(|v| v.as_u64()) {
            Some(v) if v > SNAPSHOT_VERSION as u64 => {
                listing
                    .unreadable
                    .push(UnreadableRun::new(&name, "unsupported version"));
                continue;
            }
            Some(_) => {}
            None => {
                listing
                    .unreadable
                    .push(UnreadableRun::new(&name, "malformed"));
                continue;
            }
        }
        let snap: RunSnapshot = match serde_json::from_value(value) {
            Ok(snap) => snap,
            Err(_) => {
                listing
                    .unreadable
                    .push(UnreadableRun::new(&name, "malformed"));
                continue;
            }
        };
        // `pid` is `#[serde(default)]`, so a document that omits it parses as 0
        // — and 0 is not a process: on Unix `kill(0, 0)` targets the CALLER's
        // process group and would classify it alive forever, so the orphan sweep
        // could never reclaim it. Refuse it before the predicate ever sees it.
        if snap.pid == 0 {
            listing
                .unreadable
                .push(UnreadableRun::new(&name, "malformed"));
            continue;
        }
        if is_alive(snap.pid) {
            listing.live.push(snap);
        } else {
            // Orphan sweep: a hard crash must not accumulate documents.
            let _ = std::fs::remove_file(&path);
        }
    }
    listing
        .live
        .sort_by(|a, b| (&a.started_at, &a.runid).cmp(&(&b.started_at, &b.runid)));
    listing
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::snapshot_path;
    use crate::write::write_atomic;

    fn seed(root: &Path, runid: &str, pid: u32, started_at: &str) {
        write_atomic(
            root,
            &RunSnapshot {
                runid: runid.into(),
                pid,
                started_at: started_at.into(),
                ..RunSnapshot::default()
            },
        )
        .unwrap();
    }

    fn seed_raw(root: &Path, name: &str, body: &str) {
        let dir = snapshot_dir(root);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(name), body).unwrap();
    }

    #[test]
    fn list_runs_lists_a_live_run() {
        let dir = tempfile::tempdir().unwrap();
        seed(
            dir.path(),
            "01LIVEB",
            4_000_002,
            "2026-07-24T11:00:00-03:00",
        );
        seed(
            dir.path(),
            "01LIVEA",
            4_000_002,
            "2026-07-24T10:00:00-03:00",
        );
        let listing = list_runs(dir.path(), |_| true);
        assert_eq!(
            listing
                .live
                .iter()
                .map(|s| s.runid.as_str())
                .collect::<Vec<_>>(),
            ["01LIVEA", "01LIVEB"],
            "live runs sort by started_at"
        );
        assert!(listing.unreadable.is_empty());
    }

    #[test]
    fn list_runs_drops_and_sweeps_a_dead_pid() {
        let dir = tempfile::tempdir().unwrap();
        seed(dir.path(), "01ALIVE", 111, "2026-07-24T10:00:00-03:00");
        seed(dir.path(), "01DEAD", 222, "2026-07-24T10:00:00-03:00");
        let listing = list_runs(dir.path(), |pid| pid == 111);
        assert_eq!(listing.live.len(), 1);
        assert_eq!(listing.live[0].runid, "01ALIVE");
        assert!(listing.unreadable.is_empty());
        assert!(!snapshot_path(dir.path(), "01DEAD").exists());
        assert!(snapshot_path(dir.path(), "01ALIVE").exists());
    }

    #[test]
    fn list_runs_reports_a_malformed_document() {
        let dir = tempfile::tempdir().unwrap();
        seed_raw(dir.path(), "bad.json", "not json");
        let listing = list_runs(dir.path(), |_| true);
        assert!(listing.live.is_empty());
        assert_eq!(listing.unreadable.len(), 1);
        assert_eq!(listing.unreadable[0].runid, "bad");
        assert_eq!(listing.unreadable[0].reason, "malformed");
        assert!(
            snapshot_dir(dir.path()).join("bad.json").exists(),
            "an unreadable document is reported, not deleted"
        );
    }

    #[test]
    fn list_runs_refuses_a_newer_version() {
        let dir = tempfile::tempdir().unwrap();
        seed_raw(
            dir.path(),
            "01FUTURE.json",
            &format!(r#"{{"v":{},"runid":"01FUTURE"}}"#, SNAPSHOT_VERSION + 1),
        );
        let listing = list_runs(dir.path(), |_| true);
        assert!(listing.live.is_empty());
        assert_eq!(listing.unreadable.len(), 1);
        assert_eq!(listing.unreadable[0].runid, "01FUTURE");
        assert_eq!(listing.unreadable[0].reason, "unsupported version");
        assert!(
            snapshot_dir(dir.path()).join("01FUTURE.json").exists(),
            "a newer ralphy's live run must not have its document deleted"
        );
    }

    #[test]
    fn list_runs_reports_a_document_with_no_version() {
        // Valid JSON, no `v`: the version pre-check must classify it, since
        // `RunSnapshot::v` has no serde default to fall back on.
        let dir = tempfile::tempdir().unwrap();
        seed_raw(dir.path(), "01NOVERSION.json", r#"{"runid":"01NOVERSION"}"#);
        let listing = list_runs(dir.path(), |_| true);
        assert!(listing.live.is_empty());
        assert_eq!(listing.unreadable[0].reason, "malformed");
        assert!(snapshot_dir(dir.path()).join("01NOVERSION.json").exists());
    }

    #[test]
    fn list_runs_refuses_a_zero_pid_instead_of_classifying_it() {
        // A document omitting `pid` parses as 0. It must never reach the
        // liveness predicate (see the note at the call site).
        let dir = tempfile::tempdir().unwrap();
        seed_raw(dir.path(), "01NOPID.json", r#"{"v":1,"runid":"01NOPID"}"#);
        let listing = list_runs(dir.path(), |pid| {
            panic!("pid {pid} must not reach the liveness predicate")
        });
        assert!(listing.live.is_empty());
        assert_eq!(listing.unreadable[0].reason, "malformed");
    }

    #[test]
    fn list_runs_reports_an_unreadable_directory() {
        // `.ralphy/runstate` present but NOT a directory: "could not read runs"
        // must stay distinguishable from "no runs" (ADR-0047 §6).
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".ralphy")).unwrap();
        std::fs::write(snapshot_dir(dir.path()), b"not a directory").unwrap();
        let listing = list_runs(dir.path(), |_| true);
        assert!(listing.live.is_empty());
        assert_eq!(listing.unreadable.len(), 1, "{:?}", listing.unreadable);
        assert!(
            listing.unreadable[0].reason.starts_with("unreadable"),
            "{:?}",
            listing.unreadable[0]
        );
    }

    #[test]
    fn list_runs_on_missing_dir_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        let listing = list_runs(dir.path(), |_| true);
        assert!(listing.live.is_empty());
        assert!(listing.unreadable.is_empty());
    }

    #[test]
    fn list_runs_ignores_non_json_entries() {
        let dir = tempfile::tempdir().unwrap();
        seed(dir.path(), "01LIVE", 111, "2026-07-24T10:00:00-03:00");
        seed_raw(dir.path(), "01LIVE.4242.tmp", "half a docum");
        let listing = list_runs(dir.path(), |_| true);
        assert_eq!(listing.live.len(), 1);
        assert!(listing.unreadable.is_empty());
    }
}
