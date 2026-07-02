//! The knowledge cache's consolidation bookkeeping: which per-issue notes are
//! still loose (not yet folded into `KNOWLEDGE.md`), the structural gate a
//! freshly consolidated file must pass, and archiving the folded notes into
//! `knowledge/raw/` after a consolidation session succeeds. The consolidation
//! *content* is an agent's judgment (see `prompt.consolidate.md`); this module
//! owns only the deterministic checks and file moves around it.

use std::fs;
use std::io::Write;
use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

use crate::Workspace;

/// How many green closes without a citation make a `KNOWLEDGE.md` bullet a
/// pruning candidate. The consolidation prompt states this window in prose;
/// the contract test pins the two against each other.
pub const CITATION_PRUNE_WINDOW: usize = 5;

/// One green close's `**Knowledge used**` report — a line of the append-only
/// `.ralphy/knowledge/citations.jsonl` hit-rate log. `citations` loosely quote
/// the `KNOWLEDGE.md` / `handoffs.md` bullets the session relied on; an empty
/// list is an honest `none` and still counts toward the pruning window.
#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub struct CitationEntry {
    pub issue: u64,
    pub stamp: String,
    pub date: String,
    pub citations: Vec<String>,
}

/// Append one entry to `.ralphy/knowledge/citations.jsonl` as a single JSON
/// line, creating the directory and file on first use. The file is never
/// archived or cleared — unlike `issue-<N>.md` notes it must survive
/// consolidations, or the cross-run "never cited" judgment loses its history.
pub fn append_citation(ws: &Workspace, entry: &CitationEntry) -> Result<()> {
    fs::create_dir_all(ws.knowledge_dir()).context("creating .ralphy/knowledge")?;
    let mut line = serde_json::to_string(entry).context("serializing citation entry")?;
    line.push('\n');
    fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(ws.citations_path())
        .context("opening citations.jsonl")?
        .write_all(line.as_bytes())
        .context("appending to citations.jsonl")
}

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

/// Hard cap on the curated `KNOWLEDGE.md` line count. The consolidation prompt
/// budgets ~150 lines; the gate allows some slack over that budget but rejects
/// a file that ignored it wholesale.
pub const KNOWLEDGE_LINE_CAP: usize = 200;

/// The structural gate on a freshly consolidated `KNOWLEDGE.md`: the session's
/// output is accepted only when it still has the shape `prompt.consolidate.md`
/// mandates — title, topic headings, provenance markers, under the line cap.
/// Returns the issue numbers from the `<!-- folded: ... -->` marker (the notes
/// the session declared folded in, the runner's archive contract), or an error
/// naming the first violated invariant so the caller can reject the file and
/// keep every note loose for a retry.
pub fn validate_knowledge(knowledge: &str) -> Result<Vec<u64>> {
    if knowledge.trim().is_empty() {
        bail!("file is empty");
    }
    let lines = knowledge.lines().count();
    if lines > KNOWLEDGE_LINE_CAP {
        bail!("file is {lines} lines, over the {KNOWLEDGE_LINE_CAP}-line cap");
    }
    let first = knowledge
        .lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or_default();
    if !first.starts_with("# KNOWLEDGE") {
        bail!("missing the `# KNOWLEDGE` title heading");
    }
    if !knowledge.lines().any(|l| l.starts_with("## ")) {
        bail!("no `## <topic>` headings");
    }
    if !has_provenance(knowledge) {
        bail!("no `(#<issue> ...)` provenance markers");
    }
    let Some(marker) = knowledge
        .lines()
        .rev()
        .map(str::trim)
        .find(|l| l.starts_with(FOLDED_PREFIX))
    else {
        bail!("missing the `{FOLDED_PREFIX} ... -->` marker line");
    };
    parse_folded_marker(marker).with_context(|| format!("malformed folded marker: `{marker}`"))
}

/// The line prefix of the folded-list contract between `prompt.consolidate.md`
/// and the runner: `<!-- folded: #3, #16 -->` (or `<!-- folded: none -->`).
const FOLDED_PREFIX: &str = "<!-- folded:";

/// The issue numbers of a `<!-- folded: #3, #16 -->` marker line, `Some(vec![])`
/// for the explicit `none`, and `None` for anything malformed — an unparseable
/// claim must reject the whole file, not silently archive nothing.
fn parse_folded_marker(line: &str) -> Option<Vec<u64>> {
    let body = line
        .strip_prefix(FOLDED_PREFIX)?
        .strip_suffix("-->")?
        .trim();
    if body.eq_ignore_ascii_case("none") {
        return Some(Vec::new());
    }
    let numbers: Option<Vec<u64>> = body
        .split(',')
        .map(|tok| tok.trim().strip_prefix('#')?.parse().ok())
        .collect();
    numbers.filter(|n| !n.is_empty())
}

/// Whether any `(#<digit>` provenance marker appears — every curated bullet
/// must cite the issues it came from, so a file without a single one is not a
/// consolidation output.
fn has_provenance(knowledge: &str) -> bool {
    knowledge
        .match_indices("(#")
        .any(|(i, m)| knowledge[i + m.len()..].starts_with(|c: char| c.is_ascii_digit()))
}

/// Split the loose notes into (folded → archive, unfolded → keep loose) using
/// the issue numbers the session declared folded. Numbers in `folded` without
/// a matching loose note (already archived by an earlier pass) are ignored.
pub fn partition_folded(notes: &[PathBuf], folded: &[u64]) -> (Vec<PathBuf>, Vec<PathBuf>) {
    notes.iter().cloned().partition(|note| {
        note.file_name()
            .and_then(|n| n.to_str())
            .and_then(note_number)
            .is_some_and(|n| folded.contains(&n))
    })
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
        // Neither the curated file, a stray file, nor the citations log counts
        // as a loose note — citations.jsonl must never be archived or trigger
        // a consolidation by itself.
        fs::write(ws.knowledge_file(), "curated").unwrap();
        fs::write(ws.knowledge_dir().join("notes.txt"), "stray").unwrap();
        fs::write(ws.citations_path(), "{}\n").unwrap();

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

    /// A minimal file with the exact shape `prompt.consolidate.md` mandates,
    /// parameterized on the folded-marker body.
    fn sample_knowledge(folded: &str) -> String {
        format!(
            "# KNOWLEDGE — curated project knowledge\n\
             \n\
             Consolidated through issue #21. Leads, not truths.\n\
             \n\
             ## Toolchain & platform\n\
             - cargo test needs docker up first (#3, #16; 2026-06-11)\n\
             \n\
             ## Commands that work\n\
             ```\n\
             cargo test\n\
             ```\n\
             \n\
             <!-- folded: {folded} -->\n"
        )
    }

    #[test]
    fn validate_knowledge_accepts_well_formed_and_returns_folded_numbers() {
        let folded = validate_knowledge(&sample_knowledge("#3, #16, #21")).unwrap();
        assert_eq!(folded, vec![3, 16, 21]);
    }

    #[test]
    fn validate_knowledge_folded_none_is_empty_list() {
        assert_eq!(
            validate_knowledge(&sample_knowledge("none")).unwrap(),
            Vec::<u64>::new()
        );
    }

    #[test]
    fn validate_knowledge_rejects_structural_violations() {
        // Each case is one invariant of the gate; the substring pins the reason
        // so a rejection names what actually broke.
        let over_cap = format!("{}{}", sample_knowledge("#3"), "- filler\n".repeat(250));
        let cases: Vec<(String, &str)> = vec![
            (String::new(), "empty"),
            ("   \n\n".into(), "empty"),
            (over_cap, "line cap"),
            (
                sample_knowledge("#3").replace("# KNOWLEDGE", "# NOTES"),
                "title",
            ),
            (sample_knowledge("#3").replace("## ", "### "), "headings"),
            (
                sample_knowledge("#3").replace("(#3, #16; 2026-06-11)", ""),
                "provenance",
            ),
            (
                sample_knowledge("#3").replace("<!-- folded: #3 -->", ""),
                "marker line",
            ),
            (sample_knowledge(""), "malformed"),
            (sample_knowledge("#3, oops"), "malformed"),
            (sample_knowledge("3, 16"), "malformed"),
        ];
        for (input, reason) in cases {
            let err = validate_knowledge(&input).unwrap_err().to_string();
            assert!(err.contains(reason), "expected `{reason}` in `{err}`");
        }
    }

    #[test]
    fn prompt_and_parser_agree_on_the_folded_list_contract() {
        // The consolidation prompt shows the exact marker the runner parses;
        // this pins the two sides of the contract together.
        let prompt = include_str!("../../../assets/prompts/prompt.consolidate.md");
        assert!(
            prompt.contains("<!-- folded: #3, #16, #21 -->"),
            "prompt.consolidate.md must show the folded marker the parser accepts"
        );
        assert_eq!(
            parse_folded_marker("<!-- folded: #3, #16, #21 -->"),
            Some(vec![3, 16, 21])
        );
        assert!(prompt.contains("<!-- folded: none -->"));
        assert_eq!(
            parse_folded_marker("<!-- folded: none -->"),
            Some(Vec::new())
        );
    }

    #[test]
    fn partition_folded_splits_archive_from_leftover() {
        let notes = vec![
            PathBuf::from("issue-3.md"),
            PathBuf::from("issue-16.md"),
            PathBuf::from("issue-21.md"),
        ];
        // #99 has no loose note (already archived earlier) and is ignored.
        let (archive, leftover) = partition_folded(&notes, &[3, 21, 99]);
        assert_eq!(
            archive,
            vec![PathBuf::from("issue-3.md"), PathBuf::from("issue-21.md")]
        );
        assert_eq!(leftover, vec![PathBuf::from("issue-16.md")]);
    }

    #[test]
    fn append_citation_accumulates_one_json_line_per_close() {
        let ws = temp_ws("citations");
        let first = CitationEntry {
            issue: 3,
            stamp: "run-a".into(),
            date: "2026-06-11".into(),
            citations: vec!["cargo test needs docker up first".into()],
        };
        // An honest `none` close is recorded too — it is the denominator of
        // the pruning window.
        let second = CitationEntry {
            issue: 5,
            stamp: "run-b".into(),
            date: "2026-06-12".into(),
            citations: Vec::new(),
        };
        append_citation(&ws, &first).unwrap();
        append_citation(&ws, &second).unwrap();

        let content = fs::read_to_string(ws.citations_path()).unwrap();
        let entries: Vec<CitationEntry> = content
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(entries, vec![first, second]);

        let _ = fs::remove_dir_all(ws.repo_root());
    }

    #[test]
    fn prompt_and_citation_log_agree_on_the_jsonl_contract() {
        // The consolidation prompt shows the exact JSON line `append_citation`
        // writes and states the pruning window; this pins prompt and code
        // together so neither can drift alone.
        let prompt = include_str!("../../../assets/prompts/prompt.consolidate.md");
        let entry = CitationEntry {
            issue: 3,
            stamp: "run-stamp".into(),
            date: "2026-06-11".into(),
            citations: vec!["cargo test needs docker up first".into()],
        };
        let line = serde_json::to_string(&entry).unwrap();
        assert!(
            prompt.contains(&line),
            "prompt.consolidate.md must show the exact JSON line append_citation writes: {line}"
        );
        assert!(
            prompt.contains(&format!("most recent {CITATION_PRUNE_WINDOW} entries")),
            "prompt must state the pruning window"
        );
        assert!(
            prompt.contains(&format!("fewer than {CITATION_PRUNE_WINDOW} entries")),
            "prompt must state the too-young skip clause"
        );
    }

    #[test]
    fn archive_notes_empty_is_zero_and_creates_nothing() {
        let ws = temp_ws("noop");
        assert_eq!(archive_notes(&ws, &[]).unwrap(), 0);
        assert!(!ws.knowledge_raw_dir().exists());
        let _ = fs::remove_dir_all(ws.repo_root());
    }
}
