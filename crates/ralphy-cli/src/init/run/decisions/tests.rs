use super::*;

#[test]
fn create_repo_decision_defaults_to_yes() {
    // Empty (Enter on a [Y/n] prompt) and explicit yes proceed; anything else
    // declines, keeping the original "not a git repository" error.
    assert!(create_repo_decision(""));
    assert!(create_repo_decision("y"));
    assert!(create_repo_decision("YES"));
    assert!(!create_repo_decision("n"));
    assert!(!create_repo_decision("no"));
    assert!(!create_repo_decision("huh"));
}

#[test]
fn private_visibility_defaults_to_private() {
    // The default and yes mean private; only an explicit no makes it public.
    assert!(private_visibility_decision(""));
    assert!(private_visibility_decision("y"));
    assert!(private_visibility_decision("anything"));
    assert!(!private_visibility_decision("n"));
    assert!(!private_visibility_decision("NO"));
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
fn labels_decision_empty_and_yes_proceed_no_declines() {
    assert!(labels_decision(""));
    assert!(labels_decision("y"));
    assert!(labels_decision("Y"));
    assert!(labels_decision("yes"));
    assert!(labels_decision("  YES  "));
    assert!(!labels_decision("n"));
    assert!(!labels_decision("no"));
    assert!(!labels_decision("maybe"));
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
