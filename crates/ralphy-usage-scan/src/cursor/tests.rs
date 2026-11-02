use super::*;
use std::collections::HashSet;

/// The verbatim `meta.json` shape read from the live store on this host.
const META: &str = concat!(
    r#"{"schemaVersion":1,"createdAtMs":1784593842510,"hasConversation":true,"#,
    r#""updatedAtMs":1784593855173,"cwd":"C:\\Dev\\FinCal"}"#
);

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
    assert!(
        !records[0].lower_bound,
        "only a vendor that hides spend on disk (ADR-0043 D10) flags a floor"
    );
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
    assert!(
        !only.lower_bound,
        "the transcripts site flags no floor either"
    );
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
        concat!(
            r#"{"schemaVersion":1,"createdAtMs":1735689600000,"updatedAtMs":1735689600000,"#,
            r#""cwd":"C:\\Dev\\FinCal"}"#
        ),
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
            // `meta.json`'s cwd is the Windows `C:\Dev\FinCal`:
            // `attribution::find_repo` must match it across the separator
            // and case difference.
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

#[test]
fn a_chats_session_in_a_linked_worktree_is_attributed_to_its_repo() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    let repo_fwd = repo.to_string_lossy().replace('\\', "/");
    let wt = tmp.path().join("wt");
    fs::create_dir_all(&wt).unwrap();
    fs::write(
        wt.join(".git"),
        format!("gitdir: {repo_fwd}/.git/worktrees/wt\n"),
    )
    .unwrap();
    let wt_fwd = wt.to_string_lossy().replace('\\', "/");

    seed_chat_json(
        tmp.path(),
        "33333333-3333-3333-3333-333333333333",
        &format!(
            "{{\"schemaVersion\":1,\"createdAtMs\":1784593842510,\"updatedAtMs\":1784593855173,\
             \"cwd\":\"{wt_fwd}\"}}"
        ),
    );

    let records = scan_cursor(&CursorScan {
        cursor_dir: tmp.path(),
        run_session_ids: &HashSet::new(),
        repos: &[crate::RegisteredRepo {
            slug: "o/repo".to_string(),
            path: repo.to_string_lossy().to_string(),
        }],
        since: None,
    });
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].project.as_deref(), Some("o/repo"));
}
