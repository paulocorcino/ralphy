//! The desk store (ADR-0050): which consoles were open and where each window
//! sat, persisted as `desk.toml` beside `repos.toml` in the global daemon store.
//!
//! The desk is DAEMON state, not browser state — a workbench session survives
//! the browser, so its window must too. Modelled on `registry`: pure sync,
//! path-explicit, tests pass a temp path and never touch the process env.
//!
//! The record shape mirrors what the shell already writes (wb-console.js
//! `persistWin`), spelled `camelCase` on the wire and in the file so one
//! spelling holds end to end.

use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// A window's restore box, in absolute workspace pixels. No proportional or
/// per-resolution form: the shell's `clampAll` already refits a desk saved on a
/// larger monitor (ADR-0050).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeskRect {
    pub left: f64,
    pub top: f64,
    pub width: f64,
    pub height: f64,
}

/// One desk record: a window keyed by its STABLE client-side `id`. The daemon's
/// `session_id` is a volatile attribute (a restarted daemon hands out ids from 1
/// again), which is why it is nullable and never the key.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeskRecord {
    pub id: String,
    #[serde(default)]
    pub repo: String,
    #[serde(default)]
    pub agent: String,
    #[serde(default)]
    pub kind: String,
    pub rect: DeskRect,
    #[serde(default)]
    pub max: bool,
    #[serde(default)]
    pub session_id: Option<u64>,
    #[serde(default)]
    pub ts: i64,
}

/// The persisted desk: the records in LAYOUT order (the order decides which
/// record wins a contended session in the shell's `reconcileDesk`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DeskStore {
    #[serde(default)]
    pub windows: Vec<DeskRecord>,
}

/// The daemon-side cap on desk records. Enforced here rather than trusting the
/// uploaded array — a browser upload does not get to define the size.
pub const DESK_MAX: usize = 24;

/// Keep the [`DESK_MAX`] newest records by `ts`, PRESERVING layout order. Live
/// windows cannot be pinned here — the daemon does not know which windows are on
/// screen — so the shell pins them before uploading and this is the backstop.
pub fn prune(records: Vec<DeskRecord>) -> Vec<DeskRecord> {
    if records.len() <= DESK_MAX {
        return records;
    }
    let mut by_ts: Vec<usize> = (0..records.len()).collect();
    by_ts.sort_by(|&a, &b| records[b].ts.cmp(&records[a].ts));
    by_ts.truncate(DESK_MAX);
    let keep: std::collections::HashSet<usize> = by_ts.into_iter().collect();
    records
        .into_iter()
        .enumerate()
        .filter_map(|(i, r)| keep.contains(&i).then_some(r))
        .collect()
}

/// Load the desk from `path`. A missing file AND a corrupt one both read as an
/// empty desk — deliberately diverging from [`crate::registry::load_from`],
/// which returns a `Result`: an unreadable layout costs a cascaded stage, not a
/// daemon, so this must never give a caller a startup failure to propagate.
pub fn load_from(path: &Path) -> DeskStore {
    match std::fs::read_to_string(path) {
        Ok(text) => match toml::from_str(&text) {
            Ok(store) => store,
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "unreadable desk layout — starting from an empty desk");
                DeskStore::default()
            }
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => DeskStore::default(),
        Err(e) => {
            tracing::warn!(path = %path.display(), error = %e, "could not read desk layout — starting from an empty desk");
            DeskStore::default()
        }
    }
}

/// Write the desk to `path` owner-only, creating the parent directory.
pub fn save_to(store: &DeskStore, path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    let text = toml::to_string_pretty(store).context("serializing desk layout")?;
    std::fs::write(path, text).with_context(|| format!("writing {}", path.display()))?;
    crate::registry::set_owner_only(path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(id: &str, ts: i64) -> DeskRecord {
        DeskRecord {
            id: id.into(),
            repo: "owner/repo".into(),
            agent: "claude".into(),
            kind: "console".into(),
            rect: DeskRect {
                left: 10.0,
                top: 20.0,
                width: 640.0,
                height: 480.0,
            },
            max: false,
            session_id: Some(7),
            ts,
        }
    }

    #[test]
    fn round_trip_preserves_records() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("desk.toml");
        let mut a = record("w1", 1);
        a.session_id = None;
        let mut b = record("w2", 2);
        b.max = true;
        let store = DeskStore {
            windows: vec![a, b],
        };
        save_to(&store, &path).unwrap();

        let back = load_from(&path);
        assert_eq!(back, store, "the desk round-trips through desk.toml");
        assert_eq!(back.windows[0].session_id, None);
        assert!(back.windows[1].max);
    }

    #[test]
    fn wire_key_is_camel_case_session_id() {
        let json = serde_json::to_string(&record("w1", 3)).unwrap();
        assert!(
            json.contains("\"sessionId\":7"),
            "the shell writes `sessionId`; got {json}"
        );
    }

    #[test]
    fn missing_file_reads_as_empty_desk() {
        let dir = tempfile::tempdir().unwrap();
        let store = load_from(&dir.path().join("desk.toml"));
        assert!(store.windows.is_empty());
    }

    #[test]
    fn corrupt_file_reads_as_empty_desk() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("desk.toml");
        std::fs::write(&path, "not a toml { ][").unwrap();
        let store = load_from(&path);
        assert!(
            store.windows.is_empty(),
            "a corrupt desk reads empty and does not panic"
        );
    }

    #[test]
    fn prune_keeps_24_newest_by_ts_in_layout_order() {
        let records: Vec<DeskRecord> = (1..=30).map(|n| record(&format!("w{n}"), n)).collect();
        let kept: Vec<String> = prune(records).into_iter().map(|r| r.id).collect();
        let expected: Vec<String> = (7..=30).map(|n| format!("w{n}")).collect();
        assert_eq!(kept, expected, "the six lowest-ts records are evicted");
    }

    #[test]
    fn prune_preserves_layout_order_not_ts_order() {
        // Layout order and ts order disagree: the survivors must come back in
        // LAYOUT order (w30 first), not newest-first.
        let records: Vec<DeskRecord> = (1..=30).map(|n| record(&format!("w{n}"), 31 - n)).collect();
        let kept: Vec<String> = prune(records).into_iter().map(|r| r.id).collect();
        let expected: Vec<String> = (1..=24).map(|n| format!("w{n}")).collect();
        assert_eq!(kept, expected);
    }

    #[test]
    fn prune_leaves_an_under_cap_desk_untouched() {
        let records: Vec<DeskRecord> = (1..=5).map(|n| record(&format!("w{n}"), n)).collect();
        assert_eq!(prune(records.clone()), records);
    }
}
