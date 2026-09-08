//! `changelog` — fold the per-pull-request fragments into the release record
//! (ADR-0056 §2).
//!
//! `changelog.d/<n>.md` is human-owned: one file per pull request, written by
//! whoever wrote the change. `changelog.json` and `CHANGELOG.md` are
//! machine-owned: this task rewrites them wholesale from the accumulated
//! history plus the fragments it is consuming, and nothing else ever writes
//! them (ADR-0034 A3, one owner per file).
//!
//! `changelog.json` is the durable structured record; `CHANGELOG.md` is
//! rendered from it, so the markdown is never parsed back and can be restyled
//! without risking the history.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

/// What a change means to a user. A closed set: the gate refuses anything else,
/// and this is the only severity the workbench has while the project ships
/// release candidates (ADR-0056 §7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Breaking,
    Security,
    Feature,
    Fix,
    /// Changes nothing a user can see. Recorded by the author to satisfy the
    /// gate, then dropped by the fold — it has nothing to say in a release.
    Internal,
}

impl Kind {
    fn parse(s: &str) -> Result<Self> {
        Ok(match s.trim() {
            "breaking" => Kind::Breaking,
            "security" => Kind::Security,
            "feature" => Kind::Feature,
            "fix" => Kind::Fix,
            "internal" => Kind::Internal,
            other => bail!(
                "unknown kind `{other}` (expected breaking, security, feature, fix or internal)"
            ),
        })
    }

    /// The heading this kind lands under in the rendered changelog.
    fn heading(self) -> &'static str {
        match self {
            Kind::Breaking => "Breaking",
            Kind::Security => "Security",
            Kind::Feature => "New",
            Kind::Fix => "Fixed",
            Kind::Internal => "Internal",
        }
    }

    /// Whether a release carrying this kind is worth announcing. Fixes alone are
    /// not: at a release every few days, announcing all of them trains the
    /// audience to ignore all of them (ADR-0056 §7).
    fn announceable(self) -> bool {
        matches!(self, Kind::Breaking | Kind::Security | Kind::Feature)
    }
}

/// One fragment, parsed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    /// The fragment's file stem — the pull request or issue number when there is
    /// one, otherwise any slug the author chose.
    pub id: String,
    pub kind: Kind,
    pub text: String,
}

/// One published release and everything it carried.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReleaseRecord {
    pub version: String,
    pub date: String,
    /// True when this release carried a feature, a breaking change or a security
    /// fix — the flag the release workflow reads to decide whether to open a
    /// discussion.
    pub announce: bool,
    pub entries: Vec<Entry>,
}

/// The accumulated record, newest first.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct History {
    pub releases: Vec<ReleaseRecord>,
}

/// Parse one fragment: a `---`-delimited head carrying `kind:`, then the prose.
///
/// `id` is the caller's (the file stem), so this stays a pure function of the
/// text and is testable without a filesystem.
pub fn parse_fragment(id: &str, text: &str) -> Result<Entry> {
    let normalized = text.replace("\r\n", "\n");
    let body = normalized
        .strip_prefix("---\n")
        .context("fragment must open with a `---` front-matter fence")?;
    let (head, prose) = body
        .split_once("\n---\n")
        .context("fragment front matter must be closed by a `---` line")?;

    let mut fields: BTreeMap<&str, &str> = BTreeMap::new();
    for line in head.lines().filter(|l| !l.trim().is_empty()) {
        let (key, value) = line
            .split_once(':')
            .with_context(|| format!("front-matter line is not `key: value`: {line}"))?;
        fields.insert(key.trim(), value.trim());
    }

    let kind = Kind::parse(
        fields
            .get("kind")
            .context("fragment front matter must carry `kind:`")?,
    )?;

    let text = prose.trim().to_string();
    if text.is_empty() {
        bail!("fragment has no prose: a kind alone says nothing to a reader");
    }

    Ok(Entry {
        id: id.to_string(),
        kind,
        text,
    })
}

/// Read every fragment in `dir`, ordered by number so the fold is deterministic.
/// A missing directory is an empty list, not an error.
pub fn read_fragments(dir: &Path) -> Result<Vec<Entry>> {
    let Ok(read_dir) = std::fs::read_dir(dir) else {
        return Ok(Vec::new());
    };

    let mut paths: Vec<PathBuf> = read_dir
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "md"))
        .filter(|p| p.file_stem().is_some_and(|s| s != "README"))
        .collect();
    // Numeric where the stem is a number, lexical otherwise: `10` after `9`.
    paths.sort_by_key(|p| {
        let stem = p
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_string();
        (stem.parse::<u64>().unwrap_or(u64::MAX), stem)
    });

    paths
        .iter()
        .map(|path| {
            let stem = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or_default()
                .to_string();
            let text = std::fs::read_to_string(path)
                .with_context(|| format!("reading {}", path.display()))?;
            parse_fragment(&stem, &text).with_context(|| format!("in {}", path.display()))
        })
        .collect()
}

/// Fold `entries` into `history` as `version`, dated `date`. Internal entries are
/// dropped: they were written to satisfy the gate, not to be read.
pub fn fold(history: &mut History, version: &str, date: &str, entries: Vec<Entry>) {
    let mut kept: Vec<Entry> = entries
        .into_iter()
        .filter(|e| e.kind != Kind::Internal)
        .collect();
    kept.sort_by(|a, b| {
        a.kind
            .cmp(&b.kind)
            .then_with(|| {
                a.id.parse::<u64>()
                    .unwrap_or(u64::MAX)
                    .cmp(&b.id.parse::<u64>().unwrap_or(u64::MAX))
            })
            .then_with(|| a.id.cmp(&b.id))
    });

    let announce = kept.iter().any(|e| e.kind.announceable());
    let record = ReleaseRecord {
        version: version.to_string(),
        date: date.to_string(),
        announce,
        entries: kept,
    };
    history.releases.retain(|r| r.version != record.version);
    history.releases.insert(0, record);
}

/// Render the whole history as `CHANGELOG.md`.
pub fn render_changelog(history: &History) -> String {
    let mut out = String::from(
        "# Changelog\n\n\
         Generated by `cargo run -p xtask -- changelog --release <version>`, which folds\n\
         `changelog.d/*.md` into it. Edit a fragment, never this file (ADR-0056 §2).\n\
         \n\
         Releases cut before the first entry below predate this record and are on the\n\
         [Releases page](https://github.com/paulocorcino/ralphy/releases).\n",
    );
    for record in &history.releases {
        out.push_str(&format!("\n## {} — {}\n", record.version, record.date));
        out.push_str(&render_entries(&record.entries));
    }
    out
}

/// Render one release's entries as the body of a release note.
pub fn render_notes(record: &ReleaseRecord) -> String {
    if record.entries.is_empty() {
        return "No user-visible changes in this release.\n".to_string();
    }
    render_entries(&record.entries)
}

fn render_entries(entries: &[Entry]) -> String {
    let mut out = String::new();
    let mut current: Option<Kind> = None;
    for entry in entries {
        if current != Some(entry.kind) {
            out.push_str(&format!("\n### {}\n\n", entry.kind.heading()));
            current = Some(entry.kind);
        }
        // One fragment may be several sentences; keep it on one bullet.
        let text = entry.text.replace('\n', " ");
        // A numeric stem is a pull request and renders as a link; a slug names
        // nothing GitHub can resolve, so it is left off rather than faked.
        if !entry.id.is_empty() && entry.id.chars().all(|c| c.is_ascii_digit()) {
            out.push_str(&format!("- {} (#{})\n", text.trim(), entry.id));
        } else {
            out.push_str(&format!("- {}\n", text.trim()));
        }
    }
    out
}

/// Load the history, treating an absent file as an empty one — the record starts
/// where the discipline starts.
pub fn load_history(path: &Path) -> Result<History> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Ok(History::default());
    };
    serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fragment(kind: &str, prose: &str) -> String {
        format!("---\nkind: {kind}\n---\n{prose}\n")
    }

    #[test]
    fn a_fragment_is_its_kind_and_its_prose() {
        let entry = parse_fragment(
            "389",
            &fragment("feature", "Paste a screenshot into a console."),
        )
        .expect("a well-formed fragment must parse");
        assert_eq!(entry.kind, Kind::Feature);
        assert_eq!(entry.text, "Paste a screenshot into a console.");
        assert_eq!(entry.id, "389");
    }

    #[test]
    fn windows_line_endings_parse() {
        let entry = parse_fragment(
            "1",
            "---\r\nkind: fix\r\n---\r\nIt no longer does that.\r\n",
        )
        .expect("CRLF is what a Windows editor writes");
        assert_eq!(entry.kind, Kind::Fix);
        assert_eq!(entry.text, "It no longer does that.");
    }

    #[test]
    fn an_unknown_kind_is_refused_by_name() {
        let err =
            parse_fragment("1", &fragment("chore", "Something.")).expect_err("chore is not a kind");
        assert!(err.to_string().contains("chore"), "{err}");
    }

    #[test]
    fn a_fragment_with_no_prose_is_refused() {
        let err =
            parse_fragment("1", "---\nkind: fix\n---\n\n").expect_err("a kind alone says nothing");
        assert!(err.to_string().contains("no prose"), "{err}");
    }

    #[test]
    fn a_fragment_with_no_front_matter_is_refused() {
        assert!(parse_fragment("1", "Just some prose.\n").is_err());
        assert!(parse_fragment("1", "---\nkind: fix\nnever closed\n").is_err());
    }

    #[test]
    fn internal_entries_are_consumed_and_never_rendered() {
        let mut history = History::default();
        fold(
            &mut history,
            "v0.1.0-rc.20",
            "2026-09-08",
            vec![
                parse_fragment("1", &fragment("internal", "Split a module.")).expect("parse"),
                parse_fragment(
                    "2",
                    &fragment("fix", "The keyboard stops covering the prompt."),
                )
                .expect("parse"),
            ],
        );
        let record = &history.releases[0];
        assert_eq!(record.entries.len(), 1, "internal must not be recorded");
        assert_eq!(record.entries[0].kind, Kind::Fix);
        assert!(!render_changelog(&history).contains("Split a module"));
    }

    #[test]
    fn only_a_feature_grade_release_is_announceable() {
        let mut history = History::default();
        fold(
            &mut history,
            "v1",
            "2026-09-08",
            vec![parse_fragment("1", &fragment("fix", "A fix.")).expect("parse")],
        );
        assert!(!history.releases[0].announce, "fixes alone are not news");

        fold(
            &mut history,
            "v2",
            "2026-09-09",
            vec![parse_fragment("2", &fragment("feature", "A feature.")).expect("parse")],
        );
        assert!(history.releases[0].announce);
    }

    #[test]
    fn a_release_is_rendered_breaking_first_and_fixes_last() {
        let mut history = History::default();
        fold(
            &mut history,
            "v0.1.0-rc.20",
            "2026-09-08",
            vec![
                parse_fragment("3", &fragment("fix", "A fix.")).expect("parse"),
                parse_fragment("1", &fragment("breaking", "A break.")).expect("parse"),
                parse_fragment("2", &fragment("feature", "A feature.")).expect("parse"),
            ],
        );
        let text = render_changelog(&history);
        let at = |needle: &str| {
            text.find(needle)
                .unwrap_or_else(|| panic!("{needle} missing"))
        };
        assert!(at("### Breaking") < at("### New"));
        assert!(at("### New") < at("### Fixed"));
        assert!(text.contains("- A break. (#1)"));
        assert!(text.contains("## v0.1.0-rc.20 — 2026-09-08"));
    }

    #[test]
    fn the_newest_release_is_rendered_first() {
        let mut history = History::default();
        fold(
            &mut history,
            "v0.1.0-rc.20",
            "2026-09-08",
            vec![parse_fragment("1", &fragment("fix", "Older.")).expect("parse")],
        );
        fold(
            &mut history,
            "v0.1.0-rc.21",
            "2026-09-09",
            vec![parse_fragment("2", &fragment("fix", "Newer.")).expect("parse")],
        );
        let text = render_changelog(&history);
        assert!(text.find("rc.21").expect("rc.21") < text.find("rc.20").expect("rc.20"));
    }

    #[test]
    fn refolding_a_version_replaces_it_rather_than_duplicating_it() {
        let mut history = History::default();
        for prose in ["First attempt.", "Second attempt."] {
            fold(
                &mut history,
                "v0.1.0-rc.20",
                "2026-09-08",
                vec![parse_fragment("1", &fragment("fix", prose)).expect("parse")],
            );
        }
        assert_eq!(history.releases.len(), 1);
        assert_eq!(history.releases[0].entries[0].text, "Second attempt.");
    }

    #[test]
    fn a_release_with_nothing_user_visible_says_so_in_its_notes() {
        let mut history = History::default();
        fold(
            &mut history,
            "v0.1.0-rc.20",
            "2026-09-08",
            vec![parse_fragment("1", &fragment("internal", "Split a module.")).expect("parse")],
        );
        assert_eq!(
            render_notes(&history.releases[0]),
            "No user-visible changes in this release.\n"
        );
    }

    #[test]
    fn a_multi_line_fragment_stays_one_bullet() {
        let mut history = History::default();
        fold(
            &mut history,
            "v1",
            "2026-09-08",
            vec![
                parse_fragment("1", &fragment("feature", "One sentence.\nAnd another."))
                    .expect("parse"),
            ],
        );
        let text = render_notes(&history.releases[0]);
        assert!(text.contains("- One sentence. And another. (#1)"), "{text}");
    }

    #[test]
    fn a_slug_named_fragment_does_not_fake_a_pull_request_link() {
        let mut history = History::default();
        fold(
            &mut history,
            "v1",
            "2026-09-08",
            vec![
                parse_fragment("release-watch", &fragment("feature", "A feature.")).expect("parse"),
            ],
        );
        let text = render_notes(&history.releases[0]);
        assert_eq!(text.trim_end().lines().last(), Some("- A feature."));
        assert!(!text.contains("(#"), "no issue number was claimed: {text}");
    }

    #[test]
    fn the_history_round_trips_through_json() {
        let mut history = History::default();
        fold(
            &mut history,
            "v0.1.0-rc.20",
            "2026-09-08",
            vec![parse_fragment("1", &fragment("feature", "A feature.")).expect("parse")],
        );
        let json = serde_json::to_string_pretty(&history).expect("serialize");
        let back: History = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, history);
        assert!(json.contains("\"kind\": \"feature\""), "{json}");
    }
}
