//! The Claude Code module of the usage scan (ADR-0033 §2/§6, ADR-0008 D5/D10).
//! Parses the transcript JSONL under `~/.claude/projects/<workspace-key>/` into
//! per-session × model interactive records. The dedup here is the
//! `message.id:requestId` max-merge refinement (§2) — distinct from the adapter's
//! first-wins sum ([`ralphy_agent_claude`](../../ralphy-agent-claude) — `usage.rs`):
//! Claude's streaming API rewrites one `message.id:requestId` several times with
//! growing counts, so the per-field MAX is the complete record; distinct dedup
//! keys are then SUMMED into the session×model aggregate.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::Path;

use crate::{since, ClaudeScan, InteractiveRecord, Tokens};

/// Scan the Claude projects store into interactive records (one per session ×
/// model). A missing or unreadable `projects_dir` yields an empty vec (not an
/// error). Sessions whose id is in `run_session_ids` are Ralphy runs', never
/// interactive, and are excluded. `since` drops records whose `last_ts` is
/// strictly before it (§6: reported, never dropped — the filter is payload
/// economy, so an unparseable bound or record keeps the record).
pub fn scan_claude(input: &ClaudeScan) -> Vec<InteractiveRecord> {
    let mut records = Vec::new();
    let Ok(entries) = fs::read_dir(input.projects_dir) else {
        return records;
    };
    // slug → resolved git actor email, computed at most once per attributed repo.
    let mut email_cache: HashMap<String, Option<String>> = HashMap::new();
    let keys = workspace_keys(input.repos);
    for entry in entries.flatten() {
        let ws_path = entry.path();
        if !ws_path.is_dir() {
            continue;
        }
        let ws_key = entry.file_name().to_string_lossy().to_string();
        records.extend(scan_workspace(
            input,
            &keys,
            &mut email_cache,
            &ws_path,
            &ws_key,
        ));
    }

    since::retain_since(&mut records, input.since);
    records
}

/// Dashed-cwd keys the workspace-key dir name can match: each repo's own
/// path, PLUS each of its checkouts under `.ralphy/worktrees/` (ADR-0063 §5)
/// — the lossy dashed key cannot be reversed, so this resolves it "the other
/// way" by enumerating the bounded, known checkout location instead.
fn workspace_keys(repos: &[crate::RegisteredRepo]) -> Vec<(String, &crate::RegisteredRepo)> {
    repos
        .iter()
        .flat_map(|r| {
            std::iter::once((dashed_cwd(&r.path), r)).chain(
                crate::attribution::checkout_dirs(&r.path)
                    .into_iter()
                    .map(move |d| (dashed_cwd(&d.to_string_lossy()), r)),
            )
        })
        .collect()
}

/// The interactive records of one workspace directory `ws_path` (named
/// `ws_key`), skipping the sessions that are Ralphy runs'.
fn scan_workspace(
    input: &ClaudeScan,
    keys: &[(String, &crate::RegisteredRepo)],
    email_cache: &mut HashMap<String, Option<String>>,
    ws_path: &Path,
    ws_key: &str,
) -> Vec<InteractiveRecord> {
    // Attribute by dashed-cwd-encoding each registered repo path (D10), or
    // one of its worktree checkouts (ADR-0063 §5), and matching the
    // workspace-key dir name exactly. No match → project/actor stay None;
    // the session is still reported.
    let matched = keys.iter().find(|(k, _)| *k == ws_key).map(|(_, r)| *r);
    let project = matched.map(|r| r.slug.clone());
    let actor_email = matched.and_then(|r| {
        email_cache
            .entry(r.slug.clone())
            .or_insert_with(|| ralphy_git_read::user_email(Path::new(&r.path)))
            .clone()
    });

    let mut records = Vec::new();
    for transcript in crate::walk::files_under(ws_path, crate::walk::is_jsonl) {
        let Some(session_id) = transcript
            .file_stem()
            .and_then(|s| s.to_str())
            .map(str::to_owned)
        else {
            continue;
        };
        if input.run_session_ids.contains(&session_id) {
            continue;
        }
        let Ok(text) = fs::read_to_string(&transcript) else {
            continue;
        };
        records.extend(parse_transcript(
            &text,
            &session_id,
            project.as_deref(),
            actor_email.as_deref(),
        ));
    }
    records
}

/// Per-(session, model) accumulator. `per_key` holds the running per-field MAX
/// for each dedup key (streaming duplicates of one `message.id:requestId`);
/// `no_key` sums lines that carry no dedup key. The final token total is the sum
/// of the `per_key` maxima plus `no_key`.
#[derive(Default)]
struct Group {
    per_key: HashMap<String, [u64; 4]>,
    no_key: [u64; 4],
    first_ts: Option<String>,
    last_ts: Option<String>,
}

/// One assistant line with a model and a top-level usage block.
struct UsageLine {
    model: String,
    toks: [u64; 4],
    dedup_key: Option<String>,
    ts: Option<String>,
}

/// Read one transcript line. `None` for a blank or unparseable line, a line
/// without `message.model`, and a line without `message.usage`.
fn read_usage_line(line: &str) -> Option<UsageLine> {
    let value: serde_json::Value = serde_json::from_str(line.trim()).ok()?;
    let message = value.get("message")?;
    let model = message.get("model").and_then(|v| v.as_str())?;
    let usage = message.get("usage")?;
    let field = |k: &str| {
        usage
            .get(k)
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0)
    };
    Some(UsageLine {
        model: model.to_owned(),
        toks: [
            field("input_tokens"),
            field("output_tokens"),
            field("cache_read_input_tokens"),
            cache_creation_tokens(usage),
        ],
        dedup_key: dedup_key(message, &value),
        ts: value
            .get("timestamp")
            .and_then(|v| v.as_str())
            .map(str::to_owned),
    })
}

/// Dedup key: `message.id` (in `message`) joined to `requestId` (top-level
/// on the entry, per the transcript schema), else `message:<id>`, else none.
fn dedup_key(message: &serde_json::Value, entry: &serde_json::Value) -> Option<String> {
    let id = message.get("id").and_then(|v| v.as_str());
    let req = entry.get("requestId").and_then(|v| v.as_str());
    match (id, req) {
        (Some(id), Some(req)) => Some(format!("{id}:{req}")),
        (Some(id), None) => Some(format!("message:{id}")),
        _ => None,
    }
}

impl Group {
    /// Fold one line in: max-merge under a dedup key, sum without one.
    fn fold(&mut self, line: &UsageLine) {
        match &line.dedup_key {
            Some(key) => {
                let slot = self.per_key.entry(key.clone()).or_default();
                for (s, t) in slot.iter_mut().zip(line.toks.iter()) {
                    *s = (*s).max(*t);
                }
            }
            None => {
                for (n, t) in self.no_key.iter_mut().zip(line.toks.iter()) {
                    *n += *t;
                }
            }
        }
        if let Some(ts) = &line.ts {
            self.widen(ts);
        }
    }

    /// Widen the first/last timestamps to include `ts`.
    fn widen(&mut self, ts: &str) {
        if self.first_ts.as_deref().is_none_or(|cur| ts_lt(ts, cur)) {
            self.first_ts = Some(ts.to_owned());
        }
        if self.last_ts.as_deref().is_none_or(|cur| ts_lt(cur, ts)) {
            self.last_ts = Some(ts.to_owned());
        }
    }

    /// The token total: the sum of the `per_key` maxima plus `no_key`.
    fn total(&self) -> [u64; 4] {
        let mut total = self.no_key;
        for maxed in self.per_key.values() {
            for (t, m) in total.iter_mut().zip(maxed.iter()) {
                *t += *m;
            }
        }
        total
    }
}

/// Parse one transcript file's lines into interactive records — one per model
/// seen. Only assistant lines carrying both `message.model` and `message.usage`
/// contribute (model-less / usage-less lines are skipped, so a zero-usage stub
/// session yields no record). The nested `iterations[]` is never read: only the
/// top-level `message.usage` counts (ADR-0008 D5).
fn parse_transcript(
    jsonl: &str,
    session_id: &str,
    project: Option<&str>,
    actor_email: Option<&str>,
) -> Vec<InteractiveRecord> {
    let mut groups: BTreeMap<String, Group> = BTreeMap::new();
    for line in jsonl.lines().filter_map(read_usage_line) {
        groups.entry(line.model.clone()).or_default().fold(&line);
    }

    groups
        .into_iter()
        .map(|(model, group)| {
            let total = group.total();
            InteractiveRecord {
                agent: "claude".to_owned(),
                model,
                session_id: session_id.to_owned(),
                project: project.map(str::to_owned),
                actor_email: actor_email.map(str::to_owned),
                tokens: Some(Tokens {
                    input: total[0],
                    output: total[1],
                    cache_read: total[2],
                    cache_creation: total[3],
                }),
                first_ts: group.first_ts.unwrap_or_default(),
                last_ts: group.last_ts.unwrap_or_default(),
                lower_bound: false,
            }
        })
        .collect()
}

/// `a < b` for two RFC3339 timestamp strings, comparing the parsed instants so a
/// `…Z` form and a `+00:00` offset order correctly. Falls back to a lexical
/// compare when either does not parse (best-effort ordering, never a panic).
fn ts_lt(a: &str, b: &str) -> bool {
    match (
        chrono::DateTime::parse_from_rfc3339(a),
        chrono::DateTime::parse_from_rfc3339(b),
    ) {
        (Ok(a), Ok(b)) => a < b,
        _ => a < b,
    }
}

// The same function is in `ralphy-agent-claude/src/usage.rs`, and it stays two
// copies: the usage scan does not import an adapter (ADR-0033 §7). The test
// `cache_creation_tokens_matches_the_shared_fixture` holds both to one fixture.
// jscpd:ignore-start
/// Sum `cache_creation` tokens from a transcript `usage` block: prefer the flat
/// `cache_creation_input_tokens`, else the `cache_creation` 5m/1h ephemeral
/// sub-tiers (they total to the flat field). Mirrors the adapter (ADR-0008 D5).
fn cache_creation_tokens(usage: &serde_json::Value) -> u64 {
    if let Some(flat) = usage
        .get("cache_creation_input_tokens")
        .and_then(serde_json::Value::as_u64)
    {
        return flat;
    }
    if let Some(obj) = usage.get("cache_creation").and_then(|v| v.as_object()) {
        let tier = |k: &str| obj.get(k).and_then(serde_json::Value::as_u64).unwrap_or(0);
        return tier("ephemeral_5m_input_tokens") + tier("ephemeral_1h_input_tokens");
    }
    0
}
// jscpd:ignore-end

/// Encode a launch cwd the way Claude Code names its `~/.claude/projects/<dir>`
/// transcript folder (ADR-0008 D10): every non-ASCII-alphanumeric character maps
/// to `-`, drive-letter case preserved. Verbatim from
/// `ralphy-agent-claude/src/usage.rs`.
fn dashed_cwd(cwd: &str) -> String {
    cwd.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

#[cfg(test)]
mod tests;
