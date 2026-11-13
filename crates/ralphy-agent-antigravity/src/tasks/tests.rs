use super::*;

const SOURCE: &str = include_str!("../tasks.rs");

/// The body of the function `name` in [`SOURCE`]: from its `fn` line to the
/// next item.
fn body_of(name: &str) -> &'static str {
    let start = SOURCE
        .find(&format!("fn {name}("))
        .unwrap_or_else(|| panic!("no fn {name} in tasks.rs"));
    let rest = SOURCE.get(start..).expect("a char boundary");
    let end = rest.find("\n}\n").map_or(rest.len(), |at| at + 3);
    rest.get(..end).expect("a char boundary")
}

#[test]
fn every_one_shot_holds_its_prompt_behind_the_proof() {
    for name in [
        "diagnose_repo",
        "draft_issues",
        "triage_issues",
        "consolidate_knowledge",
    ] {
        let body = body_of(name);
        assert_eq!(
            body.matches("held_one_shot(").count(),
            1,
            "{name} must run through the held session helper:\n{body}"
        );
    }
    let helper = body_of("held_one_shot");
    assert!(helper.contains("Session {"), "{helper}");
    assert!(helper.contains("session.run(task)"), "{helper}");
    for spawn in ["Command::new", "run_headless", "_session("] {
        assert!(
            !SOURCE.contains(spawn),
            "tasks.rs starts no child of its own: {spawn}"
        );
    }
}

fn fixture(name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name);
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

#[test]
fn a_one_shot_reads_its_answer_from_the_last_result() {
    let stdout = fixture("two_turns.jsonl");
    assert_eq!(answer(&stdout, "").expect("answer"), "RALPHY_DONE_EXIT\n");
}

#[test]
fn a_failed_task_turn_is_an_error_even_after_a_good_proof_turn() {
    let proof_only = fixture("two_turns.jsonl")
        .lines()
        .take_while(|line| !line.contains("\"step_index\":2"))
        .collect::<Vec<_>>()
        .join("\n");
    let failed = format!(
        "{proof_only}\n{}\n",
        r#"{"event":"result","result":{"status":"ERROR","response":"","error":"boom"}}"#
    );
    let err = answer(&failed, "").expect_err("the task turn failed");
    assert!(format!("{err:#}").contains("boom"), "{err:#}");
    assert!(
        answer("", "").is_err(),
        "a stream with no result is no answer"
    );
}

#[test]
fn a_one_shot_whose_last_result_is_cut_is_an_error() {
    let stdout = fixture("two_turns.jsonl");
    let last = stdout.trim_end().rfind('\n').expect("two lines or more");
    let cut = stdout.get(..last + 60).expect("a char boundary");
    assert!(
        answer(cut, "").is_err(),
        "the proof turn's READY is no answer"
    );
}
