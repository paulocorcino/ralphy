//! The Cursor module of the usage scan (ADR-0033 §2/§6, ADR-0042 D11). Enumerates
//! the interactive sessions Cursor left on disk under `~/.cursor/` and reports
//! each one with **no token count at all**.
//!
//! Tokens are `None`, not zero: Cursor bills in dollar-denominated credits and
//! records no per-session token totals anywhere in either store (ADR-0042 D11 —
//! the resolved model id and any counts live only in the content-addressed blob
//! graph, which this scan deliberately does not walk). A zeroed [`Tokens`](crate::Tokens) would
//! ship `0` on the wire and read as "this session spent nothing"; `None` forces
//! every consumer to render it as unavailable. `model` is the literal `"unknown"`
//! for the same reason.

use std::collections::{BTreeMap, HashMap};
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
    // slug → resolved git actor email, computed at most once per attributed repo
    // (mirrors `claude.rs`/`opencode.rs`): the resolver spawns `git`, and Cursor
    // stores routinely hold hundreds of sessions in one repo.
    let mut email_cache: HashMap<String, Option<String>> = HashMap::new();
    // Order is load-bearing: `chats` first, and the transcripts leg only fills
    // ids it did not already claim, so a session in both stores keeps the
    // `meta.json` timestamps and cwd.
    scan_chats(input, &mut by_id, &mut email_cache);
    scan_transcripts(input, &mut by_id, &mut email_cache);

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
fn scan_chats(
    input: &CursorScan,
    out: &mut BTreeMap<String, InteractiveRecord>,
    email_cache: &mut HashMap<String, Option<String>>,
) {
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
            let (project, actor_email) = attribute(input, email_cache, |r| {
                cwd.is_some_and(|c| paths_eq(&r.path, c))
            });
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

/// Walk `<cursor_dir>/projects/<slug>/agent-transcripts/<sid>/<sid>.jsonl`. Six ids
/// live only here (measured on this host), so scanning `chats` alone would hide
/// them. This store carries no machine-readable instant — its only in-band
/// timestamp is human prose inside a user message — so the `<sid>.jsonl` mtime is
/// the honest cross-platform floor for both ends of the span.
fn scan_transcripts(
    input: &CursorScan,
    out: &mut BTreeMap<String, InteractiveRecord>,
    email_cache: &mut HashMap<String, Option<String>>,
) {
    let Ok(projects) = fs::read_dir(input.cursor_dir.join("projects")) else {
        return;
    };
    for project_dir in projects.flatten() {
        let slug_dir = project_dir.file_name().to_string_lossy().to_string();
        let (project, actor_email) = attribute(input, email_cache, |r| {
            cursor_project_slug(&r.path).eq_ignore_ascii_case(&slug_dir)
        });
        let Ok(sessions) = fs::read_dir(project_dir.path().join("agent-transcripts")) else {
            continue;
        };
        for session in sessions.flatten() {
            let session_id = session.file_name().to_string_lossy().to_string();
            if input.run_session_ids.contains(&session_id) || out.contains_key(&session_id) {
                continue;
            }
            let transcript = session.path().join(format!("{session_id}.jsonl"));
            if !transcript.is_file() {
                continue;
            }
            let ts = ms_to_rfc3339(mtime_ms(&transcript));
            out.insert(
                session_id.clone(),
                InteractiveRecord {
                    agent: "cursor".to_string(),
                    model: "unknown".to_string(),
                    session_id,
                    project: project.clone(),
                    actor_email: actor_email.clone(),
                    tokens: None,
                    first_ts: ts.clone(),
                    last_ts: ts,
                },
            );
        }
    }
}

/// A file's mtime as unix ms; `None` on any metadata or range error.
fn mtime_ms(path: &Path) -> Option<i64> {
    let modified = fs::metadata(path).and_then(|m| m.modified()).ok()?;
    let dur = modified
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_millis();
    i64::try_from(dur).ok()
}

/// Cursor's `projects/` directory-name encoding of a workspace path: every
/// non-alphanumeric byte becomes `-` and consecutive `-` COLLAPSE — Cursor names
/// `C:\Dev\FinCal` as `C-Dev-FinCal`, not Claude's `C--Dev-FinCal`
/// (`claude.rs::dashed_cwd`), so the two encodings cannot share one helper.
fn cursor_project_slug(path: &str) -> String {
    // Trailing separators are trimmed first, mirroring `normalize_path`: a repo
    // registered as `C:\Dev\FinCal\` must slug to the same `C-Dev-FinCal`.
    let mut out = String::with_capacity(path.len());
    for ch in path.trim_end_matches(['/', '\\']).chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    out
}

/// `(project slug, git actor email)` for the first registered repo `matches`
/// accepts; `(None, None)` when none does (§6: reported, never dropped). The
/// email is resolved through `cache`, so one `git` spawn serves every session of
/// a repo rather than one per session.
fn attribute(
    input: &CursorScan,
    cache: &mut HashMap<String, Option<String>>,
    matches: impl Fn(&crate::RegisteredRepo) -> bool,
) -> (Option<String>, Option<String>) {
    match input.repos.iter().find(|r| matches(r)) {
        Some(r) => (
            Some(r.slug.clone()),
            cache
                .entry(r.slug.clone())
                .or_insert_with(|| repo_actor_email(&r.path))
                .clone(),
        ),
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

    /// `createdAtMs` / `updatedAtMs` of [`META`], as the scan renders them. They
    /// differ, so a leg that filled both ends from one field is caught.
    const META_FIRST_TS: &str = "2026-07-21T00:30:42.510+00:00";
    const META_LAST_TS: &str = "2026-07-21T00:30:55.173+00:00";

    fn seed_chat(base: &Path, sid: &str) {
        seed_chat_json(base, sid, META);
    }

    fn seed_chat_json(base: &Path, sid: &str, meta: &str) {
        let dir = base.join("chats").join("aaaa").join(sid);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("meta.json"), meta).unwrap();
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
        assert_eq!(
            records[0].last_ts, META_LAST_TS,
            "`last_ts` must come from `updatedAtMs` — it is what `since` filters on"
        );
        assert_eq!(records[0].first_ts, META_FIRST_TS);
    }

    #[test]
    fn an_absent_store_returns_an_empty_vec() {
        assert!(scan(Path::new("does-not-exist")).is_empty());
    }

    fn seed_transcript(base: &Path, sid: &str) {
        let dir = base
            .join("projects")
            .join("C-Dev-FinCal")
            .join("agent-transcripts")
            .join(sid);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join(format!("{sid}.jsonl")),
            "{\"type\":\"turn_ended\",\"status\":\"success\"}\n",
        )
        .unwrap();
    }

    #[test]
    fn a_session_only_in_the_transcripts_store_is_still_enumerated() {
        let tmp = tempfile::tempdir().unwrap();
        seed_chat(tmp.path(), "11111111-1111-1111-1111-111111111111");
        seed_transcript(tmp.path(), "22222222-2222-2222-2222-222222222222");

        let records = scan(tmp.path());
        assert_eq!(records.len(), 2, "{records:?}");
        let only = records
            .iter()
            .find(|r| r.session_id == "22222222-2222-2222-2222-222222222222")
            .expect("the transcripts-only session must be enumerated");
        assert_eq!(only.tokens, None);
    }

    #[test]
    fn a_session_in_both_stores_yields_exactly_one_record() {
        let tmp = tempfile::tempdir().unwrap();
        let sid = "11111111-1111-1111-1111-111111111111";
        seed_chat(tmp.path(), sid);
        seed_transcript(tmp.path(), sid);

        let records = scan(tmp.path());
        assert_eq!(records.len(), 1, "{records:?}");
        // The exact `createdAtMs` instant — the mtime is now, so this pins WHICH
        // store won rather than merely that a timestamp exists.
        assert_eq!(records[0].first_ts, META_FIRST_TS);
        assert_eq!(records[0].project, None);
    }

    #[test]
    fn cursor_project_slug_matches_the_live_encoding() {
        assert_eq!(cursor_project_slug("C:\\Dev\\FinCal"), "C-Dev-FinCal");
        assert_eq!(
            cursor_project_slug("C:\\Dev\\FinCal\\"),
            "C-Dev-FinCal",
            "a registered path with a trailing separator must slug the same"
        );
        assert_eq!(
            cursor_project_slug("C:\\Users\\PICHAU\\AppData\\Local\\Temp\\cursorlab-a"),
            "C-Users-PICHAU-AppData-Local-Temp-cursorlab-a"
        );
    }

    /// Every relative path under `base` with its file length AND mtime — the
    /// fingerprint `the_scan_writes_nothing` compares across the scan. The mtime
    /// is load-bearing: the transcripts leg derives its whole timestamp span from
    /// it, so a scan that merely TOUCHED a transcript would corrupt the span
    /// while leaving every byte length identical.
    fn snapshot(base: &Path) -> Vec<(String, u64, std::time::SystemTime)> {
        let mut out = Vec::new();
        let mut stack = vec![base.to_path_buf()];
        while let Some(dir) = stack.pop() {
            for entry in fs::read_dir(&dir).unwrap().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else {
                    let rel = path
                        .strip_prefix(base)
                        .unwrap()
                        .to_string_lossy()
                        .to_string();
                    let meta = fs::metadata(&path).unwrap();
                    out.push((rel, meta.len(), meta.modified().unwrap()));
                }
            }
        }
        out.sort();
        out
    }

    #[test]
    fn the_scan_writes_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        seed_chat(tmp.path(), "11111111-1111-1111-1111-111111111111");
        seed_transcript(tmp.path(), "22222222-2222-2222-2222-222222222222");

        let before = snapshot(tmp.path());
        let records = scan(tmp.path());
        assert_eq!(records.len(), 2);
        assert_eq!(snapshot(tmp.path()), before, "the scan is read-only");
    }

    #[test]
    fn since_drops_an_older_session_and_keeps_a_newer_one() {
        let tmp = tempfile::tempdir().unwrap();
        // 1735689600000 = 2025-01-01T00:00:00Z; the transcript's mtime is now.
        seed_chat_json(
            tmp.path(),
            "11111111-1111-1111-1111-111111111111",
            r#"{"schemaVersion":1,"createdAtMs":1735689600000,"updatedAtMs":1735689600000,"cwd":"C:\\Dev\\FinCal"}"#,
        );
        seed_transcript(tmp.path(), "22222222-2222-2222-2222-222222222222");

        let records = scan_cursor(&CursorScan {
            cursor_dir: tmp.path(),
            run_session_ids: &HashSet::new(),
            repos: &[],
            since: Some("2026-01-01T00:00:00Z"),
        });
        assert_eq!(records.len(), 1, "{records:?}");
        assert_eq!(
            records[0].session_id,
            "22222222-2222-2222-2222-222222222222"
        );
    }

    /// §6: `since` is INCLUSIVE at the boundary, and neither an unparseable bound
    /// nor a record with no timestamp may hide a session.
    #[test]
    fn since_is_inclusive_and_never_hides_on_a_parse_miss() {
        let tmp = tempfile::tempdir().unwrap();
        seed_chat(tmp.path(), "11111111-1111-1111-1111-111111111111");
        // No `updatedAtMs` → an empty `last_ts`, which cannot parse.
        seed_chat_json(
            tmp.path(),
            "44444444-4444-4444-4444-444444444444",
            r#"{"schemaVersion":1,"hasConversation":true}"#,
        );

        let with_since = |since: &str| {
            scan_cursor(&CursorScan {
                cursor_dir: tmp.path(),
                run_session_ids: &HashSet::new(),
                repos: &[],
                since: Some(since),
            })
        };
        // Exactly `last_ts`: an off-by-one `>` would drop it.
        assert_eq!(with_since(META_LAST_TS).len(), 2, "boundary is inclusive");
        assert_eq!(
            with_since("not-a-timestamp").len(),
            2,
            "an unparseable bound must not filter at all"
        );
        // One millisecond past: only the unparseable record survives.
        let past = with_since("2026-07-21T00:30:55.174+00:00");
        assert_eq!(past.len(), 1, "{past:?}");
        assert_eq!(past[0].session_id, "44444444-4444-4444-4444-444444444444");
    }

    #[test]
    fn a_run_owned_session_id_is_excluded() {
        let tmp = tempfile::tempdir().unwrap();
        seed_chat(tmp.path(), "11111111-1111-1111-1111-111111111111");
        seed_transcript(tmp.path(), "22222222-2222-2222-2222-222222222222");

        // BOTH legs must honour the exclusion: a run whose session lives only
        // under `projects/` is still a run, not interactive usage.
        for owned_id in [
            "11111111-1111-1111-1111-111111111111",
            "22222222-2222-2222-2222-222222222222",
        ] {
            let owned: HashSet<String> = [owned_id.to_string()].into_iter().collect();
            let records = scan_cursor(&CursorScan {
                cursor_dir: tmp.path(),
                run_session_ids: &owned,
                repos: &[],
                since: None,
            });
            assert_eq!(records.len(), 1, "excluding {owned_id}: {records:?}");
            assert_ne!(records[0].session_id, owned_id);
        }
    }

    #[test]
    fn a_chats_session_is_attributed_by_its_verbatim_cwd() {
        let tmp = tempfile::tempdir().unwrap();
        seed_chat(tmp.path(), "11111111-1111-1111-1111-111111111111");

        let records = scan_cursor(&CursorScan {
            cursor_dir: tmp.path(),
            run_session_ids: &HashSet::new(),
            repos: &[crate::RegisteredRepo {
                slug: "acme/fincal".to_string(),
                // `meta.json`'s cwd is the Windows `C:\Dev\FinCal`: `paths_eq`
                // must match it across the separator and case difference.
                path: "c:/dev/fincal/".to_string(),
            }],
            since: None,
        });
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].project.as_deref(), Some("acme/fincal"));
    }

    #[test]
    fn a_transcripts_only_session_is_attributed_by_its_project_slug() {
        let tmp = tempfile::tempdir().unwrap();
        seed_transcript(tmp.path(), "22222222-2222-2222-2222-222222222222");

        let records = scan_cursor(&CursorScan {
            cursor_dir: tmp.path(),
            run_session_ids: &HashSet::new(),
            repos: &[crate::RegisteredRepo {
                slug: "acme/fincal".to_string(),
                path: "C:\\Dev\\FinCal".to_string(),
            }],
            since: None,
        });
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].project.as_deref(), Some("acme/fincal"));
    }
}
