use super::*;
use std::collections::HashSet;

/// The header record, verbatim in shape from the spike's captured store
/// (`docs/research/gemini-cli-adapter-spike.md`).
const HEADER: &str = concat!(
    r#"{"sessionId":"ralphy-probe-p1p2p3p4p6","projectHash":"3c489ab0","#,
    r#""startTime":"2026-07-21T00:56:00Z","lastUpdated":"2026-07-21T01:00:00Z","kind":"main"}"#
);

/// One assistant turn, reproduced VERBATIM from the spike's captured record.
/// A format change reds against the same bytes the spike observed.
const TURN: &str = concat!(
    r#"{"id":"78d80d17","type":"gemini","content":"OK","#,
    r#""tokens":{"input":20637,"output":30,"cached":0,"thoughts":257,"tool":0,"#,
    r#""total":20924},"model":"gemini-3.1-pro-preview-customtools"}"#
);

/// A turn with a REAL cache hit, copied verbatim off this host's live store
/// (`~/.gemini/tmp/*/chats/`). The arithmetic that matters:
/// `14134 + 37 + 218 = 14389`, so `total` carries `thoughts`, AND
/// `cached: 8133 < input: 14134`, so `input` carries the cached subset.
const CACHED_TURN: &str = concat!(
    r#"{"id":"c1","type":"gemini","content":"OK","#,
    r#""tokens":{"input":14134,"output":37,"cached":8133,"thoughts":218,"tool":0,"#,
    r#""total":14389},"model":"gemini-3.5-flash"}"#
);

/// A subagent turn on its own model, from the same spike observation
/// (17 595 tokens on `gemini-3.5-flash`).
const SUBAGENT: &str = concat!(
    r#"{"sessionId":"78d80d17-sub","startTime":"2026-07-21T00:58:00Z","#,
    r#""lastUpdated":"2026-07-21T00:59:00Z","kind":"subagent"}"#,
    "
",
    r#"{"id":"s1","type":"gemini","content":"OK","#,
    r#""tokens":{"input":17000,"output":95,"cached":0,"thoughts":500,"tool":0,"#,
    r#""total":17595},"model":"gemini-3.5-flash"}"#
);

fn seed(base: &Path, project: &str, root: &str, rel: &str, body: &str) {
    let file = base.join("tmp").join(project).join("chats").join(rel);
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::write(&file, body).unwrap();
    fs::write(base.join("tmp").join(project).join(".project_root"), root).unwrap();
}

fn scan(base: &Path) -> Vec<InteractiveRecord> {
    scan_gemini(&GeminiScan {
        gemini_dir: base,
        run_session_ids: &HashSet::new(),
        repos: &[],
        since: None,
    })
}

#[test]
fn a_single_turn_folds_total_minus_input_as_billable_output() {
    let tmp = tempfile::tempdir().unwrap();
    seed(
        tmp.path(),
        "fincal",
        "c:\\dev\\fincal",
        "session-x.jsonl",
        &format!("{HEADER}\n{TURN}\n"),
    );

    let records = scan(tmp.path());
    assert_eq!(records.len(), 1, "{records:?}");
    assert_eq!(records[0].agent, "gemini");
    assert_eq!(records[0].session_id, "ralphy-probe-p1p2p3p4p6");
    assert_eq!(records[0].model, "gemini-3.1-pro-preview-customtools");
    assert_eq!(
        records[0].tokens,
        Some(Tokens {
            input: 20637,
            // 20924 − 20637: the bare `output` field (30) cannot produce it.
            output: 287,
            cache_read: 0,
            cache_creation: 0,
        })
    );
    assert_eq!(records[0].first_ts, "2026-07-21T00:56:00Z");
    assert_eq!(records[0].last_ts, "2026-07-21T01:00:00Z");
    // ADR-0043 D10: the silent `utility_router` call is never on disk, so
    // every Gemini record is a floor and must say so to its consumers.
    assert!(records[0].lower_bound, "a gemini record is a lower bound");
}

/// ADR-0040 C6's bill-multiplier trap in the opposite direction: a keep-last
/// implementation returns `20637`/`287` here and reds.
///
/// The two turns are DIFFERENT records on DIFFERENT models, so this also reds
/// a fold that folds one turn and multiplies by the turn count, and one that
/// mis-keys the per-model aggregate — both of which two byte-identical turns
/// would satisfy.
#[test]
fn usage_is_summed_not_kept_last() {
    let tmp = tempfile::tempdir().unwrap();
    seed(
        tmp.path(),
        "fincal",
        "c:\\dev\\fincal",
        "session-x.jsonl",
        &format!("{HEADER}\n{TURN}\n{TURN}\n{CACHED_TURN}\n"),
    );

    let mut records = scan(tmp.path());
    // One session, two models → two records, both under the same session id.
    assert_eq!(records.len(), 2, "{records:?}");
    records.sort_by(|a, b| a.model.cmp(&b.model));
    assert!(records
        .iter()
        .all(|r| r.session_id == "ralphy-probe-p1p2p3p4p6"));

    let pro = records[0].tokens.clone().unwrap();
    assert_eq!(records[0].model, "gemini-3.1-pro-preview-customtools");
    assert_eq!(pro.input, 41274, "two identical turns SUM");
    assert_eq!(pro.output, 574);

    let flash = records[1].tokens.clone().unwrap();
    assert_eq!(records[1].model, "gemini-3.5-flash");
    assert_eq!(
        (flash.input, flash.output, flash.cache_read),
        (6001, 255, 8133),
        "the third turn's model must keep its OWN aggregate"
    );
}

/// `Tokens`' four buckets are DISJOINT — every consumer sums them
/// (`app.js` renders `input + output + cache_read + cache_creation`). This
/// store's `input` INCLUDES `cached`, so the cached subset must be subtracted
/// out of `input`, the way `codex.rs` does. A fold that reports the raw
/// `input` alongside `cache_read` bills the cached tokens twice; one that
/// drops `cache_read` loses them. Both red here, and no `cached: 0` fixture
/// can discriminate either.
#[test]
fn a_cache_hit_is_reported_once_not_folded_into_input_as_well() {
    let tmp = tempfile::tempdir().unwrap();
    seed(
        tmp.path(),
        "fincal",
        "c:\\dev\\fincal",
        "session-x.jsonl",
        &format!("{HEADER}\n{CACHED_TURN}\n"),
    );

    let records = scan(tmp.path());
    assert_eq!(records.len(), 1, "{records:?}");
    let tokens = records[0].tokens.clone().unwrap();
    assert_eq!(
        tokens,
        Tokens {
            input: 6001,      // 14134 − 8133: the FRESH prompt only
            output: 255,      // 14389 − 14134: output + thoughts
            cache_read: 8133, // reported here, and only here
            cache_creation: 0,
        }
    );
    // The whole turn is still accounted for: nothing was lost by splitting it.
    assert_eq!(
        tokens.input + tokens.output + tokens.cache_read,
        14389,
        "the disjoint buckets must still sum to the store's own `total`"
    );
}

/// A turn with no `total` cannot be differenced. Reconstructing the sum from
/// the parts keeps its output; a bare `saturating_sub` clamps output to 0 and
/// silently reports input-only spend.
#[test]
fn a_turn_missing_total_reconstructs_its_output_from_the_parts() {
    let tmp = tempfile::tempdir().unwrap();
    seed(
        tmp.path(),
        "fincal",
        "c:\\dev\\fincal",
        "session-x.jsonl",
        &format!(
            "{HEADER}\n{}\n",
            concat!(
                r#"{"id":"n1","type":"gemini","tokens":{"input":100,"output":7,"#,
                r#""cached":0,"thoughts":11},"model":"m"}"#
            )
        ),
    );

    let tokens = scan(tmp.path())[0].tokens.clone().unwrap();
    assert_eq!(tokens.input, 100);
    assert_eq!(tokens.output, 18, "output + thoughts, not a clamped 0");
}

#[test]
fn a_nested_subagent_file_is_counted() {
    let tmp = tempfile::tempdir().unwrap();
    seed(
        tmp.path(),
        "fincal",
        "c:\\dev\\fincal",
        "session-x.jsonl",
        &format!("{HEADER}\n{TURN}\n"),
    );
    seed(
        tmp.path(),
        "fincal",
        "c:\\dev\\fincal",
        "ralphy-probe-policy/78d80d17.jsonl",
        SUBAGENT,
    );

    // A `chats/*.jsonl` glob returns one record and reds here.
    let records = scan(tmp.path());
    assert_eq!(records.len(), 2, "{records:?}");
    let sub = records
        .iter()
        .find(|r| r.model == "gemini-3.5-flash")
        .expect("the nested subagent log must contribute its own record");
    assert_eq!(sub.session_id, "78d80d17-sub");
    assert_eq!(sub.tokens.clone().unwrap().output, 595);
}

#[test]
fn a_missing_or_malformed_store_is_empty_never_an_error() {
    assert!(scan(Path::new("does-not-exist")).is_empty());

    let empty = tempfile::tempdir().unwrap();
    fs::create_dir_all(empty.path().join("tmp")).unwrap();
    assert!(scan(empty.path()).is_empty());

    let garbage = tempfile::tempdir().unwrap();
    seed(
        garbage.path(),
        "fincal",
        "c:\\dev\\fincal",
        "session-x.jsonl",
        "not json\nnot json either\n",
    );
    assert!(scan(garbage.path()).is_empty());
}

/// `lastUpdated` lives on `$set` mutation records after the header — trusting
/// the header's copy alone dates every live session to its first second.
#[test]
fn last_ts_follows_the_set_mutations_not_just_the_header() {
    let tmp = tempfile::tempdir().unwrap();
    seed(
        tmp.path(),
        "fincal",
        "c:\\dev\\fincal",
        "session-x.jsonl",
        &format!(
            "{HEADER}\n{TURN}\n{}\n",
            r#"{"$set":{"lastUpdated":"2026-07-21T02:00:00Z"}}"#
        ),
    );

    let records = scan(tmp.path());
    assert_eq!(records[0].last_ts, "2026-07-21T02:00:00Z");
}

#[test]
fn the_earliest_start_time_is_the_first_ts() {
    let lines = concat!(
        r#"{"sessionId":"s","startTime":"2026-07-21T00:58:00Z"}"#,
        "\n",
        r#"{"startTime":"2026-07-21T00:56:00Z"}"#,
        "\n",
        r#"{"startTime":"2026-07-21T00:57:00Z"}"#,
    );
    let fold = fold_session(lines).expect("a header names the session");
    assert_eq!(fold.first_ts, "2026-07-21T00:56:00Z");
}

#[test]
fn a_log_with_no_session_id_folds_to_no_session() {
    assert!(fold_session(&format!(
        "{TURN}
"
    ))
    .is_none());
}

#[test]
fn last_ts_falls_back_to_first_ts_when_no_last_updated_is_seen() {
    let lines = r#"{"sessionId":"s","startTime":"2026-07-21T00:56:00Z"}"#;
    let fold = fold_session(lines).expect("a header names the session");
    assert_eq!(fold.last_ts, "2026-07-21T00:56:00Z");
}

#[test]
fn a_session_is_attributed_through_its_project_root_sibling() {
    let tmp = tempfile::tempdir().unwrap();
    seed(
        tmp.path(),
        "fincal",
        "c:\\dev\\fincal",
        "session-x.jsonl",
        &format!("{HEADER}\n{TURN}\n"),
    );

    let records = scan_gemini(&GeminiScan {
        gemini_dir: tmp.path(),
        run_session_ids: &HashSet::new(),
        repos: &[crate::RegisteredRepo {
            slug: "acme/fincal".to_string(),
            // `.project_root` holds the lowercased Windows form: the match
            // must cross both the separator and the case difference.
            path: "C:/Dev/FinCal/".to_string(),
        }],
        since: None,
    });
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].project.as_deref(), Some("acme/fincal"));
}

#[test]
fn a_project_root_in_a_linked_worktree_is_attributed_to_its_repo() {
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
    let wt_native = wt.to_string_lossy().to_string();

    seed(
        tmp.path(),
        "wt",
        &wt_native,
        "session-wt.jsonl",
        &format!("{HEADER}\n{TURN}\n"),
    );

    let records = scan_gemini(&GeminiScan {
        gemini_dir: tmp.path(),
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

#[test]
fn a_run_owned_session_id_is_excluded() {
    let tmp = tempfile::tempdir().unwrap();
    seed(
        tmp.path(),
        "fincal",
        "c:\\dev\\fincal",
        "session-x.jsonl",
        &format!("{HEADER}\n{TURN}\n"),
    );
    let owned: HashSet<String> = ["ralphy-probe-p1p2p3p4p6".to_string()]
        .into_iter()
        .collect();

    let records = scan_gemini(&GeminiScan {
        gemini_dir: tmp.path(),
        run_session_ids: &owned,
        repos: &[],
        since: None,
    });
    assert!(records.is_empty(), "{records:?}");
}

/// §6: `since` is INCLUSIVE at the boundary, and an unparseable bound must
/// not filter at all.
#[test]
fn since_is_inclusive_and_never_filters_on_an_unparseable_bound() {
    let tmp = tempfile::tempdir().unwrap();
    seed(
        tmp.path(),
        "fincal",
        "c:\\dev\\fincal",
        "session-x.jsonl",
        &format!("{HEADER}\n{TURN}\n"),
    );

    let with_since = |since: &str| {
        scan_gemini(&GeminiScan {
            gemini_dir: tmp.path(),
            run_session_ids: &HashSet::new(),
            repos: &[],
            since: Some(since),
        })
    };
    assert_eq!(with_since("2026-07-21T01:00:00Z").len(), 1, "inclusive");
    assert_eq!(with_since("not-a-timestamp").len(), 1);
    assert!(with_since("2026-07-21T01:00:01Z").is_empty());
}

#[test]
fn the_scan_writes_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    seed(
        tmp.path(),
        "fincal",
        "c:\\dev\\fincal",
        "session-x.jsonl",
        &format!("{HEADER}\n{TURN}\n"),
    );
    let before = fs::read_to_string(
        tmp.path()
            .join("tmp/fincal/chats/session-x.jsonl")
            .to_string_lossy()
            .to_string(),
    )
    .unwrap();
    let meta = fs::metadata(tmp.path().join("tmp/fincal/chats/session-x.jsonl")).unwrap();

    assert_eq!(scan(tmp.path()).len(), 1);
    let after = fs::metadata(tmp.path().join("tmp/fincal/chats/session-x.jsonl")).unwrap();
    assert_eq!(after.modified().unwrap(), meta.modified().unwrap());
    assert_eq!(
        fs::read_to_string(tmp.path().join("tmp/fincal/chats/session-x.jsonl")).unwrap(),
        before
    );
}
