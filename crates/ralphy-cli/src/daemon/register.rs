//! Registering a repo with the daemon — `daemon add` and the passive
//! registration every `run`/`triage`/`init` performs — as ONE path-aware act.
//!
//! The registry is keyed by the project slug (ADR-0008 D7): `owner/repo` from
//! the `origin` remote, or `path-<hash>` when there is none. That key is
//! derived at registration and frozen, while the remote it was derived from is
//! not: a repo registered before `gh repo create` keeps its hash key forever,
//! and a slug-only upsert would then insert the forge key BESIDE it — two
//! sidebar rows for one directory, spend and desk split between them. So
//! registration looks the path up first and *migrates* the entry instead
//! (ADR-0036 amendment 2026-09-16), carrying the usage ledger and the events
//! sink under the new key.
//!
//! The rule: **a forge slug always wins; a hash never displaces a forge slug.**
//! A remote that was removed does not regress the key — that would strand the
//! project's history behind a `git remote remove` typo.

use std::path::Path;

use anyhow::Result;

use ralphy_core::git;
use ralphy_daemon::registry;

use crate::events::config::EventsStore;

/// What one registration did: the key the repo now lives under, the keys that
/// were folded into it, and the hash key that was NOT inserted because a forge
/// key already names the path.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Registration {
    pub(crate) slug: String,
    pub(crate) migrated_from: Vec<String>,
    pub(crate) declined: Option<String>,
}

/// Whether `slug` is a forge key (`owner/repo`) rather than the remoteless
/// `path-<hash>` fallback. The `/` is what `slug_from_url` always yields, so a
/// GitHub repo literally named `path-utils` is still a forge key.
fn is_forge(slug: &str) -> bool {
    slug.contains('/')
}

/// Register `repo_root` in the registry at `registry_path`: derive its slug,
/// fold every entry already naming that path into the winning key, upsert,
/// save. Registry only — path-explicit, no env, no printing; the side stores
/// follow in [`migrate_side_stores`].
pub(crate) fn register_or_migrate(registry_path: &Path, repo_root: &Path) -> Result<Registration> {
    let derived = git::project_slug(repo_root);
    let mut store = registry::load_from(registry_path)?;
    let matches = store.slugs_for_path(repo_root);

    // A hash never displaces a forge key: when the remote is gone (or its URL
    // no longer parses) the forge entry stays and the hash is not inserted.
    let (target, declined) = match matches.iter().find(|s| is_forge(s)) {
        Some(forge) if !is_forge(&derived) => (forge.clone(), Some(derived)),
        _ => (derived, None),
    };

    let mut migrated_from = Vec::new();
    for slug in matches {
        if slug != target && store.rekey(&slug, &target) {
            migrated_from.push(slug);
        }
    }
    store.upsert(&target, &repo_root.to_string_lossy());
    registry::save_to(&store, registry_path)?;
    Ok(Registration {
        slug: target,
        migrated_from,
        declined,
    })
}

/// Carry the usage ledger and the events sink from every key folded into
/// `reg.slug` — the keys this registration moved AND the entry's whole
/// `former_slugs` history, so a run that started under the old key and kept
/// appending after the rename is swept up on the next registration. Best
/// effort per store: a failure warns and the others still run.
pub(crate) fn migrate_side_stores(reg: &Registration, former: &[String]) {
    let mut sources: Vec<&str> = reg.migrated_from.iter().map(String::as_str).collect();
    for slug in former {
        if !sources.contains(&slug.as_str()) {
            sources.push(slug);
        }
    }
    if sources.is_empty() {
        return;
    }
    for from in &sources {
        match ralphy_core::ledger::rename_project(from, &reg.slug) {
            Ok(true) => {
                tracing::info!(from, to = %reg.slug, "usage ledger carried to the new project key")
            }
            Ok(false) => {}
            Err(e) => {
                tracing::warn!(error = %e, from, to = %reg.slug, "usage ledger not carried; the old file stays")
            }
        }
    }
    match EventsStore::load() {
        Ok(mut events) => {
            let moved = sources.iter().any(|from| events.rekey(from, &reg.slug));
            if moved {
                if let Err(e) = events.save() {
                    tracing::warn!(error = %e, "events sink not carried to the new project key");
                }
            }
        }
        Err(e) => tracing::warn!(error = %e, "events store unreadable; sink not carried"),
    }
}

/// Register `repo_root` in the registry at `registry_path` (registry only —
/// the thin form `bootstrap`'s test and the passive path share).
pub(crate) fn register_repo_at(registry_path: &Path, repo_root: &Path) -> Result<Registration> {
    register_or_migrate(registry_path, repo_root)
}

/// Best-effort passive registration for the run/triage/init entry paths. AC5:
/// this MUST NEVER fail a run — the `()` return type structurally forbids
/// propagating an error; a failed write only logs a warning and the run
/// proceeds. Absent from the UI until the next successful write is acceptable.
/// Never prints: the run presenter owns stdout.
pub(crate) fn register_repo(repo_root: &Path) {
    let result = registry::repos_toml_path().and_then(|path| {
        let reg = register_repo_at(&path, repo_root)?;
        let former = registry::load_from(&path)?
            .entry(&reg.slug)
            .map(|e| e.former_slugs.clone())
            .unwrap_or_default();
        migrate_side_stores(&reg, &former);
        Ok(())
    });
    if let Err(e) = result {
        tracing::warn!(error = %e, "failed to register repo with the daemon; run proceeds");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git(dir: &Path, args: &[&str]) {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .expect("git (CI and the build machine have git)");
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// A temp repo whose toplevel is what `resolve_toplevel` would hand the
    /// registrar — canonical, so the hash key is stable across the test.
    fn repo() -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init"]);
        let top = git::resolve_toplevel(dir.path()).unwrap();
        (dir, top)
    }

    #[test]
    fn register_repo_at_writes_entry() {
        // Path-explicit: a temp registry path + a temp repo dir, no env mutation.
        let reg_dir = tempfile::tempdir().unwrap();
        let registry_path = reg_dir.path().join("repos.toml");
        let (_repo, top) = repo();

        let reg = register_repo_at(&registry_path, &top).unwrap();
        assert!(
            reg.slug.starts_with("path-"),
            "remoteless → hash key: {}",
            reg.slug
        );
        assert!(reg.migrated_from.is_empty() && reg.declined.is_none());

        let store = registry::load_from(&registry_path).unwrap();
        assert_eq!(store.repos.len(), 1, "exactly one entry written");
        let entry = store.repos.values().next().unwrap();
        assert_eq!(entry.path, top.to_string_lossy());
    }

    #[test]
    fn register_or_migrate_same_slug_upserts() {
        let reg_dir = tempfile::tempdir().unwrap();
        let registry_path = reg_dir.path().join("repos.toml");
        let (_repo, top) = repo();
        let first = register_or_migrate(&registry_path, &top).unwrap();
        let second = register_or_migrate(&registry_path, &top).unwrap();
        assert_eq!(first, second, "a re-registration is a plain upsert");
        assert_eq!(registry::load_from(&registry_path).unwrap().repos.len(), 1);
    }

    #[test]
    fn register_or_migrate_path_slug_gaining_origin_rekeys_and_records_former() {
        let reg_dir = tempfile::tempdir().unwrap();
        let registry_path = reg_dir.path().join("repos.toml");
        let (dir, top) = repo();
        let hash = register_or_migrate(&registry_path, &top).unwrap().slug;

        git(
            dir.path(),
            &["remote", "add", "origin", "https://github.com/o/r.git"],
        );
        let reg = register_or_migrate(&registry_path, &top).unwrap();
        assert_eq!(reg.slug, "o/r");
        assert_eq!(reg.migrated_from, vec![hash.clone()]);
        assert_eq!(reg.declined, None);

        let store = registry::load_from(&registry_path).unwrap();
        assert_eq!(store.repos.len(), 1, "migrated, never duplicated");
        assert!(store.entry(&hash).is_none());
        let entry = store.entry("o/r").unwrap();
        assert_eq!(entry.path, top.to_string_lossy());
        assert_eq!(entry.former_slugs, vec![hash]);
    }

    #[test]
    fn register_or_migrate_merges_a_preexisting_duplicate_pair() {
        // The pre-migration registrar left BOTH keys for one path; the next
        // registration folds the hash into the forge key.
        let reg_dir = tempfile::tempdir().unwrap();
        let registry_path = reg_dir.path().join("repos.toml");
        let (dir, top) = repo();
        let hash = register_or_migrate(&registry_path, &top).unwrap().slug;
        let mut store = registry::load_from(&registry_path).unwrap();
        store.upsert("o/r", &top.to_string_lossy());
        registry::save_to(&store, &registry_path).unwrap();
        git(
            dir.path(),
            &["remote", "add", "origin", "https://github.com/o/r.git"],
        );

        let reg = register_or_migrate(&registry_path, &top).unwrap();
        assert_eq!(reg.slug, "o/r");
        assert_eq!(reg.migrated_from, vec![hash.clone()]);
        let store = registry::load_from(&registry_path).unwrap();
        assert_eq!(store.repos.len(), 1);
        assert_eq!(store.entry("o/r").unwrap().former_slugs, vec![hash]);
    }

    #[test]
    fn register_or_migrate_lost_remote_keeps_forge_key_and_declines() {
        let reg_dir = tempfile::tempdir().unwrap();
        let registry_path = reg_dir.path().join("repos.toml");
        let (dir, top) = repo();
        git(
            dir.path(),
            &["remote", "add", "origin", "https://github.com/o/r.git"],
        );
        assert_eq!(
            register_or_migrate(&registry_path, &top).unwrap().slug,
            "o/r"
        );

        git(dir.path(), &["remote", "remove", "origin"]);
        let reg = register_or_migrate(&registry_path, &top).unwrap();
        assert_eq!(reg.slug, "o/r", "the forge key stays");
        assert!(reg.migrated_from.is_empty());
        let declined = reg.declined.expect("the hash key was declined");
        assert!(declined.starts_with("path-"));
        let store = registry::load_from(&registry_path).unwrap();
        assert_eq!(store.repos.len(), 1, "no hash entry inserted");
        assert!(store.entry("o/r").unwrap().former_slugs.is_empty());
    }

    #[test]
    fn register_or_migrate_renamed_remote_rekeys_forge_to_forge() {
        let reg_dir = tempfile::tempdir().unwrap();
        let registry_path = reg_dir.path().join("repos.toml");
        let (dir, top) = repo();
        git(
            dir.path(),
            &["remote", "add", "origin", "https://github.com/old/name.git"],
        );
        register_or_migrate(&registry_path, &top).unwrap();
        git(
            dir.path(),
            &[
                "remote",
                "set-url",
                "origin",
                "https://github.com/new/name.git",
            ],
        );

        let reg = register_or_migrate(&registry_path, &top).unwrap();
        assert_eq!(reg.slug, "new/name");
        assert_eq!(reg.migrated_from, vec!["old/name".to_string()]);
        let store = registry::load_from(&registry_path).unwrap();
        assert_eq!(store.repos.len(), 1);
        assert_eq!(
            store.entry("new/name").unwrap().former_slugs,
            vec!["old/name".to_string()]
        );
    }
}
