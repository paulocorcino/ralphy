//! The configuration root Ralphy owns (ADR-0043 D4).
//!
//! `GEMINI_CLI_HOME` names a directory the CLI appends `.gemini` to, so pointing
//! it at `<workspace>/.ralphy/gemini-home` gives every run a root Ralphy created:
//! the operator's `~/.gemini` is never read by the child and never written, their
//! `GEMINI.md` never reaches the prompt, and `--model` cannot rewrite their
//! defaults.
//!
//! The root is **persistent**, not scratch: `.ralphy/` is gitignored (`*`), so it
//! cannot dirty the tree Ralphy refuses to run against, and keeping it across runs
//! keeps `installation_id` stable rather than minting a throwaway identity per
//! invocation.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde_json::{json, Value};

/// The directory name under the caller's base directory.
const ROOT_DIR_NAME: &str = "gemini-home";

/// The subdirectory the CLI itself appends to `GEMINI_CLI_HOME`.
const CLI_SUBDIR: &str = ".gemini";

/// `<cli_dir>/tmp` — mirrors `TMP_DIR_NAME` in `chunk-AWR3APYV.js:244314`.
const TMP_DIR_NAME: &str = "tmp";

/// `<cli_dir>/tmp/<project-id>/chats` — mirrors `chatsDir` in
/// `chunk-HR7S6IG5.js:10294`.
const CHATS_DIR_NAME: &str = "chats";

/// Mirrors `SESSION_FILE_PREFIX` in `chunk-AWR3APYV.js:276202`.
const SESSION_FILE_PREFIX: &str = "session-";

/// How many sessions Ralphy's own prune keeps, newest-first (matches the
/// `general.sessionRetention.maxCount` written into `settings.json`).
const SESSION_KEEP: usize = 50;

/// Ralphy's own root on disk, after [`ensure`].
pub(crate) struct GeminiRoot {
    /// What `GEMINI_CLI_HOME` is set to — the CLI appends `.gemini` itself.
    pub(crate) home: PathBuf,
    /// `<home>/.gemini/settings.json`, written by [`ensure`].
    pub(crate) settings: PathBuf,
}

impl GeminiRoot {
    /// `<home>/.gemini` — where the policy document and the settings live.
    pub(crate) fn cli_dir(&self) -> PathBuf {
        self.home.join(CLI_SUBDIR)
    }
}

/// The operator's own root. Ralphy reads exactly two things from it — the declared
/// auth mode and their restrictive policy rules — and the CHILD never sees it.
pub(crate) fn operator_root() -> Option<PathBuf> {
    ralphy_proc_util::home_dir().map(|h| h.join(CLI_SUBDIR))
}

/// The operator's declared authentication mode, from `settings.json`'s
/// `security.auth.selectedType`.
///
/// A NON-SECRET POINTER, not a credential: it names which mode the operator chose,
/// never the key itself (ADR-0043 D17 — the credential is never read, copied or
/// replayed). Without it an isolated root is exit 41 for every operator whose key
/// lives in the OS credential store. Any error — missing file, bad JSON, missing
/// key — is `None`, which forwards nothing and lets the vendor's own sentence
/// surface (D6) rather than guessing an auth mode.
pub(crate) fn operator_auth_type(root: Option<&Path>) -> Option<String> {
    let text = std::fs::read_to_string(root?.join("settings.json")).ok()?;
    let v: Value = serde_json::from_str(&text).ok()?;
    v.get("security")?
        .get("auth")?
        .get("selectedType")?
        .as_str()
        .map(str::to_string)
}

/// The minimal settings document Ralphy writes into its own root.
///
/// Four keys and no more:
/// - `security.auth.selectedType` mirrors the operator's declared mode, so an
///   isolated root authenticates the way their own does (omitted entirely when
///   unknown, so exit 41 surfaces with the vendor's own instruction);
/// - `privacy.usageStatisticsEnabled = false` — a run is not the operator opting
///   into telemetry;
/// - `experimental.enableAgents = false` — defensive only (D15). The policy's
///   `invoke_agent` deny is the load-bearing control; this key was observed NOT to
///   remove the tool from the schema, which is why it is not relied on;
/// - `general.sessionRetention` bounds the vendor's own (fire-and-forget, see
///   `ensure`'s prune) session cleanup, inside `validateRetentionConfig`'s bounds
///   (`chunk-HR7S6IG5.js:10485`).
///
/// Pure over its input so the document is asserted without touching a filesystem.
pub(crate) fn settings_document(auth_type: Option<&str>) -> Value {
    let mut doc = json!({
        "privacy": { "usageStatisticsEnabled": false },
        "experimental": { "enableAgents": false },
        "general": { "sessionRetention": {
            "enabled": true,
            "maxAge": "30d",
            "maxCount": SESSION_KEEP
        } }
    });
    if let Some(t) = auth_type {
        doc["security"] = json!({ "auth": { "selectedType": t } });
    }
    doc
}

/// Create `<base>/gemini-home/.gemini/` and write `settings.json` into it.
///
/// `base` is a directory rather than a `Workspace` so `ralphy init` — which has no
/// workspace — can pass `<home>/.ralphy` and reach the same code path, instead of
/// a second root implementation that can drift, and without minting a throwaway
/// installation identity on every probe.
///
/// The file is rewritten only when its bytes differ, so a run does not churn the
/// mtime of a root that is already correct, and a drifted or truncated file is
/// repaired. Nothing else in the directory is touched.
pub(crate) fn ensure(base: &Path) -> Result<GeminiRoot> {
    let home = base.join(ROOT_DIR_NAME);
    let cli_dir = home.join(CLI_SUBDIR);
    std::fs::create_dir_all(&cli_dir)
        .with_context(|| format!("creating the owned gemini root {}", cli_dir.display()))?;

    let settings = cli_dir.join("settings.json");
    let auth_type = operator_auth_type(operator_root().as_deref());
    let want = format!(
        "{}\n",
        serde_json::to_string_pretty(&settings_document(auth_type.as_deref()))?
    );
    let differs = std::fs::read_to_string(&settings)
        .map(|s| s != want)
        .unwrap_or(true);
    if differs {
        std::fs::write(&settings, &want)
            .with_context(|| format!("writing {}", settings.display()))?;
    }

    let pruned = prune_sessions(&cli_dir);
    tracing::debug!(pruned, "pruned gemini sessions beyond the keep count");

    Ok(GeminiRoot { home, settings })
}

/// Every session under `<cli_dir>/tmp/*/chats/`, grouped by file stem (a
/// session is a `.json`+`.jsonl` pair sharing one stem — see
/// `identifySessionsToDelete` in `chunk-HR7S6IG5.js`). The group's time is the
/// MAX mtime of its files. Missing `tmp/` or `chats/` yields an empty vec,
/// never an error — the vendor's own layout is not guaranteed to exist yet.
fn session_stems(cli_dir: &Path) -> Vec<(std::time::SystemTime, String, Vec<PathBuf>)> {
    let mut groups: std::collections::BTreeMap<String, (std::time::SystemTime, Vec<PathBuf>)> =
        std::collections::BTreeMap::new();

    for chats in chat_dirs(cli_dir) {
        group_chats_dir(&mut groups, &chats);
    }

    groups
        .into_iter()
        .map(|(stem, (mtime, files))| (mtime, stem, files))
        .collect()
}

/// Adds every session file of one `chats/` dir to `groups`. An unreadable dir,
/// or a file whose mtime cannot be read, is skipped.
fn group_chats_dir(
    groups: &mut std::collections::BTreeMap<String, (std::time::SystemTime, Vec<PathBuf>)>,
    chats: &Path,
) {
    let Ok(entries) = std::fs::read_dir(chats) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(stem) = session_stem(&path) else {
            continue;
        };
        let Ok(mtime) = entry.metadata().and_then(|m| m.modified()) else {
            continue;
        };
        add_to_group(groups, stem, mtime, &path);
    }
}

/// `<cli_dir>/tmp/*/chats`, one per project dir. Empty when `tmp/` is missing.
fn chat_dirs(cli_dir: &Path) -> Vec<PathBuf> {
    let Ok(projects) = std::fs::read_dir(cli_dir.join(TMP_DIR_NAME)) else {
        return Vec::new();
    };
    projects
        .flatten()
        .map(|project| project.path().join(CHATS_DIR_NAME))
        .collect()
}

/// The group stem of `path` when it is a session file: a `session-*` name with a
/// `.json` or `.jsonl` extension.
fn session_stem(path: &Path) -> Option<&str> {
    let name = path.file_name().and_then(|n| n.to_str())?;
    if !name.starts_with(SESSION_FILE_PREFIX) {
        return None;
    }
    let is_session_file = matches!(
        path.extension().and_then(|e| e.to_str()),
        Some("json") | Some("jsonl")
    );
    if !is_session_file {
        return None;
    }
    path.file_stem().and_then(|s| s.to_str())
}

/// Put `path` in the group of `stem`; the group's time is the newest of its files.
fn add_to_group(
    groups: &mut std::collections::BTreeMap<String, (std::time::SystemTime, Vec<PathBuf>)>,
    stem: &str,
    mtime: std::time::SystemTime,
    path: &Path,
) {
    let group = groups
        .entry(stem.to_string())
        .or_insert((mtime, Vec::new()));
    group.0 = group.0.max(mtime);
    group.1.push(path.to_path_buf());
}

// Delete every session beyond [`SESSION_KEEP`], newest-first by mtime.
/// Never fails: a `remove_file` error is logged and skipped, so a stale
/// Windows file lock cannot fail the run whose root is otherwise correct.
fn prune_sessions(cli_dir: &Path) -> usize {
    let mut groups = session_stems(cli_dir);
    groups.sort_by_key(|g| std::cmp::Reverse(g.0));

    let mut removed = 0;
    for (_, stem, files) in groups.into_iter().skip(SESSION_KEEP) {
        for file in files {
            match std::fs::remove_file(&file) {
                Ok(()) => removed += 1,
                Err(err) => {
                    tracing::warn!(
                        session = stem,
                        path = %file.display(),
                        %err,
                        "failed to prune a gemini session file"
                    );
                }
            }
        }
    }
    removed
}

#[cfg(test)]
mod tests;
