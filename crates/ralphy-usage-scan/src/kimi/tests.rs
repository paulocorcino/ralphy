use super::*;
use crate::RegisteredRepo;
use std::collections::HashSet;

fn no_runs() -> HashSet<String> {
    HashSet::new()
}

/// Write `content` to `<root>/<rel>`, creating parents.
fn write_wire(root: &Path, rel: &str, content: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

/// Scan a legacy-only store rooted at `kimi_dir` (kimi-code empty).
fn scan_legacy_only(kimi_dir: &Path) -> Vec<InteractiveRecord> {
    scan_kimi(&KimiScan {
        kimi_dir,
        kimi_code_dir: Path::new("does-not-exist-kimi-code"),
        run_session_ids: &no_runs(),
        repos: &[],
        since: None,
    })
}

/// Scan a kimi-code-only store rooted at `kimi_code_dir` (legacy empty).
fn scan_code_only(kimi_code_dir: &Path) -> Vec<InteractiveRecord> {
    scan_kimi(&KimiScan {
        kimi_dir: Path::new("does-not-exist-kimi"),
        kimi_code_dir,
        run_session_ids: &no_runs(),
        repos: &[],
        since: None,
    })
}

fn status_update(ts: f64, input: i64, output: i64, cache_read: i64, message_id: &str) -> String {
    format!(
        "{{\"timestamp\": {ts}, \"message\": {{\"type\": \"StatusUpdate\", \"payload\": \
        {{\"token_usage\": {{\"input_other\": {input}, \"output\": {output}, \"input_cache_read\": \
        {cache_read}, \"input_cache_creation\": 0}}, \"message_id\": \"{message_id}\"}}}}}}"
    )
}

#[test]
fn legacy_dedups_progressive_status_updates_by_message_id() {
    let tmp = tempfile::tempdir().unwrap();
    // Same message_id, totals 110 then 155 → keep the larger snapshot only.
    let body = format!(
        "{{\"type\": \"metadata\"}}\n{}\n{}",
        status_update(1_770_983_410.0, 100, 10, 0, "msg-p"),
        status_update(1_770_983_420.0, 120, 30, 5, "msg-p"),
    );
    write_wire(tmp.path(), "sessions/GRP/SESS/wire.jsonl", &body);
    let records = scan_legacy_only(tmp.path());
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].tokens.as_ref().unwrap().input, 120);
    assert_eq!(records[0].tokens.as_ref().unwrap().output, 30);
    assert_eq!(records[0].tokens.as_ref().unwrap().cache_read, 5);
    assert_eq!(records[0].session_id, "SESS");
    assert_eq!(records[0].agent, "kimi");
    assert!(
        !records[0].lower_bound,
        "Kimi writes every token to disk — this is a total, not a floor"
    );
}

#[test]
fn legacy_keeps_distinct_and_missing_message_ids_separate() {
    let tmp = tempfile::tempdir().unwrap();
    // Two distinct ids (out 1, 2) + two missing-id (out 3, 4): all summed = 10.
    let missing = |ts: f64, input: i64, output: i64| {
        format!(
            "{{\"timestamp\": {ts}, \"message\": {{\"type\": \"StatusUpdate\", \"payload\": \
            {{\"token_usage\": {{\"input_other\": {input}, \"output\": {output}, \
            \"input_cache_read\": 0, \"input_cache_creation\": 0}}}}}}}}"
        )
    };
    let body = format!(
        "{}\n{}\n{}\n{}",
        status_update(1_770_983_410.0, 10, 1, 0, "msg-1"),
        status_update(1_770_983_420.0, 20, 2, 0, "msg-2"),
        missing(1_770_983_430.0, 30, 3),
        missing(1_770_983_440.0, 40, 4),
    );
    write_wire(tmp.path(), "sessions/GRP/SESS/wire.jsonl", &body);
    let records = scan_legacy_only(tmp.path());
    assert_eq!(records.len(), 1);
    assert_eq!(
        records[0].tokens.as_ref().unwrap().output,
        10,
        "1+2+3+4 all summed"
    );
}

fn usage_record(scope: Option<&str>, input: i64, output: i64, time: i64) -> String {
    let scope = match scope {
        Some(s) => format!(",\"usageScope\":\"{s}\""),
        None => String::new(),
    };
    format!(
        "{{\"type\":\"usage.record\",\"model\":\"kimi-code/kimi-for-coding\",\
        \"usage\":{{\"inputOther\":{input},\"output\":{output},\"inputCacheRead\":0,\
        \"inputCacheCreation\":0}}{scope},\"time\":{time}}}"
    )
}

#[test]
fn kimi_code_counts_only_turn_scope_and_strips_the_model_prefix() {
    let tmp = tempfile::tempdir().unwrap();
    let body = format!(
        "{}\n{}\n{}",
        usage_record(Some("session"), 999, 999, 1_780_319_377_000),
        usage_record(None, 888, 888, 1_780_319_377_005),
        usage_record(Some("turn"), 100, 50, 1_780_319_377_010),
    );
    write_wire(tmp.path(), "sessions/WS/SESS/agents/main/wire.jsonl", &body);
    let records = scan_code_only(tmp.path());
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].tokens.as_ref().unwrap().input, 100);
    assert_eq!(records[0].tokens.as_ref().unwrap().output, 50);
    // The provider prefix of `kimi-code/kimi-for-coding` is stripped.
    assert_eq!(records[0].model, "kimi-for-coding");
}

#[test]
fn subagent_wire_attributed_to_parent_session() {
    let tmp = tempfile::tempdir().unwrap();
    // Parent main file + a subagent file, same session, same model → merged.
    write_wire(
        tmp.path(),
        "sessions/GRP/SESS/wire.jsonl",
        &status_update(1_770_983_410.0, 100, 0, 0, "msg-main"),
    );
    write_wire(
        tmp.path(),
        "sessions/GRP/SESS/subagents/SUB/wire.jsonl",
        &status_update(1_770_983_420.0, 50, 0, 0, "msg-sub"),
    );
    let records = scan_legacy_only(tmp.path());
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].session_id, "SESS");
    assert_eq!(records[0].tokens.as_ref().unwrap().input, 150);
}

#[test]
fn zero_usage_stub_parses_to_empty() {
    let tmp = tempfile::tempdir().unwrap();
    let body = "{\"type\": \"metadata\"}\n{\"timestamp\": 1770983410.0, \"message\": {\"type\": \
    \"StatusUpdate\", \"payload\": {\"token_usage\": {\"input_other\": 0, \"output\": 0, \
    \"input_cache_read\": 0, \"input_cache_creation\": 0}, \"message_id\": \"msg-empty\"}}}";
    write_wire(tmp.path(), "sessions/GRP/SESS/wire.jsonl", body);
    let records = scan_legacy_only(tmp.path());
    assert_eq!(records, Vec::<InteractiveRecord>::new());
}

#[test]
fn excludes_run_owned_sessions() {
    let tmp = tempfile::tempdir().unwrap();
    write_wire(
        tmp.path(),
        "sessions/GRP/RUN/wire.jsonl",
        &status_update(1_770_983_410.0, 10, 1, 0, "m-run"),
    );
    write_wire(
        tmp.path(),
        "sessions/GRP/INT/wire.jsonl",
        &status_update(1_770_983_420.0, 20, 2, 0, "m-int"),
    );
    let mut runs = HashSet::new();
    runs.insert("RUN".to_owned());
    let records = scan_kimi(&KimiScan {
        kimi_dir: tmp.path(),
        kimi_code_dir: Path::new("does-not-exist-kimi-code"),
        run_session_ids: &runs,
        repos: &[],
        since: None,
    });
    assert!(records.iter().any(|r| r.session_id == "INT"));
    assert!(!records.iter().any(|r| r.session_id == "RUN"));
}

#[test]
fn kimi_missing_stores_contribute_zero() {
    let records = scan_kimi(&KimiScan {
        kimi_dir: Path::new("no-such-kimi-dir-anywhere"),
        kimi_code_dir: Path::new("no-such-kimi-code-dir-anywhere"),
        run_session_ids: &no_runs(),
        repos: &[],
        since: None,
    });
    assert_eq!(records, Vec::<InteractiveRecord>::new());
}

#[test]
fn since_drops_an_older_session_and_keeps_one_ending_at_the_bound() {
    let tmp = tempfile::tempdir().unwrap();
    let (old_ms, new_ms) = (1_780_000_000_000, 1_780_319_377_010);
    write_wire(
        tmp.path(),
        "sessions/WS/OLD/agents/main/wire.jsonl",
        &usage_record(Some("turn"), 10, 1, old_ms),
    );
    write_wire(
        tmp.path(),
        "sessions/WS/NEW/agents/main/wire.jsonl",
        &usage_record(Some("turn"), 20, 2, new_ms),
    );
    let with_since = |since: &str| {
        let mut ids: Vec<String> = scan_kimi(&KimiScan {
            kimi_dir: Path::new("does-not-exist-kimi"),
            kimi_code_dir: tmp.path(),
            run_session_ids: &no_runs(),
            repos: &[],
            since: Some(since),
        })
        .into_iter()
        .map(|r| r.session_id)
        .collect();
        ids.sort();
        ids
    };
    assert_eq!(with_since(&ms_to_rfc3339(Some(old_ms + 1))), ["NEW"]);
    assert_eq!(
        with_since(&ms_to_rfc3339(Some(new_ms))),
        ["NEW"],
        "the bound is inclusive"
    );
    assert_eq!(
        with_since("not-a-timestamp"),
        ["NEW", "OLD"],
        "an unparseable bound hides nothing"
    );
}

#[test]
fn legacy_default_model_when_config_absent() {
    let tmp = tempfile::tempdir().unwrap();
    write_wire(
        tmp.path(),
        "sessions/GRP/SESS/wire.jsonl",
        &status_update(1_770_983_410.0, 10, 1, 0, "m"),
    );
    let records = scan_legacy_only(tmp.path());
    assert_eq!(records[0].model, "kimi-for-coding");
}

#[test]
fn legacy_reads_model_from_config() {
    let tmp = tempfile::tempdir().unwrap();
    fs::create_dir_all(tmp.path()).unwrap();
    fs::write(
        tmp.path().join("config.json"),
        "{\"model\": \"kimi-k2-custom\"}",
    )
    .unwrap();
    write_wire(
        tmp.path(),
        "sessions/GRP/SESS/wire.jsonl",
        &status_update(1_770_983_410.0, 10, 1, 0, "m"),
    );
    let records = scan_legacy_only(tmp.path());
    assert_eq!(records[0].model, "kimi-k2-custom");
}

#[test]
fn kimi_code_attributes_workspace_to_registered_repo() {
    let tmp = tempfile::tempdir().unwrap();
    // WORKSPACE segment equals a registered repo path (raw-path treatment).
    write_wire(
        tmp.path(),
        "sessions/wsrepo/SESS/agents/main/wire.jsonl",
        &usage_record(Some("turn"), 10, 5, 1_780_319_377_010),
    );
    let repos = vec![RegisteredRepo {
        slug: "o/wsrepo".into(),
        path: "wsrepo".into(),
    }];
    let records = scan_kimi(&KimiScan {
        kimi_dir: Path::new("does-not-exist-kimi"),
        kimi_code_dir: tmp.path(),
        run_session_ids: &no_runs(),
        repos: &repos,
        since: None,
    });
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].project.as_deref(), Some("o/wsrepo"));
}

#[test]
fn legacy_tie_on_total_is_won_by_the_later_timestamp() {
    let tmp = tempfile::tempdir().unwrap();
    // Same message_id, both totals 100; the later snapshot comes first in the file.
    let body = format!(
        "{}\n{}",
        status_update(1_770_983_420.0, 0, 100, 0, "msg-t"),
        status_update(1_770_983_410.0, 100, 0, 0, "msg-t"),
    );
    write_wire(tmp.path(), "sessions/GRP/SESS/wire.jsonl", &body);
    let records = scan_legacy_only(tmp.path());
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].tokens.as_ref().unwrap().output, 100);
    assert_eq!(records[0].tokens.as_ref().unwrap().input, 0);

    // The same tie with the later snapshot second: the order in the file
    // does not decide, the timestamp does.
    let tmp = tempfile::tempdir().unwrap();
    let body = format!(
        "{}\n{}",
        status_update(1_770_983_410.0, 100, 0, 0, "msg-t"),
        status_update(1_770_983_420.0, 0, 100, 0, "msg-t"),
    );
    write_wire(tmp.path(), "sessions/GRP/SESS/wire.jsonl", &body);
    let records = scan_legacy_only(tmp.path());
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].tokens.as_ref().unwrap().output, 100);
    assert_eq!(records[0].tokens.as_ref().unwrap().input, 0);
}

#[test]
fn legacy_empty_message_id_counts_each_snapshot() {
    let tmp = tempfile::tempdir().unwrap();
    let body = format!(
        "{}\n{}",
        status_update(1_770_983_410.0, 10, 1, 0, ""),
        status_update(1_770_983_420.0, 20, 2, 0, ""),
    );
    write_wire(tmp.path(), "sessions/GRP/SESS/wire.jsonl", &body);
    let records = scan_legacy_only(tmp.path());
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].tokens.as_ref().unwrap().input, 30);
}
