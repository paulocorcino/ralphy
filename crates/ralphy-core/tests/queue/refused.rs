//! A session the agent refused because its hooks did not prove they run: the run
//! ends with that error and the issue and the repo stay as they were.

use super::*;

fn assert_untouched(refusal: Refusal, name: &str) {
    let repo = init_repo(name);
    let queue = vec![issue(10)];
    let agent = ScriptedAgent::new(vec![Outcome::Done]).refusing(refusal);
    let tracker = RecordingTracker::default();

    let err = run_queue(
        &cfg(&repo, &format!("stamp-{name}"), false),
        &queue,
        &agent,
        &tracker,
        &ScriptedClock::never(),
    )
    .expect_err("a refused session ends the run with an error");

    let refused = err
        .downcast_ref::<ralphy_core::HooksUnproven>()
        .expect("the error is HooksUnproven");
    assert_eq!(refused.cause, "no SessionStart line");
    assert!(tracker.labels.borrow().is_empty(), "no label added");
    assert!(
        tracker.removed_labels.borrow().is_empty(),
        "no label removed"
    );
    assert!(tracker.comments.borrow().is_empty(), "no new comment");
    assert!(tracker.closes.borrow().is_empty(), "nothing closed");
    assert_eq!(current_branch(&repo), "main", "back on the original branch");
    assert!(
        !branch_exists(&repo, &format!("afk/run-stamp-{name}")),
        "the empty run branch is gone"
    );

    remove_scratch(&repo);
}

#[test]
fn a_refused_plan_leaves_the_issue_and_the_repo_as_they_were() {
    assert_untouched(Refusal::Plan("no SessionStart line".into()), "refused-plan");
}

#[test]
fn a_refused_execute_leaves_the_issue_and_the_repo_as_they_were() {
    assert_untouched(
        Refusal::Execute("no SessionStart line".into()),
        "refused-exec",
    );
}
