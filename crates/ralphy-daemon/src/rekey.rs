//! The registry's self-heal on a gained remote (ADR-0036 amendment 2026-09-16).
//!
//! A repo registered before it had an `origin` is keyed `path-<hash>`; the
//! daemon can SEE that the remote has since appeared — `/api/repos` reads it
//! live — but may not interpret it (ADR-0036 §3: the slug a URL yields is
//! `ralphy-core`'s call). So the daemon does what it does for `project.remove`:
//! it observes the fact and hands the interpretation to `ralphy daemon add`,
//! whose registrar migrates the key. One trigger, on the read that noticed it,
//! synchronous — so the first page that sees the remote already sees
//! `owner/repo`, never a hash it has to reload away.
//!
//! The rename fact itself lives in the registry (`RepoEntry::former_slugs`),
//! not here: a tab that read the desk under the old key and uploads it back —
//! after a migration the daemon never saw, or across a daemon restart — is
//! normalized through that map by the desk routes ([`rekey_desk`]).

use std::collections::{BTreeMap, HashSet};
use std::ffi::OsStr;
use std::path::Path;
use std::sync::{Arc, Mutex};

use crate::desk::DeskStore;
use crate::dispatch;

/// The `(slug, remote)` pairs already handed to `ralphy daemon add` this router
/// lifetime. A remote whose URL yields no forge slug leaves the key a hash, and
/// without the memo every page load would respawn the registrar for it.
pub(crate) type HealMemo = Arc<Mutex<HashSet<(String, String)>>>;

pub(crate) fn heal_memo() -> HealMemo {
    Arc::new(Mutex::new(HashSet::new()))
}

/// One registry row the daemon has observed to be remoteless-keyed with a
/// remote present: what `daemon add` is asked to reconcile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Candidate {
    pub(crate) slug: String,
    pub(crate) path: String,
    pub(crate) remote: String,
}

/// Whether `slug` is the remoteless fallback. The `/` test is not optional: a
/// forge repo literally named `owner/path-utils` is NOT this case (#332).
fn is_hash_key(slug: &str) -> bool {
    !slug.contains('/') && slug.starts_with("path-")
}

/// The rows worth a registrar spawn — hash-keyed, with an origin, not yet
/// claimed — claimed in `memo` as a side effect so the spawn happens once
/// whatever it yields.
pub(crate) fn claim_candidates<'a>(
    memo: &HealMemo,
    rows: impl Iterator<Item = (&'a str, &'a str, Option<&'a str>)>,
) -> Vec<Candidate> {
    let mut claimed = memo
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    rows.filter_map(|(slug, path, remote)| {
        let remote = remote?;
        if !is_hash_key(slug) {
            return None;
        }
        claimed
            .insert((slug.to_string(), remote.to_string()))
            .then(|| Candidate {
                slug: slug.to_string(),
                path: path.to_string(),
                remote: remote.to_string(),
            })
    })
    .collect()
}

/// Spawn-and-collect `daemon add <path>` per candidate: the CLI re-derives the
/// slug from the remote and migrates the entry (registry, ledger, events sink).
/// Never `--init` — every candidate is a repository already, and a heal must
/// not create one. The child's output is logged, not parsed; the caller
/// re-reads the registry for the truth. Returns whether any registrar ran.
/// Blocking (a `wait` per candidate) — run in `spawn_blocking`.
pub(crate) fn heal(spawner: &dyn dispatch::Spawner, program: &OsStr, todo: &[Candidate]) -> bool {
    for c in todo {
        let argv = ["daemon", "add", c.path.as_str()];
        match dispatch::collect(spawner, program, &argv, Path::new(&c.path), None) {
            Ok((Some(0), out)) => tracing::info!(
                slug = %c.slug,
                remote = %c.remote,
                output = %String::from_utf8_lossy(&out).trim(),
                "registry re-keyed after a gained remote"
            ),
            Ok((code, out)) => tracing::warn!(
                slug = %c.slug,
                remote = %c.remote,
                ?code,
                output = %String::from_utf8_lossy(&out).trim(),
                "registrar did not re-key the entry; the hash key stays"
            ),
            Err(e) => tracing::warn!(slug = %c.slug, error = %e, "could not spawn the registrar"),
        }
    }
    !todo.is_empty()
}

/// Rewrite every desk record's `repo` and every `checkouts` key through
/// `aliases` (former slug → the key it now lives under). Pure. When a stale
/// checkout selection collides with one already under the canonical key, the
/// canonical one wins — it is the newer fact.
pub(crate) fn rekey_desk(mut store: DeskStore, aliases: &BTreeMap<String, String>) -> DeskStore {
    if aliases.is_empty() {
        return store;
    }
    for record in &mut store.windows {
        if let Some(to) = aliases.get(&record.repo) {
            record.repo = to.clone();
        }
    }
    let mut checkouts = BTreeMap::new();
    // Canonical keys first, so a stale alias never overwrites one.
    for (repo, name) in &store.checkouts {
        if !aliases.contains_key(repo) {
            checkouts.insert(repo.clone(), name.clone());
        }
    }
    for (repo, name) in &store.checkouts {
        if let Some(to) = aliases.get(repo) {
            checkouts.entry(to.clone()).or_insert_with(|| name.clone());
        }
    }
    store.checkouts = checkouts;
    store
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::desk::{DeskRecord, DeskRect};
    use crate::dispatch::{Child, Spawner};
    use std::path::PathBuf;

    #[test]
    fn claim_candidates_skips_forge_slugs_remoteless_and_already_claimed() {
        let memo = heal_memo();
        let rows = [
            ("path-abc", "/a", Some("https://github.com/o/a.git")),
            ("path-def", "/d", None),
            (
                "owner/path-utils",
                "/u",
                Some("https://github.com/owner/path-utils"),
            ),
            ("owner/repo", "/r", Some("https://github.com/owner/repo")),
        ];
        let first = claim_candidates(&memo, rows.iter().copied());
        assert_eq!(
            first,
            vec![Candidate {
                slug: "path-abc".into(),
                path: "/a".into(),
                remote: "https://github.com/o/a.git".into(),
            }],
            "only the hash key WITH a remote is a candidate"
        );
        assert!(
            claim_candidates(&memo, rows.iter().copied()).is_empty(),
            "a claimed pair never spawns twice"
        );
        // A DIFFERENT remote on the same hash key is a new fact.
        let changed = [("path-abc", "/a", Some("https://github.com/o/b.git"))];
        assert_eq!(claim_candidates(&memo, changed.iter().copied()).len(), 1);
    }

    struct FakeSpawner {
        calls: Mutex<Vec<(Vec<String>, PathBuf)>>,
    }
    struct FakeChild;
    impl Child for FakeChild {
        fn pid(&self) -> Option<u32> {
            Some(1)
        }
        fn wait(&mut self) -> anyhow::Result<Option<i32>> {
            Ok(Some(0))
        }
        fn take_output(&mut self) -> Option<Box<dyn std::io::Read + Send>> {
            None
        }
    }
    impl Spawner for FakeSpawner {
        fn spawn(
            &self,
            _program: &OsStr,
            args: &[&str],
            cwd: &Path,
            daemon_id: Option<&str>,
        ) -> anyhow::Result<Box<dyn Child>> {
            assert!(daemon_id.is_none(), "a heal carries no daemon id");
            self.calls.lock().unwrap().push((
                args.iter().map(|a| a.to_string()).collect(),
                cwd.to_path_buf(),
            ));
            Ok(Box::new(FakeChild))
        }
    }

    #[test]
    fn heal_spawns_daemon_add_per_candidate() {
        let spawner = FakeSpawner {
            calls: Mutex::new(Vec::new()),
        };
        assert!(!heal(&spawner, OsStr::new("ralphy"), &[]), "nothing to do");
        let todo = vec![
            Candidate {
                slug: "path-a".into(),
                path: "/repo-a".into(),
                remote: "r".into(),
            },
            Candidate {
                slug: "path-b".into(),
                path: "/repo-b".into(),
                remote: "r".into(),
            },
        ];
        assert!(heal(&spawner, OsStr::new("ralphy"), &todo));
        let calls = spawner.calls.lock().unwrap();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].0, vec!["daemon", "add", "/repo-a"]);
        assert_eq!(calls[0].1, PathBuf::from("/repo-a"));
        assert_eq!(calls[1].0, vec!["daemon", "add", "/repo-b"]);
        assert!(
            calls
                .iter()
                .all(|(argv, _)| !argv.iter().any(|a| a == "--init")),
            "a heal never initializes a repository"
        );
    }

    fn record(id: &str, repo: &str) -> DeskRecord {
        DeskRecord {
            id: id.into(),
            repo: repo.into(),
            rect: DeskRect {
                left: 0.0,
                top: 0.0,
                width: 10.0,
                height: 10.0,
            },
            ..DeskRecord::default()
        }
    }

    #[test]
    fn rekey_desk_rewrites_windows_and_checkouts_and_canonical_wins() {
        let mut store = DeskStore {
            windows: vec![record("w1", "path-abc"), record("w2", "o/other")],
            ..DeskStore::default()
        };
        store
            .checkouts
            .insert("path-abc".into(), "stale-tree".into());
        store.checkouts.insert("o/r".into(), "live-tree".into());
        store
            .checkouts
            .insert("path-solo".into(), "solo-tree".into());
        let aliases: BTreeMap<String, String> = [
            ("path-abc".to_string(), "o/r".to_string()),
            ("path-solo".to_string(), "o/solo".to_string()),
        ]
        .into_iter()
        .collect();

        let out = rekey_desk(store.clone(), &aliases);
        assert_eq!(out.windows[0].repo, "o/r", "a stale record follows the key");
        assert_eq!(
            out.windows[1].repo, "o/other",
            "an unrelated record is untouched"
        );
        assert_eq!(
            out.checkouts.get("o/r").map(String::as_str),
            Some("live-tree"),
            "the canonical selection wins the collision"
        );
        assert_eq!(
            out.checkouts.get("o/solo").map(String::as_str),
            Some("solo-tree")
        );
        assert!(!out.checkouts.contains_key("path-abc"));
        assert!(!out.checkouts.contains_key("path-solo"));

        assert_eq!(
            rekey_desk(store.clone(), &BTreeMap::new()),
            store,
            "no aliases, no change"
        );
    }
}
