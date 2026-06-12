//! The knowledge cache's consolidation bookkeeping: which per-issue notes are
//! still loose (not yet folded into `KNOWLEDGE.md`), and archiving them into
//! `knowledge/raw/` after a consolidation session succeeds. The consolidation
//! *content* is an agent's judgment (see `prompt.consolidate.md`); this module
//! owns only the deterministic file moves around it.

use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};

use crate::Workspace;

/// The loose per-issue notes under `.ralphy/knowledge/` — `issue-<N>.md` files
/// not yet archived into `raw/`. Sorted by issue number so callers report them
/// stably. Empty when the directory is absent.
pub fn loose_notes(ws: &Workspace) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(ws.knowledge_dir()) else {
        return Vec::new();
    };
    let mut notes: Vec<(u64, PathBuf)> = entries
        .flatten()
        .filter_map(|e| {
            let path = e.path();
            let number = note_number(path.file_name()?.to_str()?)?;
            path.is_file().then_some((number, path))
        })
        .collect();
    notes.sort_by_key(|(n, _)| *n);
    notes.into_iter().map(|(_, p)| p).collect()
}

/// The issue number of a `issue-<N>.md` file name, or `None` for anything else
/// (`KNOWLEDGE.md`, stray files) so only real notes are listed and archived.
fn note_number(file_name: &str) -> Option<u64> {
    file_name
        .strip_prefix("issue-")?
        .strip_suffix(".md")?
        .parse()
        .ok()
}

/// Move the given notes into `knowledge/raw/`, overwriting same-named archives
/// (a re-consolidated re-run supersedes its older raw copy). Returns how many
/// were archived. Callers invoke this only AFTER the consolidation session
/// produced `KNOWLEDGE.md`, so a failed session never strands notes in `raw/`.
pub fn archive_notes(ws: &Workspace, notes: &[PathBuf]) -> Result<usize> {
    if notes.is_empty() {
        return Ok(0);
    }
    let raw = ws.knowledge_raw_dir();
    fs::create_dir_all(&raw).context("creating .ralphy/knowledge/raw")?;
    for note in notes {
        let name = note
            .file_name()
            .with_context(|| format!("note path has no file name: {}", note.display()))?;
        let dest = raw.join(name);
        // `rename` fails on a pre-existing destination on some platforms;
        // remove first so the move is an overwrite either way.
        let _ = fs::remove_file(&dest);
        fs::rename(note, &dest)
            .with_context(|| format!("archiving {} into raw/", note.display()))?;
    }
    Ok(notes.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_ws(tag: &str) -> Workspace {
        let base =
            std::env::temp_dir().join(format!("ralphy-knowledge-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        Workspace::new(&base)
    }

    fn write_note(ws: &Workspace, number: u64, body: &str) {
        fs::create_dir_all(ws.knowledge_dir()).unwrap();
        fs::write(ws.knowledge_path(number), body).unwrap();
    }

    #[test]
    fn loose_notes_lists_only_issue_notes_sorted_by_number() {
        let ws = temp_ws("loose");
        write_note(&ws, 21, "a");
        write_note(&ws, 3, "b");
        write_note(&ws, 16, "c");
        // Neither the curated file nor a stray file counts as a loose note.
        fs::write(ws.knowledge_file(), "curated").unwrap();
        fs::write(ws.knowledge_dir().join("notes.txt"), "stray").unwrap();

        let notes = loose_notes(&ws);
        let names: Vec<_> = notes
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec!["issue-3.md", "issue-16.md", "issue-21.md"]);

        let _ = fs::remove_dir_all(ws.repo_root());
    }

    #[test]
    fn loose_notes_empty_without_knowledge_dir() {
        let ws = temp_ws("empty");
        assert!(loose_notes(&ws).is_empty());
        let _ = fs::remove_dir_all(ws.repo_root());
    }

    #[test]
    fn archive_notes_moves_into_raw_and_overwrites() {
        let ws = temp_ws("archive");
        write_note(&ws, 5, "fresh note");
        // A stale archive of the same note (from an earlier consolidation of a
        // re-run issue) must be overwritten, not error the move.
        fs::create_dir_all(ws.knowledge_raw_dir()).unwrap();
        fs::write(ws.knowledge_raw_dir().join("issue-5.md"), "stale").unwrap();

        let notes = loose_notes(&ws);
        let count = archive_notes(&ws, &notes).unwrap();
        assert_eq!(count, 1);
        assert!(!ws.knowledge_path(5).exists(), "loose note must be gone");
        assert_eq!(
            fs::read_to_string(ws.knowledge_raw_dir().join("issue-5.md")).unwrap(),
            "fresh note"
        );

        let _ = fs::remove_dir_all(ws.repo_root());
    }

    #[test]
    fn archive_notes_empty_is_zero_and_creates_nothing() {
        let ws = temp_ws("noop");
        assert_eq!(archive_notes(&ws, &[]).unwrap(), 0);
        assert!(!ws.knowledge_raw_dir().exists());
        let _ = fs::remove_dir_all(ws.repo_root());
    }
}
