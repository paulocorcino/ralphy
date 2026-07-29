use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

fn jsonl_snapshot(dir: &Path) -> BTreeMap<String, Vec<u8>> {
    std::fs::read_dir(dir)
        .unwrap()
        .flatten()
        .filter(|entry| entry.path().extension().and_then(|ext| ext.to_str()) == Some("jsonl"))
        .map(|entry| {
            (
                entry.file_name().to_string_lossy().to_string(),
                std::fs::read(entry.path()).unwrap(),
            )
        })
        .collect()
}

fn run_recovery(usage: &Path, stores: &Path) -> std::process::Output {
    let missing = |name: &str| stores.join(name);
    Command::new(env!("CARGO_BIN_EXE_ralphy"))
        .args(["usage", "--recover-models"])
        .env("RALPHY_USAGE_DIR", usage)
        .env("RALPHY_CLAUDE_PROJECTS_DIR", stores.join("claude"))
        .env("RALPHY_CODEX_DIR", missing("codex"))
        .env("RALPHY_COPILOT_DB", missing("copilot.db"))
        .env("RALPHY_CURSOR_DIR", missing("cursor"))
        .env("RALPHY_GEMINI_DIR", missing("gemini"))
        .env("RALPHY_KIMI_DIR", missing("kimi"))
        .env("RALPHY_KIMI_CODE_DIR", missing("kimi-code"))
        .env("RALPHY_OPENCODE_DB", missing("opencode.db"))
        .output()
        .unwrap()
}

#[test]
fn recovery_reports_totals_persists_map_and_never_rewrites_jsonl() {
    let temp = tempfile::tempdir().unwrap();
    let usage = temp.path().join("usage");
    let stores = temp.path().join("stores");
    let claude = stores.join("claude").join("workspace");
    std::fs::create_dir_all(&usage).unwrap();
    std::fs::create_dir_all(&claude).unwrap();
    std::fs::write(
        usage.join("owner-repo.jsonl"),
        concat!(
            "{\"project\":\"owner/repo\",\"agent\":\"claude\",\"model\":\"unknown\",\"session_id\":\"sess-resolved\",\"tokens\":{\"input\":10,\"output\":20,\"cache_read\":30,\"cache_creation\":0},\"ts\":\"2026-07-29T00:00:00Z\"}\n",
            "{\"project\":\"owner/repo\",\"agent\":\"claude\",\"model\":\"unknown\",\"session_id\":\"sess-still-recoverable\",\"tokens\":{\"input\":4,\"output\":5,\"cache_read\":0,\"cache_creation\":0},\"ts\":\"2026-07-29T00:00:00Z\"}\n",
            "{\"project\":\"owner/repo\",\"agent\":\"claude\",\"model\":\"unknown\",\"tokens\":{\"input\":5,\"output\":10,\"cache_read\":0,\"cache_creation\":0},\"ts\":\"2026-07-29T00:00:01Z\"}\n",
        ),
    )
    .unwrap();
    std::fs::write(
        claude.join("sess-resolved.jsonl"),
        "{\"timestamp\":\"2026-07-29T00:00:00Z\",\"message\":{\"id\":\"m1\",\"model\":\"claude-opus-4-8\",\"usage\":{\"input_tokens\":60,\"output_tokens\":0,\"cache_read_input_tokens\":0,\"cache_creation_input_tokens\":0}}}\n",
    )
    .unwrap();
    let before = jsonl_snapshot(&usage);

    let first = run_recovery(&usage, &stores);

    assert!(
        first.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&first.stderr)
    );
    let stdout = String::from_utf8(first.stdout).unwrap();
    assert!(stdout.contains("recovered: 1 line(s), 60 tok"), "{stdout}");
    assert!(stdout.contains("recoverable: 1 line(s), 9 tok"), "{stdout}");
    assert!(stdout.contains("lost: 1 line(s), 15 tok"), "{stdout}");
    assert_eq!(jsonl_snapshot(&usage), before);
    let map_path = usage.join("session-models.json");
    let map: BTreeMap<String, String> =
        serde_json::from_slice(&std::fs::read(&map_path).unwrap()).unwrap();
    assert_eq!(
        map,
        BTreeMap::from([("sess-resolved".into(), "claude-opus-4-8".into())])
    );
    let first_map_bytes = std::fs::read(&map_path).unwrap();
    use std::io::Write;
    writeln!(
        std::fs::OpenOptions::new()
            .append(true)
            .open(usage.join("owner-repo.jsonl"))
            .unwrap(),
        "{{\"project\":\"owner/repo\",\"agent\":\"claude\",\"model\":\"unknown\",\"session_id\":\"sess-new\",\"tokens\":{{\"input\":7,\"output\":5,\"cache_read\":0,\"cache_creation\":0}},\"ts\":\"2026-07-29T00:00:02Z\"}}"
    )
    .unwrap();
    std::fs::write(
        claude.join("sess-new.jsonl"),
        "{\"timestamp\":\"2026-07-29T00:00:02Z\",\"message\":{\"id\":\"m2\",\"model\":\"claude-sonnet-5\",\"usage\":{\"input_tokens\":12,\"output_tokens\":0,\"cache_read_input_tokens\":0,\"cache_creation_input_tokens\":0}}}\n",
    )
    .unwrap();
    let before_second = jsonl_snapshot(&usage);

    let second = run_recovery(&usage, &stores);

    assert!(second.status.success());
    let second_stdout = String::from_utf8(second.stdout).unwrap();
    assert!(
        second_stdout.contains("recovered: 1 line(s), 12 tok"),
        "{second_stdout}"
    );
    let second_map_bytes = std::fs::read(&map_path).unwrap();
    assert_ne!(second_map_bytes, first_map_bytes);
    let second_map: BTreeMap<String, String> = serde_json::from_slice(&second_map_bytes).unwrap();
    assert_eq!(
        second_map,
        BTreeMap::from([
            ("sess-new".into(), "claude-sonnet-5".into()),
            ("sess-resolved".into(), "claude-opus-4-8".into()),
        ])
    );
    assert_eq!(jsonl_snapshot(&usage), before_second);
}

#[test]
fn recovery_reports_colliding_session_models_as_conflicts() {
    let temp = tempfile::tempdir().unwrap();
    let usage = temp.path().join("usage");
    let stores = temp.path().join("stores");
    std::fs::create_dir_all(&usage).unwrap();
    let claude = stores.join("claude/workspace");
    let codex = stores.join("codex/sessions");
    std::fs::create_dir_all(&claude).unwrap();
    std::fs::create_dir_all(&codex).unwrap();
    std::fs::write(
        usage.join("owner-repo.jsonl"),
        concat!(
            "{\"agent\":\"claude\",\"model\":\"unknown\",\"session_id\":\"shared\",\"tokens\":{\"input\":3},\"ts\":\"1\"}\n",
            "{\"agent\":\"codex\",\"model\":\"unknown\",\"session_id\":\"shared\",\"tokens\":{\"input\":7},\"ts\":\"2\"}\n",
        ),
    )
    .unwrap();
    std::fs::write(
        claude.join("shared.jsonl"),
        r#"{"timestamp":"2026-07-29T00:00:00Z","message":{"id":"m1","model":"claude-opus-4-8","usage":{"input_tokens":3}}}"#,
    )
    .unwrap();
    std::fs::write(
        codex.join("shared.jsonl"),
        concat!(
            "{\"type\":\"session_meta\",\"payload\":{\"id\":\"codex-meta\"}}\n",
            "{\"type\":\"turn_context\",\"payload\":{\"model\":\"gpt-5-codex\"}}\n",
            "{\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"info\":{\"total_token_usage\":{\"input_tokens\":7,\"output_tokens\":0,\"cached_input_tokens\":0}}}}\n",
        ),
    )
    .unwrap();

    let output = run_recovery(&usage, &stores);

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("recovered: 1 line(s), 3 tok"), "{stdout}");
    assert!(stdout.contains("conflicts: 1 line(s), 7 tok"), "{stdout}");
    let map: BTreeMap<String, String> =
        serde_json::from_slice(&std::fs::read(usage.join("session-models.json")).unwrap()).unwrap();
    assert_eq!(
        map,
        BTreeMap::from([("shared".into(), "claude-opus-4-8".into())])
    );
}
