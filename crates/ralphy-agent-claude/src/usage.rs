//! Token-usage capture and transcript discovery (ADR-0008 D5/D10): parse the
//! `claude -p` plan stdout and the interactive session transcript JSONL into a
//! core [`Usage`], and locate the `~/.claude/projects/<dashed-cwd>` transcript
//! directory Claude writes for a given run.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use ralphy_core::{Usage, Workspace};

use crate::ClaudeAgent;

impl ClaudeAgent {
    /// `~/.claude/projects/<dashed-cwd>` for the repo this run operates on — the
    /// directory Claude writes the session transcript JSONL into (ADR-0008 D10).
    /// Derived from the byte-exact cwd passed to `claude` (the repo root).
    pub(crate) fn transcript_dir(&self, ws: &Workspace) -> Option<PathBuf> {
        let cwd = ws.repo_root().to_string_lossy();
        ralphy_adapter_support::home_scoped_path(
            None,
            Path::new(".claude/projects"),
            &PathBuf::from(dashed_cwd(&cwd)),
        )
    }
}

/// Parse the token usage off a headless `claude -p --output-format stream-json`
/// stdout (ADR-0008 D5, plan path). The stream is preceded by a human-readable
/// warning preamble ("no stdin data received in 3s…") and interleaves event
/// lines, so only lines whose trimmed start is `{` are JSON-parsed; the LAST
/// `{"type":"result",…}` object's `usage` is the authoritative per-invocation
/// total. Reads the four Messages-API fields and a model id (the `modelUsage`
/// map key, else `usage.model`). Returns `Usage::default()` when no result line
/// is found.
pub(crate) fn parse_plan_usage(stdout: &str) -> Usage {
    let mut found: Option<Usage> = None;
    for line in stdout.lines() {
        let line = line.trim_start();
        if !line.starts_with('{') {
            continue;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if value.get("type").and_then(|t| t.as_str()) != Some("result") {
            continue;
        }
        let Some(usage) = value.get("usage") else {
            continue;
        };
        let field = |k: &str| usage.get(k).and_then(|v| v.as_u64()).unwrap_or(0);
        let mut u = Usage {
            input: field("input_tokens"),
            output: field("output_tokens"),
            cache_read: field("cache_read_input_tokens"),
            cache_creation: field("cache_creation_input_tokens"),
            model: None,
        };
        // The model id resolves the price table (D8): prefer the *dominant*
        // `modelUsage` key — the main model the top-level `usage` block accounts
        // for — falling back to a `usage.model` field. Picking the dominant entry
        // (not the first) matters because Claude Code also bills a tiny amount to
        // a background model (e.g. haiku) for auxiliary work; that entry sorts
        // first alphabetically, so `keys().next()` mislabeled the whole phase as
        // haiku — and, being a dated id, missed the price table entirely.
        u.model = value
            .get("modelUsage")
            .and_then(|m| m.as_object())
            .and_then(dominant_model_key)
            .or_else(|| {
                usage
                    .get("model")
                    .and_then(|v| v.as_str())
                    .map(str::to_string)
            });
        found = Some(u); // keep the LAST result object
    }
    found.unwrap_or_default()
}

/// The vendor session identity of a headless `claude -p` plan (ADR-0033 §5): the
/// terminal `{"type":"result",…}` event's top-level `session_id`. Same last-result
/// scan as [`parse_plan_usage`]. `None` when no result event carries one.
pub(crate) fn parse_plan_session_id(stdout: &str) -> Option<String> {
    let mut found: Option<String> = None;
    for line in stdout.lines() {
        let line = line.trim_start();
        if !line.starts_with('{') {
            continue;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if value.get("type").and_then(|t| t.as_str()) != Some("result") {
            continue;
        }
        if let Some(id) = value.get("session_id").and_then(|v| v.as_str()) {
            found = Some(id.to_string()); // keep the LAST result object
        }
    }
    found
}

/// The vendor session identity of a Claude exec (ADR-0033 §5): the first appeared
/// transcript file's stem, which is the bare session uuid (== the plan payload's
/// `session_id`). `None` when no transcript appeared.
pub(crate) fn session_id_from_files(appeared: &[PathBuf]) -> Option<String> {
    appeared
        .first()
        .and_then(|p| p.file_stem())
        .and_then(|s| s.to_str())
        .map(str::to_string)
}

/// The subagent transcripts belonging to the session transcripts in `appeared`
/// (ADR-0008 D10): Claude Code files a subagent's own transcript under
/// `<transcript-dir>/<session-id>/subagents/*.jsonl` — a sibling *directory* that
/// the flat snapshot of the dashed-cwd dir never sees, so a Task-spawning session
/// under-reports by however much its tree consumed.
///
/// Deliberately derived from `appeared` rather than by recursing the snapshot:
/// scoping to the session ids this run created keeps the appeared-over-grew rule
/// intact (a *concurrent* pre-existing session's parent file merely grows and is
/// excluded, but its subagent files are freshly created and a blind recursion
/// would misattribute them to us) and keeps the run's session identity resolvable
/// from top-level files alone — see [`session_id_from_files`].
pub(crate) fn subagent_transcripts(appeared: &[PathBuf]) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = appeared
        .iter()
        .filter_map(|p| Some(p.parent()?.join(p.file_stem()?).join("subagents")))
        .flat_map(|dir| ralphy_adapter_support::list_session_files(&dir, "jsonl", false, None))
        .collect();
    out.sort();
    out
}

/// Sum an exec session's usage and attribute the phase to one model.
/// `per_transcript` is the session's own transcript(s); `subagent` is the usage
/// of the [`subagent_transcripts`] its Task calls spawned.
///
/// Tokens sum across **both**: a subagent's spend is the run's spend, and leaving
/// it out under-counted a real 43-delivery run by 22%. The *model* is derived from
/// the parent transcripts alone — the phase is the session, not the tree it
/// spawned, so a self-review that ran on a different tier cannot relabel the
/// execute phase it belongs to. Model derivation delegates to [`Usage::fold_usage`]
/// (the single place it lives, ADR-0008 D8), carrying the heaviest transcript's
/// model and falling back to `fallback_model` (the model we requested) rather than
/// `unknown`; [`Usage::add_tokens`] then folds the subagent counts in, dropping
/// their model on purpose.
pub(crate) fn fold_exec_usage(
    per_transcript: &[Usage],
    subagent: &[Usage],
    fallback_model: &str,
) -> Usage {
    let mut folded = Usage::fold_usage(per_transcript, Some(fallback_model));
    for usage in subagent {
        folded.add_tokens(usage);
    }
    folded
}

/// The key of the `modelUsage` entry with the most tokens — the run's *main*
/// model, the one the top-level `usage` block accounts for. `None` for an empty
/// map. Ties resolve to the last-seen max, which is immaterial (a tie means equal
/// spend, so the price is the same either way for the figures that matter).
fn dominant_model_key(model_usage: &serde_json::Map<String, serde_json::Value>) -> Option<String> {
    model_usage
        .iter()
        .max_by_key(|(_, entry)| model_usage_total(entry))
        .map(|(k, _)| k.clone())
}

/// Sum a `modelUsage` entry's token counts. These fields are **camelCase**
/// (`inputTokens`, `cacheReadInputTokens`, …), unlike the snake_case top-level
/// `usage` block — Claude Code reports the per-model breakdown in the other case.
fn model_usage_total(entry: &serde_json::Value) -> u64 {
    let f = |k: &str| entry.get(k).and_then(|v| v.as_u64()).unwrap_or(0);
    f("inputTokens") + f("outputTokens") + f("cacheReadInputTokens") + f("cacheCreationInputTokens")
}

/// Encode a launch cwd the way Claude Code names its `~/.claude/projects/<dir>`
/// transcript folder (ADR-0008 D10): every non-ASCII-alphanumeric character maps
/// to `-`, drive-letter case preserved. So `c:\Dev\ralphy` → `c--Dev-ralphy` and
/// `C:\Dev\.ralph-worktrees\issue-10` → `C--Dev--ralph-worktrees-issue-10` (the
/// dot becomes a second `-`). Pure over the byte-exact cwd string.
fn dashed_cwd(cwd: &str) -> String {
    cwd.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

/// Sum `cache_creation` tokens from a transcript `usage` block: prefer the flat
/// `cache_creation_input_tokens`, else sum the `cache_creation` 5m/1h ephemeral
/// sub-tiers (they total to the flat field, so taking the flat first avoids
/// double-counting). ADR-0008 D5/D10.
fn cache_creation_tokens(usage: &serde_json::Value) -> u64 {
    if let Some(flat) = usage
        .get("cache_creation_input_tokens")
        .and_then(|v| v.as_u64())
    {
        return flat;
    }
    if let Some(obj) = usage.get("cache_creation").and_then(|v| v.as_object()) {
        let tier = |k: &str| obj.get(k).and_then(|v| v.as_u64()).unwrap_or(0);
        return tier("ephemeral_5m_input_tokens") + tier("ephemeral_1h_input_tokens");
    }
    0
}

/// Parse and sum the token usage across a Claude-exec transcript JSONL (ADR-0008
/// D5/D10). Two traps the spike found are load-bearing here: **dedup by
/// `message.id`** (resume/branch replays and parallel-tool-call lines reuse one
/// id; a naïve sum overcounts ~2.8×) and **never descending into the nested
/// `iterations[]`** array (it repeats the top-level `usage`). Only the top-level
/// `message.usage` of each unique `message.id` is summed.
pub(crate) fn parse_transcript_usage(jsonl: &str) -> Usage {
    use std::collections::{BTreeMap, HashSet};
    let mut seen: HashSet<String> = HashSet::new();
    let mut total = Usage::default();
    // Per-model token tallies so the price table can resolve on the *dominant*
    // model (D8) — mirrors `parse_plan_usage`'s `modelUsage` attribution. Without
    // this every execute row was written `model: None` → `unknown` in the ledger,
    // leaving the bulk of a run's spend unpriced (`~$?`) in `ralphy usage`.
    let mut by_model: BTreeMap<String, u64> = BTreeMap::new();
    for line in jsonl.lines() {
        let Some(message) = line_message(line) else {
            continue;
        };
        // A line without `usage` must not use up its `message.id`: a later line
        // with the same id may carry the usage.
        let Some(usage) = message.get("usage") else {
            continue;
        };
        // Mandatory dedup: count one `usage` per unique `message.id`.
        if !first_sight(&message, &mut seen) {
            continue;
        }
        let tokens = usage_tokens(usage);
        total.add_tokens(&tokens);
        // Attribute this line's tokens to its assistant `message.model` so the
        // dominant model can be picked once the whole transcript is summed.
        if let Some(m) = message.get("model").and_then(|v| v.as_str()) {
            *by_model.entry(m.to_string()).or_insert(0) += tokens.total();
        }
    }
    total.model = dominant_model(by_model);
    total
}

/// The `message` object of one transcript line, `None` for a blank line, a line
/// that is not JSON, or a record without a `message`.
fn line_message(line: &str) -> Option<serde_json::Value> {
    let line = line.trim();
    if line.is_empty() {
        return None;
    }
    let mut value = serde_json::from_str::<serde_json::Value>(line).ok()?;
    value.get_mut("message").map(serde_json::Value::take)
}

/// Whether this message's `id` has not been counted yet. A message with no `id`
/// has nothing to dedup on, so it always counts.
fn first_sight(message: &serde_json::Value, seen: &mut std::collections::HashSet<String>) -> bool {
    match message.get("id").and_then(|v| v.as_str()) {
        Some(id) => seen.insert(id.to_string()),
        None => true,
    }
}

/// The four token counts of one `message.usage`. Only the top-level object is
/// read; `iterations[]` is never descended into, so its repeated `usage` is
/// ignored by construction.
fn usage_tokens(usage: &serde_json::Value) -> Usage {
    let field = |k: &str| usage.get(k).and_then(|v| v.as_u64()).unwrap_or(0);
    Usage {
        input: field("input_tokens"),
        output: field("output_tokens"),
        cache_read: field("cache_read_input_tokens"),
        cache_creation: cache_creation_tokens(usage),
        model: None,
    }
}

/// The dominant model (most tokens) is the one the price table resolves on; a
/// tie resolves to the last key, which is immaterial (equal spend → same price
/// for the figures that matter). `None` when no line carried a `model`.
fn dominant_model(by_model: std::collections::BTreeMap<String, u64>) -> Option<String> {
    by_model.into_iter().max_by_key(|(_, n)| *n).map(|(k, _)| k)
}

/// The most recent `claude` transcript JSONL under `~/.claude/projects`, read in
/// full, if it was touched in the last 5 minutes. Ports `Get-LatestTranscript`.
pub(crate) fn latest_transcript_text() -> Option<String> {
    let base = dirs_home()?.join(".claude").join("projects");
    let newest = newest_jsonl(&base)?;
    fs::read_to_string(newest).ok()
}

/// Read the newest transcript under `base` that was modified after
/// `transcript_since`. Used by the live PTY loop so a pre-existing transcript
/// from the same project cannot falsely trip a new session.
pub(crate) fn latest_transcript_text_since(
    base: Option<&Path>,
    transcript_since: SystemTime,
) -> Option<String> {
    let newest = newest_jsonl_since(base?, Some(transcript_since))?;
    fs::read_to_string(newest).ok()
}

/// The home directory, from the platform's usual env var. Thin alias over the
/// shared [`ralphy_adapter_support::home_dir`] so the env dance lives in one place.
pub(crate) fn dirs_home() -> Option<PathBuf> {
    ralphy_adapter_support::home_dir()
}

/// Recursively find the most-recently-modified `*.jsonl` under `base`, but only if
/// it was modified within the last 5 minutes (a stale transcript is irrelevant).
fn newest_jsonl(base: &Path) -> Option<PathBuf> {
    newest_jsonl_since(base, None)
}

fn newest_jsonl_since(base: &Path, min_modified: Option<SystemTime>) -> Option<PathBuf> {
    let mut newest: Option<(SystemTime, PathBuf)> = None;
    let mut stack = vec![base.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for (path, modified) in jsonl_in_dir(&dir, &mut stack) {
            if min_modified.is_some_and(|min| modified < min) {
                continue;
            }
            if newest.as_ref().is_none_or(|(t, _)| modified > *t) {
                newest = Some((modified, path));
            }
        }
    }
    let (modified, path) = newest?;
    fresh(modified).then_some(path)
}

/// The `*.jsonl` files directly in `dir` with their modified times, in listing
/// order; every subdirectory is pushed onto `stack` for the caller to visit.
fn jsonl_in_dir(dir: &Path, stack: &mut Vec<PathBuf>) -> Vec<(PathBuf, SystemTime)> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        // `file_type` does not follow links, so a symlinked folder is not walked.
        if kind.is_dir() {
            stack.push(path);
            continue;
        }
        if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        let Ok(modified) = entry.metadata().and_then(|m| m.modified()) else {
            continue;
        };
        found.push((path, modified));
    }
    found
}

/// Whether a transcript modified at `modified` is younger than 5 minutes.
fn fresh(modified: SystemTime) -> bool {
    let age = SystemTime::now()
        .duration_since(modified)
        .unwrap_or(Duration::ZERO);
    age < Duration::from_secs(300)
}

#[cfg(test)]
mod tests;
