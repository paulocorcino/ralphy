use super::*;

/// The rows both copies of `cache_creation_tokens` are held to: this one and
/// the usage scan's, which reads the same file.
#[test]
fn cache_creation_tokens_matches_the_shared_fixture() {
    let rows: Vec<serde_json::Value> = serde_json::from_str(include_str!(
        "../../tests/fixtures/cache_creation_tokens.json"
    ))
    .expect("the fixture is JSON");
    assert!(rows.len() >= 5, "the fixture lost rows");
    for row in &rows {
        assert_eq!(
            cache_creation_tokens(&row["usage"]),
            row["tokens"].as_u64().expect("each row has tokens"),
            "{}",
            row["case"]
        );
    }
}

#[test]
fn parse_plan_usage_skips_warning_preamble() {
    // The headless `-p --output-format stream-json` stdout is preceded by a
    // non-JSON warning line; the parser must skip it and read the terminal
    // result event's usage (reconciled exactly against the payload, D5).
    let stdout = "no stdin data received in 3s. Continuing without stdin.\n\
{\"type\":\"system\",\"subtype\":\"init\"}\n\
{\"type\":\"assistant\",\"message\":{\"id\":\"m1\"}}\n\
{\"type\":\"result\",\"modelUsage\":{\"claude-opus-4-8\":{\"input_tokens\":3747}},\
\"usage\":{\"input_tokens\":3747,\"output_tokens\":9,\"cache_read_input_tokens\":0,\
\"cache_creation_input_tokens\":23406}}\n";
    let usage = parse_plan_usage(stdout);
    assert_eq!(usage.input, 3747);
    assert_eq!(usage.output, 9);
    assert_eq!(usage.cache_read, 0);
    assert_eq!(usage.cache_creation, 23406);
    assert_eq!(usage.model.as_deref(), Some("claude-opus-4-8"));
}

#[test]
fn parse_plan_usage_attributes_to_dominant_not_alphabetical_model() {
    // The real shape (captured from a plan.log): Claude bills a tiny amount to
    // the background `claude-haiku-4-5-20251001` and the bulk to the main
    // `claude-opus-4-8`. The top-level `usage` is the MAIN model's split, so the
    // phase must be labeled opus — not haiku (which sorts first alphabetically
    // and is a dated id absent from the price table).
    let stdout = "{\"type\":\"result\",\
\"modelUsage\":{\
\"claude-haiku-4-5-20251001\":{\"inputTokens\":4375,\"outputTokens\":17,\
\"cacheReadInputTokens\":0,\"cacheCreationInputTokens\":0},\
\"claude-opus-4-8\":{\"inputTokens\":4237,\"outputTokens\":14023,\"cacheReadInputTokens\":1129426,\
\"cacheCreationInputTokens\":76510}},\
\"usage\":{\"input_tokens\":4237,\"output_tokens\":14023,\"cache_read_input_tokens\":1129426,\
\"cache_creation_input_tokens\":76510}}\n";
    let usage = parse_plan_usage(stdout);
    assert_eq!(
        usage.model.as_deref(),
        Some("claude-opus-4-8"),
        "the dominant model, not the alphabetically-first background haiku"
    );
    // The numeric split is the main model's (the top-level `usage`), unchanged.
    assert_eq!(usage.input, 4237);
    assert_eq!(usage.output, 14023);
    assert_eq!(usage.cache_read, 1129426);
    assert_eq!(usage.cache_creation, 76510);
}

#[test]
fn dashed_cwd_encodes_nonalnum_and_preserves_case() {
    assert_eq!(dashed_cwd("c:\\Dev\\ralphy"), "c--Dev-ralphy");
    assert_eq!(
        dashed_cwd("C:\\Dev\\.ralph-worktrees\\issue-10"),
        "C--Dev--ralph-worktrees-issue-10"
    );
}

#[test]
fn parse_transcript_usage_dedups_message_id_and_ignores_iterations() {
    // Three assistant lines: two share `m1` (counted once), one carries `m2`
    // and nests an `iterations[]` that repeats its usage (must be ignored).
    let jsonl = "\
{\"type\":\"assistant\",\"message\":{\"id\":\"m1\",\"usage\":{\"input_tokens\":100,\
\"output_tokens\":10,\"cache_read_input_tokens\":1000,\"cache_creation_input_tokens\":5}}}
{\"type\":\"assistant\",\"message\":{\"id\":\"m1\",\"usage\":{\"input_tokens\":999,\
\"output_tokens\":999,\"cache_read_input_tokens\":999,\"cache_creation_input_tokens\":999}}}
{\"type\":\"assistant\",\"message\":{\"id\":\"m2\",\"usage\":{\"input_tokens\":200,\
\"output_tokens\":20,\"cache_read_input_tokens\":2000,\"cache_creation_input_tokens\":7}},\
\"iterations\":[{\"usage\":{\"input_tokens\":200,\"output_tokens\":20,\
\"cache_read_input_tokens\":2000,\"cache_creation_input_tokens\":7}}]}
";
    let usage = parse_transcript_usage(jsonl);
    // m1 (first only) + m2: input 100+200, output 10+20, cache_read 1000+2000,
    // cache_creation 5+7. The duplicate m1 line and the nested iterations are
    // both excluded.
    assert_eq!(
        usage,
        Usage {
            input: 300,
            output: 30,
            cache_read: 3000,
            cache_creation: 12,
            model: None,
        }
    );
}

#[test]
fn a_line_without_usage_does_not_use_up_its_message_id() {
    let jsonl = "\
{\"type\":\"assistant\",\"message\":{\"id\":\"m1\"}}
{\"type\":\"assistant\",\"message\":{\"id\":\"m1\",\"usage\":{\"input_tokens\":5}}}
";
    assert_eq!(parse_transcript_usage(jsonl).input, 5);
}

#[test]
fn parse_transcript_usage_attributes_dominant_model() {
    // Two models in one transcript: a little haiku auxiliary work and the
    // bulk on opus. The summed `usage.model` must resolve to the *dominant*
    // (most-tokens) model so the price table prices the run — without this
    // every execute row was written `unknown` and went unpriced (`~$?`).
    let jsonl = "\
{\"type\":\"assistant\",\"message\":{\"id\":\"h1\",\"model\":\"claude-haiku-4-5\",\
\"usage\":{\"input_tokens\":10,\"output_tokens\":1,\"cache_read_input_tokens\":5,\
\"cache_creation_input_tokens\":0}}}
{\"type\":\"assistant\",\"message\":{\"id\":\"o1\",\"model\":\"claude-opus-4-8\",\
\"usage\":{\"input_tokens\":200,\"output_tokens\":20,\"cache_read_input_tokens\":2000,\
\"cache_creation_input_tokens\":7}}}
";
    let usage = parse_transcript_usage(jsonl);
    // Tokens still sum across both models...
    assert_eq!(usage.input, 210);
    assert_eq!(usage.cache_read, 2005);
    // ...but the model attribution picks opus (the dominant spend), not haiku.
    assert_eq!(usage.model.as_deref(), Some("claude-opus-4-8"));
}

/// A dashed-cwd transcript dir holding two sessions: `run` (this run's, with
/// a subagent tree) and `other` (a concurrent pre-existing session that also
/// spawned one). Returns the temp dir and the two top-level transcript paths.
fn transcript_tree() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let tmp = tempfile::tempdir().expect("tempdir for a transcript tree");
    let dir = tmp.path();
    let mk = |sid: &str, subs: &[&str]| -> PathBuf {
        let top = dir.join(format!("{sid}.jsonl"));
        fs::write(&top, "{}\n").expect("write a top-level transcript");
        let nest = dir.join(sid).join("subagents");
        fs::create_dir_all(&nest).expect("create a subagents dir");
        for name in subs {
            fs::write(nest.join(format!("{name}.jsonl")), "{}\n")
                .expect("write a subagent transcript");
            // The sidecar metadata Claude files next to each transcript must
            // not be mistaken for one — only `*.jsonl` counts.
            fs::write(nest.join(format!("{name}.meta.json")), "{}")
                .expect("write a subagent meta sidecar");
        }
        top
    };
    let run = mk("run", &["sub-a", "sub-b"]);
    let other = mk("other", &["sub-c"]);
    (tmp, run, other)
}

#[test]
fn subagent_transcripts_collects_the_tree_of_an_appeared_session() {
    // The blind spot this closes: Claude Code files a Task's transcript under
    // `<sid>/subagents/`, which the flat snapshot of the dashed-cwd dir never
    // lists, so every self-review went uncounted.
    let (_tmp, run, _other) = transcript_tree();
    let found = subagent_transcripts(&[run]);
    let names: Vec<_> = found
        .iter()
        .filter_map(|p| p.file_name().and_then(|n| n.to_str()))
        .collect();
    assert_eq!(names, vec!["sub-a.jsonl", "sub-b.jsonl"]);
}

#[test]
fn subagent_transcripts_excludes_a_concurrent_sessions_tree() {
    // Load-bearing scoping (ADR-0008 D10): a concurrent pre-existing session's
    // top-level file merely *grew* and is already excluded from `appeared`, but
    // its subagent files were freshly created inside our window. Deriving from
    // `appeared` keeps appeared-over-grew intact where a blind recursion into
    // the transcript dir would have charged us for someone else's session.
    let (_tmp, run, _other) = transcript_tree();
    let found = subagent_transcripts(&[run]);
    assert!(
        !found.iter().any(|p| p.to_string_lossy().contains("sub-c")),
        "the concurrent session's subagents must not be attributed to this run; got {found:?}"
    );
}

#[test]
fn subagent_transcripts_is_empty_when_a_session_spawned_none() {
    // The common case must stay silent, not error: no `subagents/` dir at all.
    let tmp = tempfile::tempdir().expect("tempdir for a lone transcript");
    let top = tmp.path().join("lonely.jsonl");
    fs::write(&top, "{}\n").expect("write a top-level transcript");
    assert_eq!(subagent_transcripts(&[top]), Vec::<PathBuf>::new());
}

#[test]
fn fold_exec_usage_carries_heaviest_transcript_model() {
    // Two transcripts; the second is heavier. Tokens sum across both, and the
    // attribution follows the heaviest (opus) — not lost to `unknown`.
    let a = Usage {
        input: 10,
        output: 1,
        cache_read: 5,
        cache_creation: 0,
        model: Some("claude-haiku-4-5".into()),
    };
    let b = Usage {
        input: 200,
        output: 20,
        cache_read: 2000,
        cache_creation: 7,
        model: Some("claude-opus-4-8".into()),
    };
    let usage = fold_exec_usage(&[a, b], &[], "sonnet");
    assert_eq!(usage.input, 210);
    assert_eq!(usage.cache_read, 2005);
    assert_eq!(usage.model.as_deref(), Some("claude-opus-4-8"));
}

#[test]
fn fold_exec_usage_sums_subagents_without_relabelling_the_phase() {
    // The regression this exists for: a session that spawns Tasks reported
    // only its own transcript, so a real 43-delivery run under-counted by 22%.
    // The subagent's tokens must land in the phase total — and its model must
    // NOT: a sonnet reviewer under an opus execute leaves the phase on opus,
    // even when the subagent is the heavier of the two records.
    let parent = Usage {
        input: 100,
        output: 10,
        cache_read: 1000,
        cache_creation: 5,
        model: Some("claude-opus-5".into()),
    };
    let subagent = Usage {
        input: 900,
        output: 90,
        cache_read: 9000,
        cache_creation: 45,
        model: Some("claude-sonnet-5".into()),
    };
    let usage = fold_exec_usage(&[parent], &[subagent], "opus");
    assert_eq!(usage.input, 1000);
    assert_eq!(usage.output, 100);
    assert_eq!(usage.cache_read, 10_000);
    assert_eq!(usage.cache_creation, 50);
    assert_eq!(usage.model.as_deref(), Some("claude-opus-5"));
}

#[test]
fn fold_exec_usage_falls_back_to_requested_model_when_none_attributed() {
    // No transcript carried a model (counts present, attribution absent): the
    // phase falls back to the model we requested rather than `unknown`.
    let a = Usage {
        input: 100,
        output: 10,
        cache_read: 0,
        cache_creation: 0,
        model: None,
    };
    let usage = fold_exec_usage(&[a], &[], "sonnet");
    assert_eq!(usage.input, 100);
    assert_eq!(usage.model.as_deref(), Some("sonnet"));
}

#[test]
fn parse_plan_session_id_reads_result_event() {
    let stdout = "no stdin data received in 3s.\n\
{\"type\":\"system\",\"subtype\":\"init\"}\n\
{\"type\":\"result\",\"session_id\":\"sess-abc\",\"usage\":{}}\n";
    assert_eq!(parse_plan_session_id(stdout).as_deref(), Some("sess-abc"));
    // No result event → None.
    assert_eq!(parse_plan_session_id("{\"type\":\"system\"}\n"), None);
}

#[test]
fn session_id_from_files_takes_first_stem() {
    assert_eq!(
        session_id_from_files(&[PathBuf::from("/x/c6ab25d8-de.jsonl")]).as_deref(),
        Some("c6ab25d8-de")
    );
    assert_eq!(session_id_from_files(&[]), None);
}

#[test]
fn parse_transcript_usage_sums_cache_creation_subtiers() {
    // When only the `cache_creation` 5m/1h breakdown is present (no flat
    // field), the sub-tiers are summed.
    let jsonl = "{\"message\":{\"id\":\"x\",\"usage\":{\"input_tokens\":1,\
\"cache_creation\":{\"ephemeral_5m_input_tokens\":40,\"ephemeral_1h_input_tokens\":2}}}}\n";
    let usage = parse_transcript_usage(jsonl);
    assert_eq!(usage.input, 1);
    assert_eq!(usage.cache_creation, 42);
}

#[test]
fn parse_transcript_usage_sums_messages_that_carry_no_id() {
    // Without an id there is nothing to dedup on, so every such line counts.
    let jsonl = concat!(
        r#"{"message":{"usage":{"input_tokens":3,"output_tokens":1}}}"#,
        "\n",
        r#"{"message":{"usage":{"input_tokens":4,"output_tokens":2}}}"#,
        "\n"
    );
    let usage = parse_transcript_usage(jsonl);
    assert_eq!(usage.input, 7);
    assert_eq!(usage.output, 3);
}

/// Write `name` under `dir` and stamp its modified time `age` seconds ago.
fn aged_file(dir: &Path, name: &str, age: u64) -> PathBuf {
    let path = dir.join(name);
    fs::write(&path, "{}\n").expect("write a transcript file");
    let file = fs::File::options()
        .write(true)
        .open(&path)
        .expect("reopen the file to stamp its time");
    file.set_modified(SystemTime::now() - Duration::from_secs(age))
        .expect("stamp the modified time");
    path
}

#[test]
fn newest_jsonl_picks_the_newest_transcript_and_ignores_a_json_sidecar() {
    let tmp = tempfile::tempdir().expect("tempdir for transcripts");
    aged_file(tmp.path(), "old.jsonl", 60);
    let newest = aged_file(tmp.path(), "new.jsonl", 20);
    aged_file(tmp.path(), "new.meta.json", 1);
    assert_eq!(newest_jsonl_since(tmp.path(), None), Some(newest));
}

#[cfg(unix)]
#[test]
fn newest_jsonl_does_not_follow_a_symlinked_folder() {
    let tmp = tempfile::tempdir().expect("tempdir for transcripts");
    let dir = tmp.path().join("projects");
    fs::create_dir(&dir).expect("create the transcript folder");
    aged_file(&dir, "old.jsonl", 60);
    let newest = aged_file(&dir, "new.jsonl", 20);
    // Two links back to the parent: a walk that follows them visits
    // exponentially many paths.
    for name in ["loop-a", "loop-b"] {
        std::os::unix::fs::symlink(&dir, dir.join(name)).expect("link back to the parent");
    }

    let (tx, rx) = std::sync::mpsc::channel();
    let walked = dir.clone();
    std::thread::spawn(move || {
        if tx.send(newest_jsonl_since(&walked, None)).is_err() {
            // The test already gave up waiting; nobody reads the result.
        }
    });

    let found = rx
        .recv_timeout(Duration::from_secs(10))
        .expect("the walk did not end");
    assert_eq!(found, Some(newest));
}

#[test]
fn newest_jsonl_since_is_none_when_every_transcript_is_older_than_the_floor() {
    let tmp = tempfile::tempdir().expect("tempdir for transcripts");
    aged_file(tmp.path(), "a.jsonl", 60);
    aged_file(tmp.path(), "b.jsonl", 30);
    let floor = SystemTime::now() - Duration::from_secs(10);
    assert_eq!(newest_jsonl_since(tmp.path(), Some(floor)), None);
}
