use super::*;
use std::sync::Mutex;

/// `RALPHY_USAGE_DIR` is process-global, so the tests that point it at a temp
/// dir must not run concurrently — one removing it mid-way would send another's
/// read to the real home. Serialize them behind this lock.
static ENV_LOCK: Mutex<()> = Mutex::new(());

fn sample_record() -> LedgerRecord {
    LedgerRecord {
        project: "owner/repo".into(),
        actor_email: "dev@example.com".into(),
        actor_name: "Dev Name".into(),
        ralphy_version: "0.1.0-rc5".into(),
        issue: 42,
        phase: "execute".into(),
        agent: "agent-a".into(),
        model: "model-a".into(),
        session_id: Some("sess-x".into()),
        outcome: "done".into(),
        tokens: Usage {
            input: 100,
            output: 9,
            cache_read: 1710,
            cache_creation: 94,
            model: Some("model-a".into()),
        },
        ts: "2026-06-15T12:34:56+00:00".into(),
    }
}

#[test]
fn record_line_has_all_fields_and_no_cost_or_usd() {
    let line = record_line(&sample_record()).expect("serialize");
    for key in [
        "project",
        "actor_email",
        "actor_name",
        "ralphy_version",
        "issue",
        "phase",
        "agent",
        "model",
        "session_id",
        "outcome",
        "tokens",
        "ts",
    ] {
        assert!(
            line.contains(&format!("\"{key}\"")),
            "record must carry the `{key}` key: {line}"
        );
    }
    // The four token sub-fields are present...
    for key in ["input", "output", "cache_read", "cache_creation"] {
        assert!(line.contains(&format!("\"{key}\"")), "tokens.{key}: {line}");
    }
    // ...but never the model inside `tokens` (it is the top-level field), and
    // never a derived cost — USD is a read-time projection, never stored (D2).
    assert!(
        !line.contains("cost") && !line.contains("usd"),
        "no cost/usd may be written to the ledger: {line}"
    );
}

#[test]
fn sum_tokens_adds_four_fields_across_lines() {
    let jsonl = "\
{\"phase\":\"plan\",\"tokens\":{\"input\":10,\"output\":1,\"cache_read\":100,\"cache_creation\":5}}
{\"phase\":\"execute\",\"tokens\":{\"input\":20,\"output\":2,\"cache_read\":200,\"cache_creation\":7}}
";
    let total = sum_tokens(jsonl);
    assert_eq!(total.input, 30);
    assert_eq!(total.output, 3);
    assert_eq!(total.cache_read, 300);
    assert_eq!(total.cache_creation, 12);
}

#[test]
fn read_rows_parses_good_lines_and_skips_malformed() {
    // Two well-formed lines and one malformed (unparseable) middle line.
    let jsonl = "\
{\"project\":\"owner/repo\",\"actor_email\":\"a@x.io\",\"actor_name\":\"A\",\"ralphy_version\":\"rc5\",\"issue\":42,\"phase\":\"plan\",\"agent\":\"agent-a\",\"model\":\"model-a\",\"outcome\":\"ok\",\"tokens\":{\"input\":10,\"output\":1,\"cache_read\":100,\"cache_creation\":5},\"ts\":\"2026-06-15T12:00:00+00:00\"}
{ this is not valid json
{\"project\":\"owner/repo\",\"actor_email\":\"b@x.io\",\"actor_name\":\"B\",\"ralphy_version\":\"rc5\",\"issue\":42,\"phase\":\"execute\",\"agent\":\"agent-b\",\"model\":\"model-b\",\"outcome\":\"done\",\"tokens\":{\"input\":20,\"output\":2,\"cache_read\":200,\"cache_creation\":7},\"ts\":\"2026-06-15T12:05:00+00:00\"}
";
    let rows = read_rows(jsonl);
    assert_eq!(rows.len(), 2, "malformed middle line is skipped");

    assert_eq!(rows[0].model, "model-a");
    assert_eq!(rows[0].phase, "plan");
    assert_eq!(rows[0].actor_email, "a@x.io");
    assert_eq!(rows[0].issue, 42);
    assert_eq!(rows[0].tokens.input, 10);
    assert_eq!(rows[0].tokens.cache_read, 100);
    assert_eq!(rows[0].ts, "2026-06-15T12:00:00+00:00");

    assert_eq!(rows[1].model, "model-b");
    assert_eq!(rows[1].phase, "execute");
    assert_eq!(rows[1].agent, "agent-b");
    assert_eq!(rows[1].tokens.output, 2);
}

#[test]
fn read_rows_parses_mixed_session_id() {
    // One OLD line lacking `session_id`, one NEW line carrying it — both must
    // parse into a `UsageRow`, neither skipped (additive, append-only safe).
    let jsonl = "\
{\"project\":\"owner/repo\",\"issue\":42,\"phase\":\"plan\",\"agent\":\"a\",\"model\":\"m\",\"outcome\":\"ok\",\"tokens\":{\"input\":1,\"output\":0,\"cache_read\":0,\"cache_creation\":0},\"ts\":\"t\"}
{\"project\":\"owner/repo\",\"issue\":42,\"phase\":\"execute\",\"agent\":\"a\",\"model\":\"m\",\"session_id\":\"sess-b\",\"outcome\":\"done\",\"tokens\":{\"input\":2,\"output\":0,\"cache_read\":0,\"cache_creation\":0},\"ts\":\"t\"}
";
    let rows = read_rows(jsonl);
    assert_eq!(rows.len(), 2);
    assert!(rows[0].session_id.is_none(), "old line has no session_id");
    assert_eq!(rows[1].session_id.as_deref(), Some("sess-b"));
}

#[test]
fn append_then_project_total_round_trips() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    // Point the ledger root at a unique temp dir so production is untouched.
    let dir = std::env::temp_dir().join(format!(
        "ralphy-ledger-{}-{:x}",
        std::process::id(),
        sample_record().issue
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::env::set_var("RALPHY_USAGE_DIR", &dir);

    let mut first = sample_record();
    first.phase = "plan".into();
    first.tokens = Usage {
        input: 1,
        output: 2,
        cache_read: 3,
        cache_creation: 4,
        model: None,
    };
    let mut second = sample_record();
    second.tokens = Usage {
        input: 10,
        output: 20,
        cache_read: 30,
        cache_creation: 40,
        model: None,
    };
    append(&first).expect("append first");
    append(&second).expect("append second");

    let total = project_total(&sample_record().project);
    assert_eq!(total.input, 11);
    assert_eq!(total.output, 22);
    assert_eq!(total.cache_read, 33);
    assert_eq!(total.cache_creation, 44);

    std::env::remove_var("RALPHY_USAGE_DIR");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Issue #269: the run-level `consolidate` line lands with `issue = 0`, counts
/// toward the project total, and is skipped by a per-issue read — while a
/// zero-token pass writes nothing at all.
#[test]
fn append_run_phase_records_a_run_level_consolidate_line() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = std::env::temp_dir().join(format!("ralphy-ledger-runphase-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::env::set_var("RALPHY_USAGE_DIR", &dir);

    let usage = Usage {
        input: 33_398,
        output: 5_444,
        cache_read: 337_152,
        cache_creation: 0,
        model: Some("composer-2.5".into()),
    };
    // A zero-token pass is a no-op: no line, no file.
    append_run_phase(
        "owner/repo",
        "dev@example.com",
        "Dev Name",
        "cursor",
        "consolidate",
        &Usage::default(),
    );
    assert_eq!(
        project_total("owner/repo").total(),
        0,
        "a zero-token consolidation must write nothing"
    );

    append_run_phase(
        "owner/repo",
        "dev@example.com",
        "Dev Name",
        "cursor",
        "consolidate",
        &usage,
    );

    // It counts toward the project total.
    assert_eq!(project_total("owner/repo").total(), usage.total());

    // The line is a run-level `consolidate` phase at issue 0, agent/model intact.
    let rows = read_project_rows("owner/repo");
    assert_eq!(rows.len(), 1, "exactly the one non-zero line was written");
    let row = &rows[0];
    assert_eq!(row.issue, 0, "run-level sentinel");
    assert_eq!(row.phase, "consolidate");
    assert_eq!(row.agent, "cursor");
    assert_eq!(row.model, "composer-2.5");
    assert_eq!(row.outcome, "ok");
    assert_eq!(row.tokens.cache_read, 337_152);

    std::env::remove_var("RALPHY_USAGE_DIR");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn read_project_rows_projects_only_unknown_session_rows_without_rewriting_jsonl() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    std::env::set_var("RALPHY_USAGE_DIR", dir.path());
    let ledger = dir.path().join("owner-repo.jsonl");
    let bytes = concat!(
        "{\"project\":\"owner/repo\",\"model\":\"unknown\",\"session_id\":\"rollout-codex\",\"tokens\":{\"input\":1}}\n",
        "{\"project\":\"owner/repo\",\"model\":\"known\",\"session_id\":\"rollout-codex\",\"tokens\":{\"input\":2}}\n",
        "{\"project\":\"owner/repo\",\"model\":\"unknown\",\"tokens\":{\"input\":3}}\n",
    )
    .as_bytes()
    .to_vec();
    std::fs::write(&ledger, &bytes).unwrap();
    let mut models = crate::SessionModelMap::default();
    models.merge([("rollout-codex".into(), "gpt-5-codex".into())]);
    models
        .persist(&crate::session_model_map_path(dir.path()))
        .unwrap();

    let rows = read_project_rows("owner/repo");

    assert_eq!(rows[0].model, "gpt-5-codex");
    assert_eq!(rows[1].model, "known");
    assert_eq!(rows[2].model, "unknown");
    assert_eq!(std::fs::read(&ledger).unwrap(), bytes);
    std::env::remove_var("RALPHY_USAGE_DIR");
}

#[test]
fn rename_project_moves_rows_and_rewrites_inline_project() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let mut rec = sample_record();
    rec.project = "path-abc".into();
    let good = record_line(&rec).unwrap();
    std::fs::write(
        ledger_path_in(root, "path-abc"),
        format!("{good}\nnot json at all\n{good}\n"),
    )
    .unwrap();

    assert!(rename_project_in(root, "path-abc", "owner/repo").unwrap());
    assert!(
        !ledger_path_in(root, "path-abc").exists(),
        "the source file is removed"
    );
    let moved = std::fs::read_to_string(ledger_path_in(root, "owner/repo")).unwrap();
    let lines: Vec<&str> = moved.lines().collect();
    assert_eq!(lines.len(), 3, "every line carried over: {moved}");
    assert_eq!(
        lines[1], "not json at all",
        "an unparseable line rides verbatim"
    );
    for line in [lines[0], lines[2]] {
        let v: serde_json::Value = serde_json::from_str(line).unwrap();
        assert_eq!(
            v["project"], "owner/repo",
            "inline project rewritten: {line}"
        );
        assert_eq!(v["tokens"]["input"], 100, "tokens untouched: {line}");
    }
}

#[test]
fn rename_project_appends_when_target_exists() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let mut old = sample_record();
    old.project = "path-abc".into();
    old.issue = 1;
    std::fs::write(
        ledger_path_in(root, "path-abc"),
        format!("{}\n", record_line(&old).unwrap()),
    )
    .unwrap();
    let mut existing = sample_record();
    existing.issue = 2;
    std::fs::write(
        ledger_path_in(root, "owner/repo"),
        format!("{}\n", record_line(&existing).unwrap()),
    )
    .unwrap();

    assert!(rename_project_in(root, "path-abc", "owner/repo").unwrap());
    let merged = std::fs::read_to_string(ledger_path_in(root, "owner/repo")).unwrap();
    let issues: Vec<u64> = merged
        .lines()
        .map(|l| {
            serde_json::from_str::<serde_json::Value>(l).unwrap()["issue"]
                .as_u64()
                .unwrap()
        })
        .collect();
    assert_eq!(issues, vec![2, 1], "the target's own rows stay first");
}

#[test]
fn rename_project_is_a_noop_without_a_source_file() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    assert!(!rename_project_in(root, "path-abc", "owner/repo").unwrap());
    assert!(
        !ledger_path_in(root, "owner/repo").exists(),
        "no target is created for nothing"
    );
    assert!(!rename_project_in(root, "same", "same").unwrap());
}
