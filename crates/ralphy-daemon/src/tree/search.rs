//! The two Observe searches over a confined working tree (ADR-0036, amendment
//! 2026-09-15): [`find`] matches entry NAMES with the tree's own policy, so it
//! finds exactly what the tree can show; [`grep`] matches file CONTENT and
//! consults `.gitignore` (except `.ralphy/`, always searched) — a search that
//! walks `.venv/` on every keystroke is noise, not search. Both are literal
//! and case-insensitive, and both stop at a [`SearchBudget`] because nothing
//! upstream of an Observe read caps it.

use std::path::Path;
use std::time::{Duration, Instant};

use encoding_rs::Encoding;

use crate::confine::{self, ConfineError};
use crate::textcodec;

use super::{is_not_noise, text_of_with, walker, MAX_READ_BYTES};

/// The name of the directory [`grep`] searches even when the repo ignores it:
/// the plan and the run logs live there, and they are what the operator is
/// most often looking for mid-run.
const ALWAYS_SEARCHED: &str = ".ralphy";

/// The shortest query worth a walk. The UI never sends less; the verb still
/// refuses so a stray one-character request cannot list the whole tree.
pub const MIN_QUERY_CHARS: usize = 2;

/// Where a search stops: after `max_hits` hits, or when `deadline` has elapsed
/// since the walk began. Either way the reply says `truncated`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SearchBudget {
    pub max_hits: usize,
    pub deadline: Duration,
}

impl Default for SearchBudget {
    /// The wire budget: 200 hits, five seconds. A search over a peer must not
    /// hold the relay, and 200 rows is more than a tree can usefully show.
    fn default() -> Self {
        Self {
            max_hits: 200,
            deadline: Duration::from_secs(5),
        }
    }
}

/// A [`find`] hit: the entry's `/`-joined path relative to the root.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct FindHit {
    pub path: String,
    pub dir: bool,
}

/// A [`grep`] hit: a text file and how many times the query occurs in it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct GrepHit {
    pub path: String,
    pub count: u32,
}

/// A search's answer: the hits it collected and whether it stopped early.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct SearchReply<H> {
    pub hits: Vec<H>,
    pub truncated: bool,
}

/// The clock a walk checks per entry; `expired` is sticky so the caller reads
/// one answer after the loop.
struct Clock {
    started: Instant,
    limit: Duration,
}

impl Clock {
    fn start(budget: &SearchBudget) -> Self {
        Self {
            started: Instant::now(),
            limit: budget.deadline,
        }
    }

    fn expired(&self) -> bool {
        self.started.elapsed() >= self.limit
    }
}

/// The relative, `/`-joined path of a walked entry — the shape the workbench's
/// `relPath` produces, so a hit can be revealed in the tree on any host.
fn rel_of(root: &Path, entry: &ignore::DirEntry) -> Option<String> {
    let rel = entry.path().strip_prefix(root).ok()?;
    let parts: Vec<String> = rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    if parts.is_empty() {
        return None;
    }
    Some(parts.join("/"))
}

/// Find the entries (files and directories) under `root` whose name contains
/// `query`, case-insensitively. Same walk policy as [`super::list`]: hidden and
/// gitignored entries included, only `super::HARD_EXCLUDE` dropped. Hits are
/// sorted directories first, then by path.
pub fn find(
    root: &Path,
    query: &str,
    budget: &SearchBudget,
) -> Result<SearchReply<FindHit>, ConfineError> {
    let dir = confine::confine(root, "")?;
    let needle = query.trim().to_lowercase();
    let mut reply = SearchReply {
        hits: Vec::new(),
        truncated: false,
    };
    if needle.chars().count() < MIN_QUERY_CHARS {
        return Ok(reply);
    }
    let clock = Clock::start(budget);
    for entry in walker(&dir).build().filter_map(Result::ok) {
        if entry.depth() == 0 {
            continue;
        }
        if clock.expired() {
            reply.truncated = true;
            break;
        }
        if !entry
            .file_name()
            .to_string_lossy()
            .to_lowercase()
            .contains(&needle)
        {
            continue;
        }
        if reply.hits.len() >= budget.max_hits {
            reply.truncated = true;
            break;
        }
        if let Some(path) = rel_of(&dir, &entry) {
            reply.hits.push(FindHit {
                path,
                dir: entry.file_type().map(|t| t.is_dir()).unwrap_or(false),
            });
        }
    }
    reply
        .hits
        .sort_by(|a, b| b.dir.cmp(&a.dir).then_with(|| a.path.cmp(&b.path)));
    Ok(reply)
}

/// Find the text files under `root` containing `query` (literal,
/// case-insensitive), with the occurrence count per file. Consults
/// `.gitignore`/`.git/info/exclude`/the global excludes — the one policy
/// divergence from the tree, decided on purpose — but always searches
/// `.ralphy/`, and never the `super::HARD_EXCLUDE` dirs. A file [`super::read`]
/// would refuse (binary, over [`MAX_READ_BYTES`]) is skipped, never a hit.
/// Decodes with the default fallback encoding; the verb uses [`grep_with`].
pub fn grep(
    root: &Path,
    query: &str,
    budget: &SearchBudget,
) -> Result<SearchReply<GrepHit>, ConfineError> {
    grep_with(root, query, budget, textcodec::DEFAULT_FALLBACK)
}

/// [`grep`] decoding non-UTF-8 files with the repo's `fallback`
/// ([`textcodec::fallback_for`]), so search sees exactly what the viewer sees.
pub fn grep_with(
    root: &Path,
    query: &str,
    budget: &SearchBudget,
    fallback: &'static Encoding,
) -> Result<SearchReply<GrepHit>, ConfineError> {
    let dir = confine::confine(root, "")?;
    let needle = query.trim();
    let mut reply = SearchReply {
        hits: Vec::new(),
        truncated: false,
    };
    if needle.chars().count() < MIN_QUERY_CHARS {
        return Ok(reply);
    }
    let Some(re) = literal_regex(needle) else {
        return Ok(reply);
    };
    let clock = Clock::start(budget);
    collect_hits(
        grep_walks(&dir),
        &dir,
        &re,
        fallback,
        budget,
        &clock,
        &mut reply,
    );
    reply.hits.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(reply)
}

/// `needle` as a literal, case-insensitive regex.
fn literal_regex(needle: &str) -> Option<regex::Regex> {
    // An escaped literal always compiles; the only error is a query so long
    // it trips the regex size limit, and an empty answer is the honest one
    // for that.
    regex::RegexBuilder::new(&regex::escape(needle))
        .case_insensitive(true)
        .build()
        .ok()
}

/// The walks [`grep`] searches under `dir`.
///
/// Two walks, not one override: the `ignore` crate's whitelist overrides
/// turn every NON-matching file into an ignored one, so `.ralphy/**` cannot
/// be whitelisted without hiding the rest of the repo. The main walk skips
/// the top-level `.ralphy` (whether or not the repo ignores it, so a hit is
/// never counted twice); the second walk owns it with every filter off.
fn grep_walks(dir: &Path) -> impl Iterator<Item = ignore::Walk> {
    let ralphy = dir.join(ALWAYS_SEARCHED);
    let mut main = ignore::WalkBuilder::new(dir);
    main.hidden(false)
        .git_ignore(true)
        .git_exclude(true)
        .git_global(true)
        // The policy holds with or without a `.git`: a `.gitignore` in a
        // not-yet-initialized checkout says the same thing.
        .require_git(false)
        .ignore(false)
        .parents(false)
        .filter_entry(move |e| is_not_noise(e) && !(e.depth() == 1 && e.path() == ralphy));
    let ralphy_walk = {
        let sub = dir.join(ALWAYS_SEARCHED);
        sub.is_dir().then(|| walker(&sub).build())
    };
    std::iter::once(main.build()).chain(ralphy_walk)
}

/// Push a hit into `reply` for each file of `walks` that contains `re`, until
/// the clock runs out or the hit cap is reached; either one sets `truncated`.
/// The cap is checked only for a file that is a hit, so a walk that ends
/// right at the cap is not `truncated`.
fn collect_hits(
    walks: impl Iterator<Item = ignore::Walk>,
    dir: &Path,
    re: &regex::Regex,
    fallback: &'static Encoding,
    budget: &SearchBudget,
    clock: &Clock,
    reply: &mut SearchReply<GrepHit>,
) {
    for entry in walks.flatten().filter_map(Result::ok) {
        if clock.expired() {
            reply.truncated = true;
            return;
        }
        if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
            continue;
        }
        let Some(count) = count_in(entry.path(), re, fallback) else {
            continue;
        };
        if reply.hits.len() >= budget.max_hits {
            reply.truncated = true;
            return;
        }
        reply
            .hits
            .extend(rel_of(dir, &entry).map(|path| GrepHit { path, count }));
    }
}

/// Occurrences of `re` in the file at `path`, or `None` when the file is not a
/// hit: unreadable, oversized, binary, or simply without a match.
fn count_in(path: &Path, re: &regex::Regex, fallback: &'static Encoding) -> Option<u32> {
    let meta = std::fs::metadata(path).ok()?;
    if meta.len() > MAX_READ_BYTES {
        return None;
    }
    let text = text_of_with(std::fs::read(path).ok()?, fallback)?;
    let count = re.find_iter(&text).count();
    (count > 0).then_some(u32::try_from(count).unwrap_or(u32::MAX))
}

#[cfg(test)]
mod tests;
