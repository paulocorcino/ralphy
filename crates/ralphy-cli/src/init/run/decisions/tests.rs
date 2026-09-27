use super::*;

/// Every `[Y/n]`/`[y/N]` prompt of `ralphy init` maps its answers: trimmed,
/// case-folded, and with the default the prompt shows. Default-Yes prompts
/// (draft, create repo, private visibility, labels) accept an empty answer;
/// default-No prompts (publish, download, smoke test) decline it — a bulk
/// external write or a download is never the silent default. The repo stays
/// private unless the answer is an explicit no.
#[test]
fn every_prompt_decision_maps_its_answers() {
    use crate::init::{issues, skills, verify};
    type Decision = fn(&str) -> bool;
    // (prompt, decision, answers that proceed, answers that decline)
    let rows: [(&str, Decision, &[&str], &[&str]); 7] = [
        (
            "draft",
            issues::draft_decision,
            &["", "y", "  YES "],
            &["n", "no", "nah"],
        ),
        (
            "publish",
            issues::publish_decision,
            &["y", "yes", "  YES "],
            &["", "n", "maybe"],
        ),
        (
            "create repo",
            create_repo_decision,
            &["", "y", "YES"],
            &["n", "no", "huh"],
        ),
        (
            "private visibility",
            private_visibility_decision,
            &["", "y", "anything"],
            &["n", "NO"],
        ),
        (
            "labels",
            labels_decision,
            &["", "y", "Y", "yes", "  YES  "],
            &["n", "no", "maybe"],
        ),
        (
            "download",
            skills::download_decision,
            &["yes", "y", "  Y  ", "YES"],
            &["", "n", "no", "maybe"],
        ),
        (
            "smoke test",
            verify::smoke_test_decision,
            &["y", "yes"],
            &["", "n"],
        ),
    ];
    for (prompt, decide, proceed, decline) in rows {
        for answer in proceed {
            assert!(decide(answer), "{prompt}: {answer:?} proceeds");
        }
        for answer in decline {
            assert!(!decide(answer), "{prompt}: {answer:?} declines");
        }
    }
}

#[test]
fn repo_name_from_path_uses_final_segment() {
    assert_eq!(
        repo_name_from_path(Path::new("/home/dev/subtitle-downloader")),
        "subtitle-downloader"
    );
    // A root with no usable base name falls back to `repo`.
    assert_eq!(repo_name_from_path(Path::new("/")), "repo");
}

#[test]
fn commit_decision_maps_clean_dirty_yes_and_refusal() {
    assert_eq!(
        commit_decision(true, "anything"),
        CommitDecision::NothingToCommit
    );
    assert_eq!(commit_decision(false, "yes"), CommitDecision::Commit);
    assert_eq!(commit_decision(false, "y"), CommitDecision::Commit);
    // Empty input accepts the `[Y/n]` default and commits the snapshot.
    assert_eq!(commit_decision(false, ""), CommitDecision::Commit);
    match commit_decision(false, "no") {
        CommitDecision::Abort(msg) => assert!(!msg.is_empty()),
        other => panic!("expected Abort, got {other:?}"),
    }
}

#[test]
fn branch_decision_maps_default_and_decline() {
    assert_eq!(
        branch_decision("main", ""),
        BranchDecision::Create("ralphy/init".into())
    );
    assert_eq!(
        branch_decision("main", "yes"),
        BranchDecision::Create("ralphy/init".into())
    );
    assert_eq!(branch_decision("main", "no"), BranchDecision::Stay);
    assert_eq!(branch_decision("main", "n"), BranchDecision::Stay);
}
#[test]
fn select_agent_defaults_to_first_logged_in() {
    let logged_in = vec![Agent::Codex, Agent::Opencode];
    assert_eq!(select_agent(None, &logged_in).unwrap(), Agent::Codex);
}

#[test]
fn select_agent_honours_explicit_logged_in_choice() {
    let logged_in = vec![Agent::Claude, Agent::Codex];
    assert_eq!(
        select_agent(Some(Agent::Codex), &logged_in).unwrap(),
        Agent::Codex
    );
}

#[test]
fn select_agent_rejects_explicit_not_logged_in() {
    // A present-but-not-logged-in (or absent) agent is a hard error naming the
    // logged-in set, never a silent fallback to another agent.
    let logged_in = vec![Agent::Claude];
    let err = select_agent(Some(Agent::Opencode), &logged_in).unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("opencode"), "names the rejected agent:\n{msg}");
    assert!(msg.contains("claude"), "names the logged-in set:\n{msg}");
}

#[test]
fn init_model_pins_sonnet_for_claude_only() {
    assert_eq!(init_model_for(Agent::Claude), Some("sonnet"));
    assert_eq!(init_model_for(Agent::Codex), None);
    assert_eq!(init_model_for(Agent::Kimi), None);
    assert_eq!(init_model_for(Agent::Opencode), None);
}
