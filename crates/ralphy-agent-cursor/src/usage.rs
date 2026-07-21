//! Cursor token-usage capture from the `{"type":"result"}` stream record
//! (ADR-0042 D11).
//!
//! No local store records tokens on this vendor — the ONLY accounting is the
//! `usage` block Cursor prints on the terminal `result` record of a stream, and
//! that block is per-invocation, not cumulative: two invocations of the same
//! session (captured live at `%TEMP%\cursor-probe\raw\r-t1.json` / `r-t2.json`,
//! P20) each carry their own `18336/16/128/0` and `102/16/18432/0` — summing
//! them is the only reading that does not silently divide the bill.

use std::path::{Path, PathBuf};

use ralphy_core::Usage;

/// Cursor bills in dollar-denominated credits at per-1M-token rates; Ralphy
/// counts tokens. The two numbers are not expected to match, and every run
/// states that once (`note_usage_provenance`).
pub(crate) const CURSOR_CREDIT_NOTE: &str =
    "cursor bills in dollar-denominated credits; ralphy counts tokens — the two numbers are not expected to match";

/// Sum every `{"type":"result"}` record's `usage` block within one stream
/// (ADR-0042 D11): the spike measured exactly one `result` per run, so this is
/// the defensive form of the same rule rather than new plumbing. Seeded from
/// `crate::requested_model_usage(model)` so a stream with no envelope still
/// attributes the requested model at zero tokens. Missing usage fields default
/// to `0`; unparseable lines are skipped.
pub(crate) fn parse_cursor_usage(stdout: &str, model: Option<&str>) -> Usage {
    let mut usage = crate::requested_model_usage(model);
    for line in stdout.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if value.get("type").and_then(|v| v.as_str()) != Some("result") {
            continue;
        }
        let Some(tu) = value.get("usage") else {
            continue;
        };
        let field = |k: &str| tu.get(k).and_then(|v| v.as_u64()).unwrap_or(0);
        usage.input += field("inputTokens");
        usage.output += field("outputTokens");
        usage.cache_read += field("cacheReadTokens");
        usage.cache_creation += field("cacheWriteTokens");
    }
    usage
}

/// Locate the run's own on-disk session store under the scratch
/// `CURSOR_CONFIG_DIR` (D17), scanning `<config_dir>/chats/*/<session_id>`
/// rather than computing the cwd digest — the spike records `<cwd-hash>` as an
/// opaque 32-hex digest of unknown algorithm, so scanning one directory level
/// is the honest reading, not a guessed derivation. `None` when no such
/// directory exists (the store holds no token count either way; absence is
/// normal, not an error).
pub(crate) fn cursor_session_store(config_dir: &Path, session_id: &str) -> Option<PathBuf> {
    let chats = config_dir.join("chats");
    let entries = std::fs::read_dir(&chats).ok()?;
    for entry in entries.flatten() {
        let candidate = entry.path().join(session_id);
        if candidate.exists() {
            return Some(candidate);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// P20 (ADR-0042 D11): two real invocations of the same session,
    /// `18336/16/128/0` and `102/16/18432/0`. Records are incremental, not
    /// cumulative — keeping the last record would report `102/16/18432/0` and
    /// silently divide the bill.
    #[test]
    fn the_two_live_envelopes_are_summed_not_kept_last() {
        let t1 = include_str!("../fixtures/usage-turn1-2026-07-20.json");
        let t2 = include_str!("../fixtures/usage-turn2-2026-07-20.json");
        let u1 = parse_cursor_usage(t1, Some("composer-2.5"));
        let u2 = parse_cursor_usage(t2, Some("composer-2.5"));
        let folded = Usage::fold_usage(&[u1, u2], Some("composer-2.5"));
        assert_eq!(
            folded,
            Usage {
                input: 18438,
                output: 32,
                cache_read: 18560,
                cache_creation: 0,
                model: Some("composer-2.5".into()),
            }
        );
        assert_ne!(
            folded.input, 102,
            "records are incremental (ADR-0042 D11): keeping the last record divides the bill"
        );
    }

    /// Cache reads are priced far below fresh input (ADR-0008 D2) and must land
    /// in their own field, never folded into `input`.
    #[test]
    fn cache_tokens_never_land_in_input() {
        let t2 = include_str!("../fixtures/usage-turn2-2026-07-20.json");
        let usage = parse_cursor_usage(t2, Some("composer-2.5"));
        assert_eq!(usage.input, 102);
        assert_eq!(usage.cache_read, 18432);
        assert_eq!(usage.cache_creation, 0);
    }

    /// A full multi-record stream (16 lines, 15 non-`result`) — proves the
    /// parse ignores everything but the terminal `result` record.
    #[test]
    fn an_envelope_maps_all_four_counters() {
        let stream = include_str!("../fixtures/permission-denied-2026-07-20.jsonl");
        let usage = parse_cursor_usage(stream, Some("composer-2.5"));
        assert_eq!(usage.input, 18484);
        assert_eq!(usage.output, 136);
        assert_eq!(usage.cache_read, 18688);
        assert_eq!(usage.cache_creation, 0);
    }

    /// A truncated capture (zero records) must not fabricate a number nor lose
    /// the requested-model attribution.
    #[test]
    fn a_stream_with_no_envelope_reports_zero_tokens() {
        let stream = include_str!("../fixtures/preflight-rejection-2026-07-20.jsonl");
        let usage = parse_cursor_usage(stream, None);
        assert_eq!(usage.input, 0);
        assert_eq!(usage.output, 0);
        assert_eq!(usage.cache_read, 0);
        assert_eq!(usage.cache_creation, 0);
        assert_eq!(usage.model.as_deref(), Some("auto"));
    }

    #[test]
    fn the_credit_note_names_both_units() {
        assert!(CURSOR_CREDIT_NOTE.contains("credits"));
        assert!(CURSOR_CREDIT_NOTE.contains("tokens"));
        assert!(CURSOR_CREDIT_NOTE.contains("not expected to match"));
    }

    /// The locator must resolve the run's OWN scratch config dir, never the
    /// operator's real `~/.cursor` — a run-scoped store lookup that fell through
    /// to the operator's dir would address the wrong session entirely.
    #[test]
    fn the_store_locator_reads_the_scratch_config_dir() {
        let sid = "4233e5ca-56a3-4d6d-832c-f704789b1756";
        let scratch = tempfile::tempdir().expect("scratch tempdir");
        let scratch_chat = scratch.path().join("chats").join("deadbeef00000000");
        std::fs::create_dir_all(scratch_chat.join(sid)).expect("scratch session dir");
        std::fs::write(scratch_chat.join(sid).join("store.db"), b"").expect("scratch store file");

        let operator = tempfile::tempdir().expect("operator tempdir");
        let operator_chat = operator.path().join("chats").join("cafef00d00000000");
        std::fs::create_dir_all(operator_chat.join(sid)).expect("operator session dir");

        let found =
            cursor_session_store(scratch.path(), sid).expect("scratch session must resolve");
        assert!(found.starts_with(scratch.path()));
        assert!(!found.starts_with(operator.path()));

        assert_eq!(
            cursor_session_store(scratch.path(), "unknown-session"),
            None
        );
    }
}
