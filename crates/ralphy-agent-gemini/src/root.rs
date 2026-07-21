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

    let Ok(projects) = std::fs::read_dir(cli_dir.join(TMP_DIR_NAME)) else {
        return Vec::new();
    };
    for project in projects.flatten() {
        let chats = project.path().join(CHATS_DIR_NAME);
        let Ok(entries) = std::fs::read_dir(&chats) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if !name.starts_with(SESSION_FILE_PREFIX) {
                continue;
            }
            let is_session_file = matches!(
                path.extension().and_then(|e| e.to_str()),
                Some("json") | Some("jsonl")
            );
            if !is_session_file {
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            let Ok(mtime) = entry.metadata().and_then(|m| m.modified()) else {
                continue;
            };
            let group = groups
                .entry(stem.to_string())
                .or_insert((mtime, Vec::new()));
            group.0 = group.0.max(mtime);
            group.1.push(path);
        }
    }

    groups
        .into_iter()
        .map(|(stem, (mtime, files))| (mtime, stem, files))
        .collect()
}

/// Delete every session beyond [`SESSION_KEEP`], newest-first by mtime.
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
                    tracing::warn!(session = stem, path = %file.display(), %err, "failed to prune a gemini session file");
                }
            }
        }
    }
    removed
}

#[cfg(test)]
mod tests {
    use super::*;

    /// D4: the root is per-workspace and persistent, so `ensure` must be a no-op
    /// on a root that is already correct — a rewrite every run would churn the
    /// installation identity the vendor keys on.
    #[test]
    fn ensure_is_idempotent() {
        let base = tempfile::tempdir().unwrap();
        let first = ensure(base.path()).unwrap();
        let bytes = std::fs::read(&first.settings).unwrap();
        let second = ensure(base.path()).unwrap();
        assert_eq!(first.settings, second.settings);
        assert_eq!(std::fs::read(&second.settings).unwrap(), bytes);

        // Exactly one file, in exactly one place.
        let entries: Vec<_> = std::fs::read_dir(first.cli_dir())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(entries, ["settings.json"], "{entries:?}");
        assert_eq!(first.home, base.path().join("gemini-home"));
        assert!(first.settings.starts_with(&first.home));
    }

    /// A truncated or hand-edited settings file is repaired — and anything else
    /// the root accumulated across runs (session state, the installation id) is
    /// left exactly as it was.
    #[test]
    fn ensure_restores_a_drifted_settings_file() {
        let base = tempfile::tempdir().unwrap();
        let root = ensure(base.path()).unwrap();
        let want = std::fs::read_to_string(&root.settings).unwrap();

        let sibling = root.cli_dir().join("installation_id");
        std::fs::write(&sibling, b"keep-me").unwrap();
        std::fs::write(&root.settings, b"{ corrupted").unwrap();

        ensure(base.path()).unwrap();
        let restored = std::fs::read_to_string(&root.settings).unwrap();
        assert_eq!(restored, want);
        assert!(
            restored.contains("sessionRetention"),
            "the restored document must still bound session retention: {restored}"
        );
        assert_eq!(
            std::fs::read_to_string(&sibling).unwrap(),
            "keep-me",
            "an unrelated sibling file must survive"
        );
    }

    /// The two keys Ralphy forces regardless of the operator, and the one it
    /// mirrors. An absent auth mode omits the key entirely rather than guessing.
    #[test]
    fn the_settings_document_forces_the_privacy_and_agents_keys() {
        let doc = settings_document(Some("gemini-api-key"));
        assert_eq!(doc["privacy"]["usageStatisticsEnabled"], json!(false));
        assert_eq!(doc["experimental"]["enableAgents"], json!(false));
        assert_eq!(
            doc["security"]["auth"]["selectedType"],
            json!("gemini-api-key")
        );

        let unknown = settings_document(None);
        assert!(
            unknown.get("security").is_none(),
            "an unknown auth mode must not be guessed: {unknown}"
        );
        assert_eq!(unknown["privacy"]["usageStatisticsEnabled"], json!(false));
    }

    /// The settings document declares the vendor's own retention bound —
    /// belt-and-suspenders alongside Ralphy's own [`prune_sessions`], since the
    /// vendor's cleanup is fire-and-forget at startup.
    #[test]
    fn the_settings_document_bounds_session_retention() {
        let doc = settings_document(None);
        assert_eq!(doc["general"]["sessionRetention"]["enabled"], json!(true));
        assert_eq!(doc["general"]["sessionRetention"]["maxAge"], json!("30d"));
        assert_eq!(doc["general"]["sessionRetention"]["maxCount"], json!(50));
    }

    /// The auth-mode read is a pointer lookup that fails to `None` on every bad
    /// shape — never a panic, never a partial guess.
    #[test]
    fn the_operator_auth_type_fails_soft() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(operator_auth_type(None), None);
        assert_eq!(operator_auth_type(Some(dir.path())), None, "no file");

        std::fs::write(dir.path().join("settings.json"), b"not json").unwrap();
        assert_eq!(operator_auth_type(Some(dir.path())), None, "bad json");

        std::fs::write(dir.path().join("settings.json"), br#"{"security":{}}"#).unwrap();
        assert_eq!(operator_auth_type(Some(dir.path())), None, "missing key");

        std::fs::write(
            dir.path().join("settings.json"),
            br#"{"security":{"auth":{"selectedType":"vertex-ai"}}}"#,
        )
        .unwrap();
        assert_eq!(
            operator_auth_type(Some(dir.path())).as_deref(),
            Some("vertex-ai")
        );
    }

    /// Writes `<chats>/session-<i>.json` + `.jsonl`, both stamped `i` seconds
    /// after the Unix epoch so mtime order is deterministic across the pair
    /// and across the whole synthetic set.
    fn write_session_pair(chats: &Path, i: u32) {
        let mtime = std::time::UNIX_EPOCH + std::time::Duration::from_secs(i as u64);
        for ext in ["json", "jsonl"] {
            let path = chats.join(format!("session-{i:02}-{i:02}.{ext}"));
            let file = std::fs::File::create(&path).unwrap();
            file.set_modified(mtime).unwrap();
        }
    }

    #[test]
    fn ensure_prunes_sessions_beyond_the_keep_count() {
        let base = tempfile::tempdir().unwrap();
        let root = ensure(base.path()).unwrap();
        let chats = root.cli_dir().join("tmp").join("proj-abc").join("chats");
        std::fs::create_dir_all(&chats).unwrap();
        for i in 0..60 {
            write_session_pair(&chats, i);
        }

        ensure(base.path()).unwrap();

        let mut stems = std::fs::read_dir(&chats)
            .unwrap()
            .map(|e| {
                e.unwrap()
                    .path()
                    .file_stem()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(stems.len(), 50, "{stems:?}");
        for i in 0..10 {
            assert!(
                !stems.contains(&format!("session-{i:02}-{i:02}")),
                "the oldest sessions must be pruned: {stems:?}"
            );
        }
        assert!(stems.remove("session-59-59"));
    }

    #[test]
    fn ensure_leaves_unowned_files_in_the_session_dir_alone() {
        let base = tempfile::tempdir().unwrap();
        let root = ensure(base.path()).unwrap();
        let chats = root.cli_dir().join("tmp").join("proj-abc").join("chats");
        std::fs::create_dir_all(&chats).unwrap();
        for i in 0..60 {
            write_session_pair(&chats, i);
        }
        let notes = chats.join("notes.txt");
        let decoy = chats.join("session-decoy.txt");
        std::fs::write(&notes, b"mine").unwrap();
        std::fs::write(&decoy, b"mine").unwrap();

        ensure(base.path()).unwrap();

        assert_eq!(std::fs::read(&notes).unwrap(), b"mine");
        assert_eq!(std::fs::read(&decoy).unwrap(), b"mine");
    }

    #[test]
    fn ensure_is_idempotent_with_sessions_present() {
        let base = tempfile::tempdir().unwrap();
        let root = ensure(base.path()).unwrap();
        let chats = root.cli_dir().join("tmp").join("proj-abc").join("chats");
        std::fs::create_dir_all(&chats).unwrap();
        for i in 0..3 {
            write_session_pair(&chats, i);
        }
        ensure(base.path()).unwrap();

        fn snapshot(dir: &Path) -> Vec<(PathBuf, std::time::SystemTime)> {
            let mut out = Vec::new();
            for entry in walkdir(dir) {
                let mtime = entry.metadata().unwrap().modified().unwrap();
                out.push((entry.path(), mtime));
            }
            out.sort();
            out
        }
        fn walkdir(dir: &Path) -> Vec<std::fs::DirEntry> {
            let mut out = Vec::new();
            for entry in std::fs::read_dir(dir).unwrap() {
                let entry = entry.unwrap();
                if entry.file_type().unwrap().is_dir() {
                    out.extend(walkdir(&entry.path()));
                } else {
                    out.push(entry);
                }
            }
            out
        }

        let before = snapshot(&root.cli_dir());
        ensure(base.path()).unwrap();
        let after = snapshot(&root.cli_dir());
        assert_eq!(before, after);
    }

    #[test]
    fn the_installation_identity_survives_reconciliation() {
        let base = tempfile::tempdir().unwrap();
        let root = ensure(base.path()).unwrap();
        let id_file = root.cli_dir().join("installation_id");
        std::fs::write(&id_file, b"b54f6a30-stable").unwrap();

        ensure(base.path()).unwrap();

        assert_eq!(
            std::fs::read_to_string(&id_file).unwrap(),
            "b54f6a30-stable"
        );
    }

    #[test]
    fn two_workspaces_get_two_independent_roots() {
        let base_a = tempfile::tempdir().unwrap();
        let base_b = tempfile::tempdir().unwrap();
        let root_a = ensure(base_a.path()).unwrap();
        let root_b = ensure(base_b.path()).unwrap();
        assert_ne!(root_a.home, root_b.home);

        let b_bytes = std::fs::read(&root_b.settings).unwrap();
        let b_mtime = std::fs::metadata(&root_b.settings)
            .unwrap()
            .modified()
            .unwrap();

        std::fs::remove_file(&root_a.settings).unwrap();
        ensure(base_a.path()).unwrap();

        assert!(root_a.settings.exists(), "base A's root must be restored");
        assert_eq!(
            std::fs::read(&root_b.settings).unwrap(),
            b_bytes,
            "base B's settings must be untouched by an ensure() on base A"
        );
        assert_eq!(
            std::fs::metadata(&root_b.settings)
                .unwrap()
                .modified()
                .unwrap(),
            b_mtime,
            "base B's mtime must be untouched by an ensure() on base A"
        );
    }

    /// D17: the operator's root is reached for the auth POINTER and their policy
    /// rules only — no credential file is ever named here.
    #[test]
    fn the_root_module_names_no_credential_file() {
        let production = include_str!("root.rs")
            .split("#[cfg(test)]")
            .next()
            .unwrap();
        for banned in ["oauth_creds", "google_accounts", "keytar", "access_token"] {
            assert!(
                !production.contains(banned),
                "the credential is never read (D17); found {banned}"
            );
        }
    }
}
