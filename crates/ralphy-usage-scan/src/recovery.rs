//! Vendor-neutral model recovery over the existing usage-scan readers.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::PathBuf;

use crate::{
    claude::scan_claude, codex::scan_codex_with_stems, copilot::scan_copilot, cursor::scan_cursor,
    gemini::scan_gemini, kimi::scan_kimi, opencode::scan_opencode, ClaudeScan, CodexScan,
    CopilotScan, CursorScan, GeminiScan, InteractiveRecord, KimiScan, OpenCodeScan,
};

/// A ledger identity whose model is unknown.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RecoveryCandidate {
    pub agent: String,
    pub session_id: String,
}

impl RecoveryCandidate {
    #[must_use]
    pub fn new(agent: impl Into<String>, session_id: impl Into<String>) -> Self {
        Self {
            agent: agent.into(),
            session_id: session_id.into(),
        }
    }
}

/// Vendor store locations consumed by the existing scan readers.
#[derive(Debug, Clone, Default)]
pub struct RecoveryStores {
    pub claude_projects_dir: PathBuf,
    pub codex_dir: PathBuf,
    pub copilot_db: PathBuf,
    pub cursor_dir: PathBuf,
    pub gemini_dir: PathBuf,
    pub kimi_dir: PathBuf,
    pub kimi_code_dir: PathBuf,
    pub opencode_db: PathBuf,
}

/// One attempted recovery. `None` means the store has no usable model fact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryResult {
    pub candidate: RecoveryCandidate,
    pub model: Option<String>,
}

/// Resolve only the requested agent/session pairs. Every vendor record is
/// produced by its existing usage-scan reader; this module only joins and folds.
#[must_use]
pub fn resolve_models(
    candidates: &[RecoveryCandidate],
    stores: &RecoveryStores,
) -> Vec<RecoveryResult> {
    let requested: HashSet<RecoveryCandidate> = candidates.iter().cloned().collect();
    let agents: BTreeSet<&str> = requested.iter().map(|c| c.agent.as_str()).collect();
    let mut matches: BTreeMap<RecoveryCandidate, Vec<InteractiveRecord>> = BTreeMap::new();

    for agent in agents {
        if agent == "codex" {
            collect_codex(&requested, &mut matches, stores);
        } else {
            collect_requested(&requested, &mut matches, scan_agent(agent, stores));
        }
    }
    results_for(candidates, &matches)
}

/// The records of the one reader that scans `agent`'s store; none for an
/// agent with no reader.
fn scan_agent(agent: &str, stores: &RecoveryStores) -> Vec<InteractiveRecord> {
    let no_runs = HashSet::new();
    match agent {
        "claude" => scan_claude(&ClaudeScan {
            projects_dir: &stores.claude_projects_dir,
            run_session_ids: &no_runs,
            repos: &[],
            since: None,
        }),
        "copilot" => scan_copilot(&CopilotScan {
            db_path: &stores.copilot_db,
            run_session_ids: &no_runs,
            repos: &[],
            since: None,
        }),
        "cursor" => scan_cursor(&CursorScan {
            cursor_dir: &stores.cursor_dir,
            run_session_ids: &no_runs,
            repos: &[],
            since: None,
        }),
        "gemini" => scan_gemini(&GeminiScan {
            gemini_dir: &stores.gemini_dir,
            run_session_ids: &no_runs,
            repos: &[],
            since: None,
        }),
        "kimi" => scan_kimi(&KimiScan {
            kimi_dir: &stores.kimi_dir,
            kimi_code_dir: &stores.kimi_code_dir,
            run_session_ids: &no_runs,
            repos: &[],
            since: None,
        }),
        "opencode" => scan_opencode(&OpenCodeScan {
            db_path: &stores.opencode_db,
            run_session_ids: &no_runs,
            repos: &[],
            since: None,
        }),
        _ => Vec::new(),
    }
}

/// A codex session is named by its rollout file stem or by the id in its
/// session meta; either one matches a candidate.
fn collect_codex(
    requested: &HashSet<RecoveryCandidate>,
    matches: &mut BTreeMap<RecoveryCandidate, Vec<InteractiveRecord>>,
    stores: &RecoveryStores,
) {
    let no_runs = HashSet::new();
    let scanned = scan_codex_with_stems(&CodexScan {
        codex_dir: &stores.codex_dir,
        run_session_ids: &no_runs,
        repos: &[],
        since: None,
    });
    for (stem, record) in scanned {
        for session_id in [&stem, &record.session_id] {
            let candidate = RecoveryCandidate::new("codex", session_id);
            if requested.contains(&candidate) {
                matches.entry(candidate).or_default().push(record.clone());
            }
        }
    }
}

/// One result per candidate, in the order given.
fn results_for(
    candidates: &[RecoveryCandidate],
    matches: &BTreeMap<RecoveryCandidate, Vec<InteractiveRecord>>,
) -> Vec<RecoveryResult> {
    candidates
        .iter()
        .cloned()
        .map(|candidate| RecoveryResult {
            model: matches
                .get(&candidate)
                .and_then(|records| best_model(records)),
            candidate,
        })
        .collect()
}

fn collect_requested(
    requested: &HashSet<RecoveryCandidate>,
    matches: &mut BTreeMap<RecoveryCandidate, Vec<InteractiveRecord>>,
    records: Vec<InteractiveRecord>,
) {
    for record in records {
        let candidate = RecoveryCandidate::new(&record.agent, &record.session_id);
        if requested.contains(&candidate) {
            matches.entry(candidate).or_default().push(record);
        }
    }
}

fn best_model(records: &[InteractiveRecord]) -> Option<String> {
    records
        .iter()
        .filter(|record| record.model != "unknown")
        .max_by(|left, right| {
            record_total(left)
                .cmp(&record_total(right))
                .then_with(|| left.model.cmp(&right.model))
        })
        .map(|record| record.model.clone())
}

fn record_total(record: &InteractiveRecord) -> u64 {
    record.tokens.as_ref().map_or(0, |tokens| {
        tokens.input + tokens.output + tokens.cache_read + tokens.cache_creation
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Tokens;
    use rusqlite::Connection;
    use std::path::Path;

    fn record(model: &str, total: u64) -> InteractiveRecord {
        InteractiveRecord {
            agent: "claude".into(),
            model: model.into(),
            session_id: "session".into(),
            project: None,
            actor_email: None,
            tokens: Some(Tokens {
                input: total,
                ..Tokens::default()
            }),
            first_ts: String::new(),
            last_ts: String::new(),
            lower_bound: false,
        }
    }

    #[test]
    fn heaviest_known_model_wins_with_lexical_tie_break() {
        assert_eq!(
            best_model(&[
                record("unknown", 1_000),
                record("alpha", 20),
                record("zeta", 20),
                record("lighter", 19),
            ])
            .as_deref(),
            Some("zeta")
        );
    }

    #[test]
    fn unknown_only_has_no_recovery() {
        assert_eq!(best_model(&[record("unknown", 10)]), None);
    }

    #[test]
    fn missing_stores_return_none_for_every_candidate() {
        let temp = tempfile::tempdir().unwrap();
        let stores = RecoveryStores {
            claude_projects_dir: temp.path().join("missing"),
            ..RecoveryStores::default()
        };
        let results = resolve_models(&[RecoveryCandidate::new("claude", "absent")], &stores);
        assert_eq!(results[0].model, None);
    }

    fn write(path: &Path, content: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }

    #[test]
    fn a_codex_candidate_may_name_the_session_meta_id() {
        let temp = tempfile::tempdir().unwrap();
        let stores = RecoveryStores {
            codex_dir: temp.path().join("codex"),
            ..RecoveryStores::default()
        };
        write(
            &stores.codex_dir.join("sessions/rollout-codex.jsonl"),
            concat!(
                "{\"timestamp\":\"2026-07-29T00:00:00Z\",\"type\":\"session_meta\",\"payload\":{\"id\":\"codex-meta\",\"cwd\":\"C:\\\\repo\"}}\n",
                "{\"timestamp\":\"2026-07-29T00:00:01Z\",\"type\":\"turn_context\",\"payload\":{\"model\":\"gpt-5-codex\"}}\n",
                "{\"timestamp\":\"2026-07-29T00:00:02Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"info\":{\"total_token_usage\":{\"input_tokens\":10,\"cached_input_tokens\":0,\"output_tokens\":2}}}}\n",
            ),
        );

        let results = resolve_models(&[RecoveryCandidate::new("codex", "codex-meta")], &stores);

        assert_eq!(results[0].model.as_deref(), Some("gpt-5-codex"));
    }

    #[test]
    fn existing_vendor_readers_resolve_fixture_candidates() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let stores = RecoveryStores {
            claude_projects_dir: root.join("claude"),
            codex_dir: root.join("codex"),
            copilot_db: root.join("copilot.db"),
            cursor_dir: root.join("cursor"),
            gemini_dir: root.join("gemini"),
            kimi_dir: root.join("kimi"),
            kimi_code_dir: root.join("kimi-code"),
            opencode_db: root.join("opencode.db"),
        };

        write(
            &stores
                .claude_projects_dir
                .join("workspace/sess-claude.jsonl"),
            r#"{"timestamp":"2026-07-29T00:00:00Z","message":{"id":"m1","model":"claude-opus-4-8","usage":{"input_tokens":10,"output_tokens":0,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}"#,
        );
        write(
            &stores.codex_dir.join("sessions/rollout-codex.jsonl"),
            concat!(
                "{\"timestamp\":\"2026-07-29T00:00:00Z\",\"type\":\"session_meta\",\"payload\":{\"id\":\"codex-meta\",\"cwd\":\"C:\\\\repo\"}}\n",
                "{\"timestamp\":\"2026-07-29T00:00:01Z\",\"type\":\"turn_context\",\"payload\":{\"model\":\"gpt-5-codex\"}}\n",
                "{\"timestamp\":\"2026-07-29T00:00:02Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"info\":{\"total_token_usage\":{\"input_tokens\":10,\"cached_input_tokens\":0,\"output_tokens\":2}}}}\n",
            ),
        );
        let copilot = Connection::open(&stores.copilot_db).unwrap();
        copilot
            .execute_batch(
                "CREATE TABLE assistant_usage_events (
                    id INTEGER PRIMARY KEY AUTOINCREMENT, session_id TEXT, turn_index INTEGER,
                    model TEXT, input_tokens INTEGER, output_tokens INTEGER,
                    cache_read_tokens INTEGER, cache_write_tokens INTEGER,
                    reasoning_tokens INTEGER, reasoning_effort TEXT,
                    token_details_json TEXT, created_at TEXT);
                 CREATE TABLE sessions (id TEXT PRIMARY KEY, cwd TEXT);
                 INSERT INTO assistant_usage_events
                    (session_id, turn_index, model, input_tokens, output_tokens,
                     cache_read_tokens, cache_write_tokens, reasoning_tokens, created_at)
                 VALUES ('sess-copilot', 0, 'claude-sonnet-5', 10, 2, 0, 0, 0,
                         '2026-07-29T00:00:00Z');",
            )
            .unwrap();
        drop(copilot);
        write(
            &stores.cursor_dir.join("chats/hash/sess-cursor/meta.json"),
            r#"{"schemaVersion":1,"createdAtMs":1784593842510,"hasConversation":true,"updatedAtMs":1784593855173,"cwd":"C:\\repo"}"#,
        );
        write(
            &stores
                .gemini_dir
                .join("tmp/project/chats/sess-gemini.jsonl"),
            concat!(
                "{\"sessionId\":\"sess-gemini\",\"startTime\":\"2026-07-29T00:00:00Z\",\"lastUpdated\":\"2026-07-29T00:00:01Z\",\"kind\":\"main\"}\n",
                "{\"id\":\"turn\",\"type\":\"gemini\",\"tokens\":{\"input\":10,\"output\":2,\"cached\":0,\"thoughts\":0,\"tool\":0,\"total\":12},\"model\":\"gemini-3.5-flash\"}\n",
            ),
        );
        write(
            &stores.gemini_dir.join("tmp/project/.project_root"),
            "C:\\repo",
        );
        write(
            &stores
                .kimi_code_dir
                .join("sessions/ws/sess-kimi/agents/main/wire.jsonl"),
            r#"{"type":"usage.record","model":"kimi-code/kimi-for-coding","usage":{"inputOther":10,"output":2,"inputCacheRead":0,"inputCacheCreation":0},"usageScope":"turn","time":1780319377010}"#,
        );
        let opencode = Connection::open(&stores.opencode_db).unwrap();
        opencode
            .execute_batch(
                "CREATE TABLE message (id TEXT, session_id TEXT, data TEXT);
                 CREATE TABLE session (id TEXT, directory TEXT);
                 INSERT INTO message VALUES (
                    'm1', 'sess-opencode',
                    '{\"role\":\"assistant\",\"modelID\":\"k2p6\",\"tokens\":{\"input\":10,\"output\":2}}'
                 );",
            )
            .unwrap();
        drop(opencode);

        let candidates = [
            ("claude", "sess-claude", Some("claude-opus-4-8")),
            ("codex", "rollout-codex", Some("gpt-5-codex")),
            ("copilot", "sess-copilot", Some("claude-sonnet-5")),
            ("cursor", "sess-cursor", None),
            ("gemini", "sess-gemini", Some("gemini-3.5-flash")),
            ("kimi", "sess-kimi", Some("kimi-for-coding")),
            ("opencode", "sess-opencode", Some("k2p6")),
        ];
        let requests: Vec<_> = candidates
            .iter()
            .map(|(agent, session, _)| RecoveryCandidate::new(*agent, *session))
            .collect();
        let results = resolve_models(&requests, &stores);

        assert_eq!(results.len(), candidates.len(), "one result per candidate");
        for (result, (_, _, expected)) in results.iter().zip(candidates) {
            assert_eq!(
                result.model.as_deref(),
                expected,
                "candidate {:?}",
                result.candidate
            );
        }
    }
}
