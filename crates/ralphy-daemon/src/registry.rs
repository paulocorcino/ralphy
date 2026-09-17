//! The daemon's repo registry (docs/adr/0032): the set of repos Ralphy knows
//! about, keyed by `owner/repo` slug, persisted in the global store
//! (`<home>/.ralphy/repos.toml`) — never under a repo-local `.ralphy/`.
//!
//! Pure sync and path-explicit, mirroring `identity`: tests pass a temp path and
//! never mutate the process-global env (the `RALPHY_*_DIR` env-race trap). The
//! CLI (which has `ralphy-core`) computes the slug and calls this store; this
//! module never depends on `ralphy-core` (ADR-0032: the daemon must not import
//! the core). Reachability is computed at read time, never persisted — a stale
//! flag would contradict "never removed automatically" and self-healing.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// One registered repo: its filesystem path. Reachability is derived, not
/// stored, so a moved repo self-heals and a returned repo un-greys with no write.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoEntry {
    pub path: String,
    /// The keys this entry was registered under before — a remoteless
    /// `path-<hash>` that gained a forge remote, or a renamed remote (ADR-0036
    /// amendment 2026-09-16). Written by the CLI's migration
    /// ([`RegistryStore::rekey`]), read by the desk routes to normalize a record
    /// a stale tab still uploads under the old key. Omitted when empty so an
    /// older store and a peer's parser keep their exact shape.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub former_slugs: Vec<String>,
    /// RETIRED (ADR-0063 §3, #408): the experimental `claude --worktree` opt-in.
    /// Loaded so an older `repos.toml` still parses, never read for behaviour,
    /// never written back — a console opens in the worktree the picker selected.
    #[serde(default, skip_serializing)]
    console_worktree: Option<bool>,
}

impl RepoEntry {
    /// Whether the stored path currently resolves to a directory. Computed on
    /// each read — an unreachable repo is flagged, never removed.
    pub fn reachable(&self) -> bool {
        Path::new(&self.path).is_dir()
    }

    /// The absolute, canonical native path of the repo root — what the explorer
    /// joins a relative path onto for "Copy full path". Separate from
    /// [`Self::path`], which stays byte-identical to what git's `--show-toplevel`
    /// stored (forward-slashed on Windows) because peers parse it.
    ///
    /// The verbatim prefix `canonicalize` returns on Windows is stripped: it is a
    /// Win32 API escape hatch, not something an operator pastes into a shell. The
    /// UNC form is stripped FIRST and restored to its `\\server\share` spelling —
    /// stripping only `\\?\` would leave the literal `UNC\server\share`, which is
    /// not a path in any shell. `None` when the repo is unreachable.
    pub fn root(&self) -> Option<String> {
        let canon = std::fs::canonicalize(&self.path).ok()?;
        let s = canon.to_string_lossy().into_owned();
        Some(match s.strip_prefix(r"\\?\UNC\") {
            Some(rest) => format!(r"\\{rest}"),
            None => s.strip_prefix(r"\\?\").unwrap_or(&s).to_string(),
        })
    }

    /// The current branch name, read fresh from `<path>/.git/HEAD`. `None` for
    /// a detached HEAD (raw commit sha), a missing/unreadable `.git/HEAD` (no
    /// repo, or a worktree/submodule gitdir-pointer file), or any other
    /// non-`ref:` content.
    pub fn head_branch(&self) -> Option<String> {
        let head = Path::new(&self.path).join(".git").join("HEAD");
        let s = std::fs::read_to_string(head).ok()?;
        s.trim()
            .strip_prefix("ref: refs/heads/")
            .map(str::to_string)
    }

    /// Whether the working tree has uncommitted changes, via `git -C <path>
    /// status --porcelain` (dirty = non-empty stdout). `false` on a spawn error
    /// or a non-git dir (no working-tree state to report). Spawns a subprocess,
    /// so callers on the async reactor MUST run this in `spawn_blocking`.
    pub fn dirty(&self) -> bool {
        match git_output(&self.path, &["status", "--porcelain"]) {
            Some((true, stdout)) => !stdout.trim().is_empty(),
            _ => false,
        }
    }

    /// Whether `root` names this entry's directory. Canonical paths decide when
    /// both resolve; otherwise the two strings are compared normalized — `\`
    /// and `/` alike, no trailing separator, ASCII case-folded on Windows —
    /// so an unreachable entry still matches the spelling that registered it.
    pub fn same_root(&self, root: &Path) -> bool {
        if let (Ok(a), Ok(b)) = (
            std::fs::canonicalize(&self.path),
            std::fs::canonicalize(root),
        ) {
            return a == b;
        }
        normalize_path(&self.path) == normalize_path(&root.to_string_lossy())
    }

    /// The `origin` remote URL, via `git -C <path> remote get-url origin`.
    /// `None` when there is no `origin` (non-zero exit), the URL is empty, or
    /// git cannot be spawned. Spawns a subprocess — run in `spawn_blocking` off
    /// the async reactor.
    pub fn remote(&self) -> Option<String> {
        match git_output(&self.path, &["remote", "get-url", "origin"]) {
            Some((true, stdout)) => {
                let url = stdout.trim();
                (!url.is_empty()).then(|| url.to_string())
            }
            _ => None,
        }
    }
}

/// The string form [`RepoEntry::same_root`] falls back to: one separator, no
/// trailing one, case-folded where the filesystem is (Windows).
fn normalize_path(s: &str) -> String {
    let unified = s.replace('\\', "/");
    let trimmed = unified.trim_end_matches('/');
    if cfg!(windows) {
        trimmed.to_ascii_lowercase()
    } else {
        trimmed.to_string()
    }
}

/// Run `git -C <path> <args…>` with piped stdio (no console window on Windows;
/// see `CREATE_NO_WINDOW`), returning `(status.success(), stdout)` or `None`
/// when git cannot be spawned.
fn git_output(path: &str, args: &[&str]) -> Option<(bool, String)> {
    use std::process::Command;
    let mut cmd = Command::new("git");
    cmd.args(["-C", path]).args(args);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let out = cmd.output().ok()?;
    Some((
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
    ))
}

/// The persisted registry: slug → entry. The slug carries a `/`, so TOML quotes
/// the key (see the round-trip test).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistryStore {
    #[serde(default)]
    pub repos: BTreeMap<String, RepoEntry>,
}

impl RegistryStore {
    /// Insert or overwrite the entry for `slug`. Overwriting is the self-heal:
    /// a moved repo re-registers under the same slug with its new path. The
    /// entry's `former_slugs` survive the overwrite — they are the rename
    /// history, and a passive re-registration must not erase it.
    pub fn upsert(&mut self, slug: &str, path: &str) {
        let former_slugs = self
            .repos
            .get(slug)
            .map(|e| e.former_slugs.clone())
            .unwrap_or_default();
        self.repos.insert(
            slug.into(),
            RepoEntry {
                path: path.into(),
                former_slugs,
                console_worktree: None,
            },
        );
    }

    /// Every slug whose entry names `root` (see [`RepoEntry::same_root`]), in
    /// key order. Plural because the pre-migration registrar could leave the
    /// same path under both its `path-<hash>` and its `owner/repo` key.
    pub fn slugs_for_path(&self, root: &Path) -> Vec<String> {
        self.repos
            .iter()
            .filter(|(_, e)| e.same_root(root))
            .map(|(slug, _)| slug.clone())
            .collect()
    }

    /// Move the entry under `from` to the key `to`. An existing `to` keeps its
    /// own path and absorbs `from`'s history plus `from` itself; the target
    /// never lists itself as a former slug. `false` when `from` is absent or
    /// equals `to` — nothing moved.
    pub fn rekey(&mut self, from: &str, to: &str) -> bool {
        if from == to {
            return false;
        }
        let Some(old) = self.repos.remove(from) else {
            return false;
        };
        let target = self.repos.entry(to.to_string()).or_insert(RepoEntry {
            path: old.path.clone(),
            former_slugs: Vec::new(),
            console_worktree: None,
        });
        for slug in old.former_slugs.into_iter().chain([from.to_string()]) {
            if slug != to && !target.former_slugs.contains(&slug) {
                target.former_slugs.push(slug);
            }
        }
        true
    }

    /// Every former slug → the key it now lives under, over the whole store.
    pub fn former_slug_map(&self) -> BTreeMap<String, String> {
        self.repos
            .iter()
            .flat_map(|(slug, e)| {
                e.former_slugs
                    .iter()
                    .map(move |f| (f.clone(), slug.clone()))
            })
            .collect()
    }

    /// The slugs whose entry still carries the retired `console_worktree` key
    /// (presence, not value) — what the startup notice names once.
    pub fn retired_console_worktree(&self) -> Vec<&str> {
        self.repos
            .iter()
            .filter(|(_, e)| e.console_worktree.is_some())
            .map(|(slug, _)| slug.as_str())
            .collect()
    }

    /// Remove the entry for `slug`; `true` when one was present (idempotent:
    /// removing an absent slug returns `false`).
    pub fn remove(&mut self, slug: &str) -> bool {
        self.repos.remove(slug).is_some()
    }

    /// The entry for `slug`, if registered.
    pub fn entry(&self, slug: &str) -> Option<&RepoEntry> {
        self.repos.get(slug)
    }
}

/// Load a [`RegistryStore`] from `path`, or `Ok(default())` when the file does
/// not exist yet (no repos registered).
pub fn load_from(path: &Path) -> Result<RegistryStore> {
    match std::fs::read_to_string(path) {
        Ok(text) => toml::from_str(&text).with_context(|| format!("parsing {}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(RegistryStore::default()),
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

/// Write the store to `path` owner-only, creating the parent directory.
pub fn save_to(store: &RegistryStore, path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    let text = toml::to_string_pretty(store).context("serializing repo registry")?;
    std::fs::write(path, text).with_context(|| format!("writing {}", path.display()))?;
    set_owner_only(path)?;
    Ok(())
}

/// The production path of `repos.toml`: `$RALPHY_DAEMON_DIR` when set (tests
/// point it at a temp dir), else `<home>/.ralphy/repos.toml` — the same global
/// store root as `daemon.toml`, never a repo-local `.ralphy/`.
pub fn repos_toml_path() -> Result<PathBuf> {
    if let Some(dir) = std::env::var_os("RALPHY_DAEMON_DIR") {
        return Ok(PathBuf::from(dir).join("repos.toml"));
    }
    let home = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .context("could not resolve a home directory for the repo registry store")?;
    Ok(PathBuf::from(home).join(".ralphy").join("repos.toml"))
}

/// Load the current registry from its production path.
pub fn load_current() -> Result<RegistryStore> {
    load_from(&repos_toml_path()?)
}

/// Restrict a freshly written store file to the owner only (mode `0o600` on
/// unix; the per-user home ACL on Windows), mirroring the identity store.
#[cfg(unix)]
pub(crate) fn set_owner_only(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let perms = std::fs::Permissions::from_mode(0o600);
    std::fs::set_permissions(path, perms)
        .with_context(|| format!("setting owner-only permissions on {}", path.display()))
}

#[cfg(not(unix))]
pub(crate) fn set_owner_only(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests;
