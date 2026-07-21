//! The Cursor module of the usage scan (ADR-0033 §2/§6, ADR-0042 D11). Enumerates
//! the interactive sessions Cursor left on disk under `~/.cursor/` and reports
//! each one with **no token count at all**.
//!
//! Tokens are `None`, not zero: Cursor bills in dollar-denominated credits and
//! records no per-session token totals anywhere in either store (ADR-0042 D11 —
//! the resolved model id and any counts live only in the content-addressed blob
//! graph, which this scan deliberately does not walk). A zeroed [`Tokens`] would
//! ship `0` on the wire and read as "this session spent nothing"; `None` forces
//! every consumer to render it as unavailable. `model` is the literal `"unknown"`
//! for the same reason.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use crate::{CursorScan, InteractiveRecord};

/// Scan the Cursor store into interactive records — one per session, always with
/// `tokens: None`. A missing or unreadable `cursor_dir` yields an empty vec (not
/// an error). Sessions whose id is in `run_session_ids` are Ralphy runs', never
/// interactive, and are excluded. `since` drops records whose `last_ts` is
/// strictly before it (§6: an unparseable bound or record keeps the record).
pub fn scan_cursor(input: &CursorScan) -> Vec<InteractiveRecord> {
    // Keyed by session id so the two stores union rather than duplicate.
    let mut by_id: BTreeMap<String, InteractiveRecord> = BTreeMap::new();
    scan_chats(input, &mut by_id);

    let mut records: Vec<InteractiveRecord> = by_id.into_values().collect();
    if let Some(since) = input.since {
        if let Ok(since_dt) = chrono::DateTime::parse_from_rfc3339(since) {
            records.retain(|r| match chrono::DateTime::parse_from_rfc3339(&r.last_ts) {
                Ok(last) => last >= since_dt,
                Err(_) => true, // never hide a session on a parse miss
            });
        }
    }
    records
}

/// Walk `<cursor_dir>/chats/<hash>/<sid>/meta.json`. This store carries real unix-ms
/// timestamps and the verbatim `cwd`, so its record wins any collision with the
/// transcripts store.
fn scan_chats(input: &CursorScan, out: &mut BTreeMap<String, InteractiveRecord>) {
    let Ok(hashes) = fs::read_dir(input.cursor_dir.join("chats")) else {
        return;
    };
    for hash in hashes.flatten() {
        let Ok(sessions) = fs::read_dir(hash.path()) else {
            continue;
        };
        for session in sessions.flatten() {
            let session_id = session.file_name().to_string_lossy().to_string();
            if input.run_session_ids.contains(&session_id) {
                continue;
            }
            let Ok(text) = fs::read_to_string(session.path().join("meta.json")) else {
                continue;
            };
            let Ok(meta) = serde_json::from_str::<serde_json::Value>(&text) else {
                continue;
            };
            let ms = |k: &str| meta.get(k).and_then(|v| v.as_i64());
            let cwd = meta.get("cwd").and_then(|v| v.as_str());
            let (project, actor_email) =
                attribute(input, |r| cwd.is_some_and(|c| paths_eq(&r.path, c)));
            out.insert(
                session_id.clone(),
                InteractiveRecord {
                    agent: "cursor".to_string(),
                    model: "unknown".to_string(),
                    session_id,
                    project,
                    actor_email,
                    tokens: None,
                    first_ts: ms_to_rfc3339(ms("createdAtMs")),
                    last_ts: ms_to_rfc3339(ms("updatedAtMs")),
                },
            );
        }
    }
}

/// `(project slug, git actor email)` for the first registered repo `matches`
/// accepts; `(None, None)` when none does (§6: reported, never dropped).
fn attribute(
    input: &CursorScan,
    matches: impl Fn(&crate::RegisteredRepo) -> bool,
) -> (Option<String>, Option<String>) {
    match input.repos.iter().find(|r| matches(r)) {
        Some(r) => (Some(r.slug.clone()), repo_actor_email(&r.path)),
        None => (None, None),
    }
}

/// A unix-ms instant → RFC3339 UTC; empty string when absent or out of range.
/// Duplicated from `opencode.rs` (ADR-0033 §7 accepts per-vendor duplication).
fn ms_to_rfc3339(ms: Option<i64>) -> String {
    ms.and_then(chrono::DateTime::from_timestamp_millis)
        .map(|d| d.to_rfc3339())
        .unwrap_or_default()
}

/// Normalize a filesystem path for a case-insensitive compare: `\` → `/`, trailing
/// `/` trimmed. Duplicated from `opencode.rs`.
fn normalize_path(p: &str) -> String {
    p.replace('\\', "/").trim_end_matches('/').to_string()
}

/// True when two paths name the same directory. Duplicated from `opencode.rs`.
fn paths_eq(a: &str, b: &str) -> bool {
    normalize_path(a).eq_ignore_ascii_case(&normalize_path(b))
}

/// `git config user.email` for the attributed repo (ADR-0008 D7). `None` on a
/// non-zero exit or empty output. Duplicated from `opencode.rs`.
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    /// The verbatim `meta.json` shape read from the live store on this host.
    const META: &str = r#"{"schemaVersion":1,"createdAtMs":1784593842510,"hasConversation":true,"updatedAtMs":1784593855173,"cwd":"C:\\Dev\\FinCal"}"#;

    fn seed_chat(base: &Path, sid: &str) {
        let dir = base.join("chats").join("aaaa").join(sid);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("meta.json"), META).unwrap();
    }

    fn scan(base: &Path) -> Vec<InteractiveRecord> {
        scan_cursor(&CursorScan {
            cursor_dir: base,
            run_session_ids: &HashSet::new(),
            repos: &[],
            since: None,
        })
    }

    #[test]
    fn chats_meta_yields_a_session_with_tokens_unavailable() {
        let tmp = tempfile::tempdir().unwrap();
        seed_chat(tmp.path(), "11111111-1111-1111-1111-111111111111");

        let records = scan(tmp.path());
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].agent, "cursor");
        assert_eq!(
            records[0].session_id,
            "11111111-1111-1111-1111-111111111111"
        );
        assert_eq!(
            records[0].tokens, None,
            "Cursor records no token count anywhere — unavailable, never zero"
        );
        assert!(records[0].last_ts.starts_with("2026-"), "{:?}", records[0]);
    }

    #[test]
    fn an_absent_store_returns_an_empty_vec() {
        assert!(scan(Path::new("does-not-exist")).is_empty());
    }
}
