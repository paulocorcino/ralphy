//! Transcript/rollout **snapshot-diff** helpers, shared by the adapters that
//! attribute token usage to a run by comparing the session-store directory before
//! and after the session (ADR-0008 D10, "appeared-over-grew").
//!
//! The vendor-specific parts — the file extension, whether the store nests files
//! in subdirectories, and any filename prefix — are passed in by each adapter.
//! Claude's transcripts sit flat as `*.jsonl`; Codex nests `rollout-*.jsonl` under
//! `<YYYY>/<MM>/<DD>/`. OpenCode has no file store (it correlates a session id to a
//! SQLite DB) and uses neither helper.

use std::fs;
use std::path::{Path, PathBuf};

/// The files present in `after` but not in `before` — the appeared-over-grew rule
/// (ADR-0008 D10). A file that merely *grew* (present in both) is a concurrent
/// pre-existing session and is excluded; only a freshly *appeared* session file is
/// attributed to this run. Pure over its inputs.
pub fn session_files_appeared(before: &[PathBuf], after: &[PathBuf]) -> Vec<PathBuf> {
    after
        .iter()
        .filter(|p| !before.contains(p))
        .cloned()
        .collect()
}

/// List the session files under `dir` whose extension is `ext` and whose file name
/// starts with `name_prefix` (when `Some`). When `recursive`, descends into
/// subdirectories; otherwise scans `dir` alone. Empty when `dir` is missing or
/// unreadable — best-effort, never failing the run.
///
/// Claude calls this `(dir, "jsonl", false, None)` (flat `*.jsonl`); Codex calls it
/// `(dir, "jsonl", true, Some("rollout-"))` (nested `rollout-*.jsonl`).
pub fn list_session_files(
    dir: &Path,
    ext: &str,
    recursive: bool,
    name_prefix: Option<&str>,
) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if recursive && path.is_dir() {
            out.extend(list_session_files(&path, ext, recursive, name_prefix));
            continue;
        }
        let ext_ok = path.extension().and_then(|e| e.to_str()) == Some(ext);
        let prefix_ok = match name_prefix {
            Some(pre) => path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with(pre)),
            None => true,
        };
        if ext_ok && prefix_ok {
            out.push(path);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_files_appeared_returns_new_not_grown() {
        // Moved from the Claude adapter (`appeared_files_returns_new_not_grown`) and
        // the Codex adapter (`appeared_returns_new_not_grown`): a file present in
        // both snapshots merely grew and is excluded; only the freshly appeared file
        // is attributed to this run.
        let a = PathBuf::from("/s/a.jsonl");
        let b = PathBuf::from("/s/b.jsonl");
        let before = vec![a.clone()];
        let after = vec![a.clone(), b.clone()];
        assert_eq!(session_files_appeared(&before, &after), vec![b]);
        // A snapshot that only grew (same set) yields nothing.
        assert!(session_files_appeared(&after, &after).is_empty());
    }

    #[test]
    fn list_session_files_flat_matches_extension() {
        let dir = std::env::temp_dir().join("ralphy-sf-flat-test");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("one.jsonl"), "").unwrap();
        fs::write(dir.join("two.jsonl"), "").unwrap();
        fs::write(dir.join("note.txt"), "").unwrap();
        let sub = dir.join("nested");
        fs::create_dir_all(&sub).unwrap();
        fs::write(sub.join("deep.jsonl"), "").unwrap();

        let mut got = list_session_files(&dir, "jsonl", false, None);
        got.sort();
        assert_eq!(got, vec![dir.join("one.jsonl"), dir.join("two.jsonl")]);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn list_session_files_recursive_with_prefix() {
        let dir = std::env::temp_dir().join("ralphy-sf-rec-test");
        let _ = fs::remove_dir_all(&dir);
        let day = dir.join("2026").join("07").join("02");
        fs::create_dir_all(&day).unwrap();
        fs::write(day.join("rollout-abc.jsonl"), "").unwrap();
        fs::write(day.join("other.jsonl"), "").unwrap();
        fs::write(dir.join("rollout-top.jsonl"), "").unwrap();

        let mut got = list_session_files(&dir, "jsonl", true, Some("rollout-"));
        got.sort();
        assert_eq!(
            got,
            vec![day.join("rollout-abc.jsonl"), dir.join("rollout-top.jsonl")]
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn list_session_files_missing_dir_is_empty() {
        let dir = std::env::temp_dir().join("ralphy-sf-does-not-exist-xyz");
        let _ = fs::remove_dir_all(&dir);
        assert!(list_session_files(&dir, "jsonl", false, None).is_empty());
    }
}
