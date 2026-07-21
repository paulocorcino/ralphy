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
use std::path::{Path, PathBuf};

use crate::{ClaudeScan, InteractiveRecord, Tokens};

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
    for entry in entries.flatten() {
        let ws_path = entry.path();
        if !ws_path.is_dir() {
            continue;
        }
        let ws_key = entry.file_name().to_string_lossy().to_string();
        // Attribute by dashed-cwd-encoding each registered repo path (D10) and
        // matching the workspace-key dir name exactly. No match → project/actor
        // stay None; the session is still reported.
        let matched = input.repos.iter().find(|r| dashed_cwd(&r.path) == ws_key);
        let project = matched.map(|r| r.slug.clone());
        let actor_email = matched.and_then(|r| {
            email_cache
                .entry(r.slug.clone())
                .or_insert_with(|| repo_actor_email(&r.path))
                .clone()
        });

        for transcript in jsonl_files(&ws_path) {
            let Some(session_id) = transcript
                .file_stem()
                .and_then(|s| s.to_str())
                .map(str::to_string)
            else {
                continue;
            };
            if input.run_session_ids.contains(&session_id) {
                continue;
            }
            let Ok(text) = fs::read_to_string(&transcript) else {
                continue;
            };
            records.extend(parse_transcript(&text, &session_id, &project, &actor_email));
        }
    }

    if let Some(since) = input.since {
        if let Ok(since_dt) = chrono::DateTime::parse_from_rfc3339(since) {
            records.retain(|r| match chrono::DateTime::parse_from_rfc3339(&r.last_ts) {
                Ok(last) => last >= since_dt,
                Err(_) => true, // never hide spend on a parse miss
            });
        }
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

/// Parse one transcript file's lines into interactive records — one per model
/// seen. Only assistant lines carrying both `message.model` and `message.usage`
/// contribute (model-less / usage-less lines are skipped, so a zero-usage stub
/// session yields no record). The nested `iterations[]` is never read: only the
/// top-level `message.usage` counts (ADR-0008 D5).
fn parse_transcript(
    jsonl: &str,
    session_id: &str,
    project: &Option<String>,
    actor_email: &Option<String>,
) -> Vec<InteractiveRecord> {
    let mut groups: BTreeMap<String, Group> = BTreeMap::new();
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
        let Some(model) = message.get("model").and_then(|v| v.as_str()) else {
            continue;
        };
        let Some(usage) = message.get("usage") else {
            continue;
        };
        let field = |k: &str| usage.get(k).and_then(|v| v.as_u64()).unwrap_or(0);
        let toks = [
            field("input_tokens"),
            field("output_tokens"),
            field("cache_read_input_tokens"),
            cache_creation_tokens(usage),
        ];

        // Dedup key: `message.id` (in `message`) joined to `requestId` (top-level
        // on the entry, per the transcript schema), else `message:<id>`, else none.
        let id = message.get("id").and_then(|v| v.as_str());
        let req = value.get("requestId").and_then(|v| v.as_str());
        let dedup_key = match (id, req) {
            (Some(id), Some(req)) => Some(format!("{id}:{req}")),
            (Some(id), None) => Some(format!("message:{id}")),
            _ => None,
        };

        let group = groups.entry(model.to_string()).or_default();
        match dedup_key {
            Some(key) => {
                let slot = group.per_key.entry(key).or_default();
                for (s, t) in slot.iter_mut().zip(toks.iter()) {
                    *s = (*s).max(*t);
                }
            }
            None => {
                for (n, t) in group.no_key.iter_mut().zip(toks.iter()) {
                    *n += *t;
                }
            }
        }

        if let Some(ts) = value.get("timestamp").and_then(|v| v.as_str()) {
            if group.first_ts.as_deref().is_none_or(|cur| ts_lt(ts, cur)) {
                group.first_ts = Some(ts.to_string());
            }
            if group.last_ts.as_deref().is_none_or(|cur| ts_lt(cur, ts)) {
                group.last_ts = Some(ts.to_string());
            }
        }
    }

    groups
        .into_iter()
        .map(|(model, group)| {
            let mut total = group.no_key;
            for maxed in group.per_key.values() {
                for (t, m) in total.iter_mut().zip(maxed.iter()) {
                    *t += *m;
                }
            }
            InteractiveRecord {
                agent: "claude".to_string(),
                model,
                session_id: session_id.to_string(),
                project: project.clone(),
                actor_email: actor_email.clone(),
                tokens: Some(Tokens {
                    input: total[0],
                    output: total[1],
                    cache_read: total[2],
                    cache_creation: total[3],
                }),
                first_ts: group.first_ts.unwrap_or_default(),
                last_ts: group.last_ts.unwrap_or_default(),
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

/// Sum `cache_creation` tokens from a transcript `usage` block: prefer the flat
/// `cache_creation_input_tokens`, else the `cache_creation` 5m/1h ephemeral
/// sub-tiers (they total to the flat field). Mirrors the adapter (ADR-0008 D5).
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

/// Encode a launch cwd the way Claude Code names its `~/.claude/projects/<dir>`
/// transcript folder (ADR-0008 D10): every non-ASCII-alphanumeric character maps
/// to `-`, drive-letter case preserved. Verbatim from
/// `ralphy-agent-claude/src/usage.rs`.
fn dashed_cwd(cwd: &str) -> String {
    cwd.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

/// `git config user.email` for the attributed repo (ADR-0008 D7). `None` on a
/// non-zero exit or empty output. Mirrors `ralphy-core/src/git.rs` `user_email`;
/// the scan crate cannot depend on core (ADR-0032), so it shells out directly.
fn repo_actor_email(path: &str) -> Option<String> {
    let output = std::process::Command::new("git")
        .args(["-C", path, "config", "user.email"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let email = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!email.is_empty()).then_some(email)
}

/// Every `*.jsonl` under `dir`, recursively. Tolerant: an unreadable subdir is
/// skipped. Order is unspecified (each file is one independent session).
fn jsonl_files(dir: &Path) -> Vec<PathBuf> {
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
            } else if path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
                out.push(path);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn write(dir: &Path, rel: &str, content: &str) {
        let path = dir.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    fn no_runs() -> HashSet<String> {
        HashSet::new()
    }

    /// One usage line: an assistant `message` with model, usage, and a timestamp.
    fn usage_line(model: &str, id: &str, req: Option<&str>, input: u64, ts: &str) -> String {
        let req_field = req
            .map(|r| format!("\"requestId\":\"{r}\","))
            .unwrap_or_default();
        format!(
            "{{{req_field}\"timestamp\":\"{ts}\",\"message\":{{\"id\":\"{id}\",\"model\":\"{model}\",\"usage\":{{\"input_tokens\":{input},\"output_tokens\":0,\"cache_read_input_tokens\":0,\"cache_creation_input_tokens\":0}}}}}}"
        )
    }

    #[test]
    fn dedups_message_id_request_id_by_max_merge() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        // Three lines share id=m1,requestId=r1 (input 100→300→250: MAX is 300, not
        // 650 sum, not 100 first-wins); a distinct m2 line adds 50 → session 350.
        let body = [
            usage_line(
                "claude-opus-4-8",
                "m1",
                Some("r1"),
                100,
                "2026-07-10T10:00:00Z",
            ),
            usage_line(
                "claude-opus-4-8",
                "m1",
                Some("r1"),
                300,
                "2026-07-10T10:00:01Z",
            ),
            usage_line(
                "claude-opus-4-8",
                "m1",
                Some("r1"),
                250,
                "2026-07-10T10:00:02Z",
            ),
            usage_line(
                "claude-opus-4-8",
                "m2",
                Some("r2"),
                50,
                "2026-07-10T10:00:03Z",
            ),
        ]
        .join("\n");
        write(root, "ws-key/sess1.jsonl", &body);

        let records = scan_claude(&ClaudeScan {
            projects_dir: root,
            run_session_ids: &no_runs(),
            repos: &[],
            since: None,
        });
        assert_eq!(records.len(), 1);
        assert_eq!(
            records[0].tokens.as_ref().unwrap().input,
            350,
            "m1 max 300 + m2 50, not 650+50 sum"
        );
    }

    #[test]
    fn excludes_run_owned_sessions() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write(
            root,
            "ws-key/run-sess.jsonl",
            &usage_line("m", "a", Some("r"), 10, "2026-07-10T10:00:00Z"),
        );
        write(
            root,
            "ws-key/int-sess.jsonl",
            &usage_line("m", "b", Some("r"), 20, "2026-07-10T10:00:00Z"),
        );
        let mut runs = HashSet::new();
        runs.insert("run-sess".to_string());

        let records = scan_claude(&ClaudeScan {
            projects_dir: root,
            run_session_ids: &runs,
            repos: &[],
            since: None,
        });
        assert!(records.iter().any(|r| r.session_id == "int-sess"));
        assert!(!records.iter().any(|r| r.session_id == "run-sess"));
    }

    #[test]
    fn attributes_dashed_cwd_dotted_and_drive_case() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write(
            root,
            "c--Dev-ralphy/s1.jsonl",
            &usage_line("m", "a", Some("r"), 10, "2026-07-10T10:00:00Z"),
        );
        write(
            root,
            "C--Dev--ralph-worktrees-issue-10/s2.jsonl",
            &usage_line("m", "b", Some("r"), 20, "2026-07-10T10:00:00Z"),
        );
        let repos = vec![
            crate::RegisteredRepo {
                slug: "o/ralphy".into(),
                path: "c:\\Dev\\ralphy".into(),
            },
            crate::RegisteredRepo {
                slug: "o/wt".into(),
                path: "C:\\Dev\\.ralph-worktrees\\issue-10".into(),
            },
        ];
        let records = scan_claude(&ClaudeScan {
            projects_dir: root,
            run_session_ids: &no_runs(),
            repos: &repos,
            since: None,
        });
        let s1 = records.iter().find(|r| r.session_id == "s1").unwrap();
        let s2 = records.iter().find(|r| r.session_id == "s2").unwrap();
        assert_eq!(
            s1.project.as_deref(),
            Some("o/ralphy"),
            "drive-case preserved"
        );
        assert_eq!(
            s2.project.as_deref(),
            Some("o/wt"),
            "dotted path → double dash"
        );
    }

    #[test]
    fn unmatched_workspace_yields_null_project() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write(
            root,
            "some-unknown-ws/s1.jsonl",
            &usage_line("m", "a", Some("r"), 10, "2026-07-10T10:00:00Z"),
        );
        let records = scan_claude(&ClaudeScan {
            projects_dir: root,
            run_session_ids: &no_runs(),
            repos: &[],
            since: None,
        });
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].session_id, "s1");
        assert_eq!(records[0].project, None);
        assert_eq!(records[0].actor_email, None);
    }

    #[test]
    fn zero_usage_session_scans_to_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        // A summary line and a user line: neither carries `message.usage`.
        let body = "{\"type\":\"summary\"}\n{\"type\":\"user\",\"message\":{\"role\":\"user\"}}\n";
        write(root, "ws-key/empty.jsonl", body);
        let records = scan_claude(&ClaudeScan {
            projects_dir: root,
            run_session_ids: &no_runs(),
            repos: &[],
            since: None,
        });
        assert!(records.is_empty());
    }

    #[test]
    fn attributed_record_carries_git_actor_email() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("projects");
        // A real git repo so `git config user.email` resolves.
        let repo = tmp.path().join("repo");
        fs::create_dir_all(&repo).unwrap();
        let run = |args: &[&str]| {
            std::process::Command::new("git")
                .args(["-C", repo.to_str().unwrap()])
                .args(args)
                .output()
                .unwrap();
        };
        run(&["init"]);
        run(&["config", "user.email", "t@example.com"]);

        let repo_path = repo.to_string_lossy().to_string();
        let ws_key = dashed_cwd(&repo_path);
        write(
            &root,
            &format!("{ws_key}/s1.jsonl"),
            &usage_line("m", "a", Some("r"), 10, "2026-07-10T10:00:00Z"),
        );
        let repos = vec![crate::RegisteredRepo {
            slug: "o/repo".into(),
            path: repo_path,
        }];
        let records = scan_claude(&ClaudeScan {
            projects_dir: &root,
            run_session_ids: &no_runs(),
            repos: &repos,
            since: None,
        });
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].project.as_deref(), Some("o/repo"));
        assert_eq!(records[0].actor_email.as_deref(), Some("t@example.com"));
    }

    #[test]
    fn since_filters_interactive_by_last_ts() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write(
            root,
            "ws-key/old.jsonl",
            &usage_line("m", "a", Some("r"), 10, "2026-07-01T10:00:00Z"),
        );
        write(
            root,
            "ws-key/new.jsonl",
            &usage_line("m", "b", Some("r"), 20, "2026-07-10T10:00:00Z"),
        );
        // `+00:00` offset bound vs the transcripts' `Z` form: parsed compare works.
        let records = scan_claude(&ClaudeScan {
            projects_dir: root,
            run_session_ids: &no_runs(),
            repos: &[],
            since: Some("2026-07-05T00:00:00+00:00"),
        });
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].session_id, "new");
    }
}
