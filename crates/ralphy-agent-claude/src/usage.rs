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

/// Sum a session's per-transcript usages and attribute the phase to one model.
/// Delegates to [`Usage::fold_usage`] — the single place accumulated-usage model
/// derivation lives (ADR-0008 D8) — so the heaviest transcript's model is carried,
/// falling back to `fallback_model` (the model we requested) rather than `unknown`.
pub(crate) fn fold_exec_usage(per_transcript: &[Usage], fallback_model: &str) -> Usage {
    Usage::fold_usage(per_transcript, Some(fallback_model))
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
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let Some(message) = value.get("message") else {
            continue;
        };
        // Mandatory dedup: count one `usage` per unique `message.id`.
        if let Some(id) = message.get("id").and_then(|v| v.as_str()) {
            if !seen.insert(id.to_string()) {
                continue;
            }
        }
        let Some(usage) = message.get("usage") else {
            continue;
        };
        let field = |k: &str| usage.get(k).and_then(|v| v.as_u64()).unwrap_or(0);
        // Only the top-level `message.usage` is read; `iterations[]` is never
        // descended into, so its repeated `usage` is ignored by construction.
        let input = field("input_tokens");
        let output = field("output_tokens");
        let cache_read = field("cache_read_input_tokens");
        let cache_creation = cache_creation_tokens(usage);
        total.input += input;
        total.output += output;
        total.cache_read += cache_read;
        total.cache_creation += cache_creation;
        // Attribute this line's tokens to its assistant `message.model` so the
        // dominant model can be picked once the whole transcript is summed.
        if let Some(m) = message.get("model").and_then(|v| v.as_str()) {
            *by_model.entry(m.to_string()).or_insert(0) +=
                input + output + cache_read + cache_creation;
        }
    }
    // The dominant model (most tokens) is the one the price table resolves on; a
    // tie resolves to the last key, which is immaterial (equal spend → same price
    // for the figures that matter). `None` when no line carried a `model`.
    total.model = by_model.into_iter().max_by_key(|(_, n)| *n).map(|(k, _)| k);
    total
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
    let mut newest: Option<(std::time::SystemTime, PathBuf)> = None;
    let mut stack = vec![base.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            let Ok(modified) = entry.metadata().and_then(|m| m.modified()) else {
                continue;
            };
            if min_modified.is_some_and(|min| modified < min) {
                continue;
            }
            if newest.as_ref().is_none_or(|(t, _)| modified > *t) {
                newest = Some((modified, path));
            }
        }
    }
    let (modified, path) = newest?;
    let age = std::time::SystemTime::now()
        .duration_since(modified)
        .unwrap_or(Duration::ZERO);
    (age < Duration::from_secs(300)).then_some(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_plan_usage_skips_warning_preamble() {
        // The headless `-p --output-format stream-json` stdout is preceded by a
        // non-JSON warning line; the parser must skip it and read the terminal
        // result event's usage (reconciled exactly against the payload, D5).
        let stdout = "no stdin data received in 3s. Continuing without stdin.\n\
{\"type\":\"system\",\"subtype\":\"init\"}\n\
{\"type\":\"assistant\",\"message\":{\"id\":\"m1\"}}\n\
{\"type\":\"result\",\"modelUsage\":{\"claude-opus-4-8\":{\"input_tokens\":3747}},\"usage\":{\"input_tokens\":3747,\"output_tokens\":9,\"cache_read_input_tokens\":0,\"cache_creation_input_tokens\":23406}}\n";
        let usage = parse_plan_usage(stdout);
        assert_eq!(usage.input, 3747);
        assert_eq!(usage.output, 9);
        assert_eq!(usage.cache_read, 0);
        assert_eq!(usage.cache_creation, 23406);
        assert_eq!(usage.model.as_deref(), Some("claude-opus-4-8"));
    }

    #[test]
    fn parse_plan_usage_attributes_to_dominant_not_alphabetical_model() {
        // The real shape (captured from a plan.log): Claude bills a tiny amount to
        // the background `claude-haiku-4-5-20251001` and the bulk to the main
        // `claude-opus-4-8`. The top-level `usage` is the MAIN model's split, so the
        // phase must be labeled opus — not haiku (which sorts first alphabetically
        // and is a dated id absent from the price table).
        let stdout = "{\"type\":\"result\",\
\"modelUsage\":{\
\"claude-haiku-4-5-20251001\":{\"inputTokens\":4375,\"outputTokens\":17,\"cacheReadInputTokens\":0,\"cacheCreationInputTokens\":0},\
\"claude-opus-4-8\":{\"inputTokens\":4237,\"outputTokens\":14023,\"cacheReadInputTokens\":1129426,\"cacheCreationInputTokens\":76510}},\
\"usage\":{\"input_tokens\":4237,\"output_tokens\":14023,\"cache_read_input_tokens\":1129426,\"cache_creation_input_tokens\":76510}}\n";
        let usage = parse_plan_usage(stdout);
        assert_eq!(
            usage.model.as_deref(),
            Some("claude-opus-4-8"),
            "the dominant model, not the alphabetically-first background haiku"
        );
        // The numeric split is the main model's (the top-level `usage`), unchanged.
        assert_eq!(usage.input, 4237);
        assert_eq!(usage.output, 14023);
        assert_eq!(usage.cache_read, 1129426);
        assert_eq!(usage.cache_creation, 76510);
    }

    #[test]
    fn dashed_cwd_encodes_nonalnum_and_preserves_case() {
        assert_eq!(dashed_cwd("c:\\Dev\\ralphy"), "c--Dev-ralphy");
        assert_eq!(
            dashed_cwd("C:\\Dev\\.ralph-worktrees\\issue-10"),
            "C--Dev--ralph-worktrees-issue-10"
        );
    }

    #[test]
    fn parse_transcript_usage_dedups_message_id_and_ignores_iterations() {
        // Three assistant lines: two share `m1` (counted once), one carries `m2`
        // and nests an `iterations[]` that repeats its usage (must be ignored).
        let jsonl = "\
{\"type\":\"assistant\",\"message\":{\"id\":\"m1\",\"usage\":{\"input_tokens\":100,\"output_tokens\":10,\"cache_read_input_tokens\":1000,\"cache_creation_input_tokens\":5}}}
{\"type\":\"assistant\",\"message\":{\"id\":\"m1\",\"usage\":{\"input_tokens\":999,\"output_tokens\":999,\"cache_read_input_tokens\":999,\"cache_creation_input_tokens\":999}}}
{\"type\":\"assistant\",\"message\":{\"id\":\"m2\",\"usage\":{\"input_tokens\":200,\"output_tokens\":20,\"cache_read_input_tokens\":2000,\"cache_creation_input_tokens\":7}},\"iterations\":[{\"usage\":{\"input_tokens\":200,\"output_tokens\":20,\"cache_read_input_tokens\":2000,\"cache_creation_input_tokens\":7}}]}
";
        let usage = parse_transcript_usage(jsonl);
        // m1 (first only) + m2: input 100+200, output 10+20, cache_read 1000+2000,
        // cache_creation 5+7. The duplicate m1 line and the nested iterations are
        // both excluded.
        assert_eq!(
            usage,
            Usage {
                input: 300,
                output: 30,
                cache_read: 3000,
                cache_creation: 12,
                model: None,
            }
        );
    }

    #[test]
    fn parse_transcript_usage_attributes_dominant_model() {
        // Two models in one transcript: a little haiku auxiliary work and the
        // bulk on opus. The summed `usage.model` must resolve to the *dominant*
        // (most-tokens) model so the price table prices the run — without this
        // every execute row was written `unknown` and went unpriced (`~$?`).
        let jsonl = "\
{\"type\":\"assistant\",\"message\":{\"id\":\"h1\",\"model\":\"claude-haiku-4-5\",\"usage\":{\"input_tokens\":10,\"output_tokens\":1,\"cache_read_input_tokens\":5,\"cache_creation_input_tokens\":0}}}
{\"type\":\"assistant\",\"message\":{\"id\":\"o1\",\"model\":\"claude-opus-4-8\",\"usage\":{\"input_tokens\":200,\"output_tokens\":20,\"cache_read_input_tokens\":2000,\"cache_creation_input_tokens\":7}}}
";
        let usage = parse_transcript_usage(jsonl);
        // Tokens still sum across both models...
        assert_eq!(usage.input, 210);
        assert_eq!(usage.cache_read, 2005);
        // ...but the model attribution picks opus (the dominant spend), not haiku.
        assert_eq!(usage.model.as_deref(), Some("claude-opus-4-8"));
    }

    #[test]
    fn fold_exec_usage_carries_heaviest_transcript_model() {
        // Two transcripts; the second is heavier. Tokens sum across both, and the
        // attribution follows the heaviest (opus) — not lost to `unknown`.
        let a = Usage {
            input: 10,
            output: 1,
            cache_read: 5,
            cache_creation: 0,
            model: Some("claude-haiku-4-5".into()),
        };
        let b = Usage {
            input: 200,
            output: 20,
            cache_read: 2000,
            cache_creation: 7,
            model: Some("claude-opus-4-8".into()),
        };
        let usage = fold_exec_usage(&[a, b], "sonnet");
        assert_eq!(usage.input, 210);
        assert_eq!(usage.cache_read, 2005);
        assert_eq!(usage.model.as_deref(), Some("claude-opus-4-8"));
    }

    #[test]
    fn fold_exec_usage_falls_back_to_requested_model_when_none_attributed() {
        // No transcript carried a model (counts present, attribution absent): the
        // phase falls back to the model we requested rather than `unknown`.
        let a = Usage {
            input: 100,
            output: 10,
            cache_read: 0,
            cache_creation: 0,
            model: None,
        };
        let usage = fold_exec_usage(&[a], "sonnet");
        assert_eq!(usage.input, 100);
        assert_eq!(usage.model.as_deref(), Some("sonnet"));
    }

    #[test]
    fn parse_plan_session_id_reads_result_event() {
        let stdout = "no stdin data received in 3s.\n\
{\"type\":\"system\",\"subtype\":\"init\"}\n\
{\"type\":\"result\",\"session_id\":\"sess-abc\",\"usage\":{}}\n";
        assert_eq!(parse_plan_session_id(stdout).as_deref(), Some("sess-abc"));
        // No result event → None.
        assert_eq!(parse_plan_session_id("{\"type\":\"system\"}\n"), None);
    }

    #[test]
    fn session_id_from_files_takes_first_stem() {
        assert_eq!(
            session_id_from_files(&[PathBuf::from("/x/c6ab25d8-de.jsonl")]).as_deref(),
            Some("c6ab25d8-de")
        );
        assert_eq!(session_id_from_files(&[]), None);
    }

    #[test]
    fn parse_transcript_usage_sums_cache_creation_subtiers() {
        // When only the `cache_creation` 5m/1h breakdown is present (no flat
        // field), the sub-tiers are summed.
        let jsonl = "{\"message\":{\"id\":\"x\",\"usage\":{\"input_tokens\":1,\"cache_creation\":{\"ephemeral_5m_input_tokens\":40,\"ephemeral_1h_input_tokens\":2}}}}\n";
        let usage = parse_transcript_usage(jsonl);
        assert_eq!(usage.input, 1);
        assert_eq!(usage.cache_creation, 42);
    }
}
