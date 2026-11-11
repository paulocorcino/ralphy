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
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

/// What a change means to a user. A closed set: the gate refuses anything else,
/// and this is the only severity the workbench has while the project ships
/// release candidates (ADR-0056 §7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Kind {
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

    /// The heading this kind lands under on the release page. The daemon's
    /// `parse_body` strips the leading symbol to read the kind back (§10).
    fn page_heading(self) -> &'static str {
        match self {
            Kind::Breaking => "⚠️ Breaking",
            Kind::Security => "🔒 Security",
            Kind::Feature => "✨ New",
            Kind::Fix => "🐛 Fixed",
            Kind::Internal => "Internal",
        }
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
pub(crate) struct Entry {
    /// The fragment's file stem — the pull request or issue number when there is
    /// one, otherwise any slug the author chose.
    pub id: String,
    pub kind: Kind,
    pub text: String,
    /// Groups one capability's fragments into a single line on the release
    /// page (ADR-0056 §10).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub topic: Option<String>,
    /// The release page's line for this entry; the prose's first sentence when
    /// absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub headline: Option<String>,
}

/// Prose cap: one line. At 280 the record's median entry was 196 characters
/// (ADR-0056 §10, amended 2026-09-30).
pub(crate) const MAX_PROSE_CHARS: usize = 140;
/// Headline cap: one line on the release page.
pub(crate) const MAX_HEADLINE_CHARS: usize = 100;

/// One published release and everything it carried.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ReleaseRecord {
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
pub(crate) struct History {
    pub releases: Vec<ReleaseRecord>,
}

/// Parse one fragment: a `---`-delimited head carrying `kind:`, then the prose.
///
/// `id` is the caller's (the file stem), so this stays a pure function of the
/// text and is testable without a filesystem.
pub(crate) fn parse_fragment(id: &str, text: &str) -> Result<Entry> {
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

    let text = prose.trim().to_owned();
    if text.is_empty() {
        bail!("fragment has no prose: a kind alone says nothing to a reader");
    }
    let prose_len = text.chars().count();
    if prose_len > MAX_PROSE_CHARS {
        bail!(
            "fragment prose is {prose_len} characters, over the {MAX_PROSE_CHARS} cap: \
             say what the reader can now do, and leave the mechanism to the pull request"
        );
    }

    let optional = |key: &str| {
        fields
            .get(key)
            .map(ToString::to_string)
            .filter(|v| !v.is_empty())
    };
    let headline = optional("headline");
    if let Some(len) = headline.as_ref().map(|h| h.chars().count()) {
        if len > MAX_HEADLINE_CHARS {
            bail!("headline is {len} characters, over the {MAX_HEADLINE_CHARS} cap");
        }
    }

    Ok(Entry {
        id: id.to_owned(),
        kind,
        text,
        topic: optional("topic"),
        headline,
    })
}

/// Read every fragment in `dir`, ordered by number so the fold is deterministic.
/// A missing directory is an empty list, not an error.
pub(crate) fn read_fragments(dir: &Path) -> Result<Vec<Entry>> {
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
            .to_owned();
        (stem.parse::<u64>().unwrap_or(u64::MAX), stem)
    });

    paths
        .iter()
        .map(|path| {
            let stem = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or_default()
                .to_owned();
            let text = std::fs::read_to_string(path)
                .with_context(|| format!("reading {}", path.display()))?;
            parse_fragment(&stem, &text).with_context(|| format!("in {}", path.display()))
        })
        .collect()
}

/// Fold `entries` into `history` as `version`, dated `date`. Internal entries are
/// dropped: they were written to satisfy the gate, not to be read.
pub(crate) fn fold(history: &mut History, version: &str, date: &str, entries: Vec<Entry>) {
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
        version: version.to_owned(),
        date: date.to_owned(),
        announce,
        entries: kept,
    };
    history.releases.retain(|r| r.version != record.version);
    history.releases.insert(0, record);
}

/// Render the whole history as `CHANGELOG.md`.
pub(crate) fn render_changelog(history: &History) -> String {
    let mut out = String::from(
        "# Changelog\n\n\
         Generated by `cargo run -p xtask -- changelog --release <version>`, which folds\n\
         `changelog.d/*.md` into it. Edit a fragment, never this file (ADR-0056 §2).\n\
         \n\
         Releases cut before the first entry below predate this record and are on the\n\
         [Releases page](https://github.com/paulocorcino/ralphy/releases).\n",
    );
    for record in &history.releases {
        write!(out, "\n## {} — {}\n", record.version, record.date).expect(crate::WRITE_TO_STRING);
        out.push_str(&render_entries(&record.entries));
    }
    out
}

/// Render one release as its page: one line per capability, a feature's own
/// fixes left to the record (ADR-0056 §10).
pub(crate) fn render_notes(record: &ReleaseRecord) -> String {
    if record.entries.is_empty() {
        return "No user-visible changes in this release.\n".to_owned();
    }
    let launched = launched_topics(&record.entries);

    let mut out = String::new();
    let mut current: Option<Kind> = None;
    let mut seen: Vec<(Kind, &str)> = Vec::new();
    for (i, entry) in record.entries.iter().enumerate() {
        if is_repeat(entry, &launched, &mut seen) {
            continue;
        }
        let source = topic_source(&record.entries[i..], entry);
        if current != Some(entry.kind) {
            write!(out, "\n### {}\n\n", entry.kind.page_heading()).expect(crate::WRITE_TO_STRING);
            current = Some(entry.kind);
        }
        out.push_str(&bullet(&notes_line(source), &source.id));
    }
    out
}

/// The topics of the features in a release.
fn launched_topics(entries: &[Entry]) -> Vec<&str> {
    entries
        .iter()
        .filter(|e| e.kind == Kind::Feature)
        .filter_map(|e| e.topic.as_deref())
        .collect()
}

/// Does the page already have a line for this entry's topic? A fix of a
/// launched topic has none of its own. An entry with a topic that the page does
/// not have yet is recorded in `seen`.
fn is_repeat<'a>(entry: &'a Entry, launched: &[&str], seen: &mut Vec<(Kind, &'a str)>) -> bool {
    let Some(topic) = entry.topic.as_deref() else {
        return false;
    };
    if entry.kind == Kind::Fix && launched.contains(&topic) {
        return true;
    }
    if seen.contains(&(entry.kind, topic)) {
        return true;
    }
    seen.push((entry.kind, topic));
    false
}

/// The entry whose line stands for `entry`'s topic: the first headline any of
/// its fragments in `rest` carries, else `entry` itself.
fn topic_source<'a>(rest: &'a [Entry], entry: &'a Entry) -> &'a Entry {
    match entry.topic.as_deref() {
        Some(topic) => rest
            .iter()
            .filter(|e| e.kind == entry.kind && e.topic.as_deref() == Some(topic))
            .find(|e| e.headline.is_some())
            .unwrap_or(entry),
        None => entry,
    }
}

/// The page line of an entry: its headline, else the first sentence of its prose.
fn notes_line(source: &Entry) -> String {
    match source.headline.as_deref() {
        Some(headline) => headline.to_owned(),
        None => first_sentence(&source.text),
    }
}

fn render_entries(entries: &[Entry]) -> String {
    let mut out = String::new();
    let mut current: Option<Kind> = None;
    for entry in entries {
        if current != Some(entry.kind) {
            write!(out, "\n### {}\n\n", entry.kind.heading()).expect(crate::WRITE_TO_STRING);
            current = Some(entry.kind);
        }
        out.push_str(&bullet(&entry.text, &entry.id));
    }
    out
}

/// One fragment may be several lines; it stays one bullet. A numeric stem is a
/// pull request and renders as a link; a slug names nothing GitHub can resolve,
/// so it is left off rather than faked.
fn bullet(text: &str, id: &str) -> String {
    let text = text.replace('\n', " ");
    if !id.is_empty() && id.chars().all(|c| c.is_ascii_digit()) {
        format!("- {} (#{id})\n", text.trim())
    } else {
        format!("- {}\n", text.trim())
    }
}

/// The prose up to its first sentence break, for an entry with no headline.
fn first_sentence(text: &str) -> String {
    let text = text.replace('\n', " ");
    match text.split_once(". ") {
        Some((head, _)) => format!("{}.", head.trim()),
        None => text.trim().to_owned(),
    }
}

/// Load the history, treating an absent file as an empty one — the record starts
/// where the discipline starts.
pub(crate) fn load_history(path: &Path) -> Result<History> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Ok(History::default());
    };
    serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))
}

#[cfg(test)]
mod tests;
