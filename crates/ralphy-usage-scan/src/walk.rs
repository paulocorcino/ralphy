//! The recursive walk that lists a vendor's session files.

use std::fs;
use std::path::{Path, PathBuf};

/// Every file under `dir`, recursively, that `keep` accepts. Tolerant: a
/// missing or unreadable folder yields nothing. Order is unspecified (each file
/// is one independent session).
pub(crate) fn files_under(dir: &Path, keep: impl Fn(&Path) -> bool) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = fs::read_dir(&d) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if keep(&path) {
                out.push(path);
            }
        }
    }
    out
}

/// `true` for a `*.jsonl` file: the Claude and Codex session logs.
pub(crate) fn is_jsonl(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()) == Some("jsonl")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_under_walks_nested_folders_and_keeps_only_matches() {
        let dir = tempfile::tempdir().expect("a temp dir is writable");
        let deep = dir.path().join("2026").join("10");
        fs::create_dir_all(&deep).expect("the folder is created");
        fs::write(dir.path().join("top.jsonl"), "").expect("written");
        fs::write(deep.join("deep.jsonl"), "").expect("written");
        fs::write(deep.join("notes.txt"), "").expect("written");

        let mut found = files_under(dir.path(), is_jsonl);
        found.sort();
        assert_eq!(
            found,
            [deep.join("deep.jsonl"), dir.path().join("top.jsonl")]
        );
        assert!(files_under(&dir.path().join("missing"), is_jsonl).is_empty());
    }
}
