//! Branch mode, the undo tag, the preflight aborts and `.gitignore` hygiene.

use super::*;

#[test]
fn current_mode_commits_on_current_branch() {
    let repo = init_repo("current");
    let queue = vec![issue(1)];
    let agent = ScriptedAgent::new(vec![Outcome::Done]);
    let tracker = RecordingTracker::default();

    let report = run_queue(
        &cfg_current(&repo, "stamp-current"),
        &queue,
        &agent,
        &tracker,
        &ScriptedClock::never(),
    )
    .unwrap();

    // No fresh run branch: commits land on the branch the repo was already on.
    assert_eq!(report.branch, "main", "current mode commits onto main");
    assert!(
        !branch_exists(&repo, "afk/run-stamp-current"),
        "current mode creates no run branch"
    );
    // The green commit landed, counted over the pre-run HEAD compare ref.
    assert!(report.commits > 0, "current-mode work counted");
    // The repo is left on the same branch — nothing is checked out or deleted.
    assert_eq!(current_branch(&repo), "main");

    fs::remove_dir_all(&repo).ok();
}

#[test]
fn undo_tag_marks_pre_run_head_in_current_mode() {
    let repo = init_repo("undo-current");
    let pre_run = rev_parse(&repo, "HEAD").expect("pre-run HEAD resolves");
    let queue = vec![issue(1)];
    let agent = ScriptedAgent::new(vec![Outcome::Done]);
    let tracker = RecordingTracker::default();

    let report = run_queue(
        &cfg_current(&repo, "stamp-undo-current"),
        &queue,
        &agent,
        &tracker,
        &ScriptedClock::never(),
    )
    .unwrap();

    // The report hands back the marker, and the local tag points at the commit
    // the branch stood on before the run — `git reset --hard <tag>` is the undo.
    let tag = report.undo_tag.as_deref().expect("undo tag reported");
    assert_eq!(tag, "ralphy/pre-run-stamp-undo-current");
    assert_eq!(
        rev_parse(&repo, tag).as_deref(),
        Some(pre_run.as_str()),
        "tag marks the pre-run HEAD"
    );
    // The work itself is untouched — commits still on the live branch.
    assert!(report.commits > 0);

    fs::remove_dir_all(&repo).ok();
}

#[test]
fn undo_tag_marks_the_base_in_new_mode() {
    let repo = init_repo("undo-new");
    let base = rev_parse(&repo, "main").expect("base resolves");
    let queue = vec![issue(1)];
    let agent = ScriptedAgent::new(vec![Outcome::Done]);
    let tracker = RecordingTracker::default();

    let report = run_queue(
        &cfg(&repo, "stamp-undo-new", false),
        &queue,
        &agent,
        &tracker,
        &ScriptedClock::never(),
    )
    .unwrap();

    // In New mode the marker is the cut point of the run branch (the base).
    let tag = report.undo_tag.as_deref().expect("undo tag reported");
    assert_eq!(tag, "ralphy/pre-run-stamp-undo-new");
    assert_eq!(
        rev_parse(&repo, tag).as_deref(),
        Some(base.as_str()),
        "tag marks the base the run branch was cut from"
    );

    fs::remove_dir_all(&repo).ok();
}

#[test]
fn undo_tag_dropped_when_the_run_adds_no_commits() {
    let repo = init_repo("undo-dry");
    let queue = vec![issue(1)];
    let agent = ScriptedAgent::new(vec![Outcome::Done]);
    let tracker = RecordingTracker::default();

    let report = run_queue(
        &cfg(&repo, "stamp-undo-dry", true),
        &queue,
        &agent,
        &tracker,
        &ScriptedClock::never(),
    )
    .unwrap();

    // Nothing to undo: the report carries no marker and the tag is gone from
    // the repo (mirrors the empty-branch delete).
    assert!(report.undo_tag.is_none(), "no undo tag on an empty run");
    assert!(
        rev_parse(&repo, "ralphy/pre-run-stamp-undo-dry").is_none(),
        "empty run's tag deleted"
    );

    fs::remove_dir_all(&repo).ok();
}

#[test]
fn current_mode_refuses_detached_head() {
    let repo = init_repo("detached");
    git(&repo, &["checkout", "--detach", "--quiet"]);
    let queue = vec![issue(1)];
    let agent = ScriptedAgent::new(vec![Outcome::Done]);
    let tracker = RecordingTracker::default();

    let result = run_queue(
        &cfg_current(&repo, "stamp-detached"),
        &queue,
        &agent,
        &tracker,
        &ScriptedClock::never(),
    );

    assert!(result.is_err(), "detached HEAD must abort the run");
    assert!(
        agent.planned.borrow().is_empty(),
        "nothing planned on abort"
    );

    fs::remove_dir_all(&repo).ok();
}

#[test]
fn dirty_tree_aborts_before_branch_work() {
    let repo = init_repo("dirty");
    // A tracked, uncommitted change (not under .ralphy/) makes the tree dirty.
    fs::write(repo.join("README.md"), "changed\n").unwrap();
    let queue = vec![issue(1)];
    let agent = ScriptedAgent::new(vec![Outcome::Done]);
    let tracker = RecordingTracker::default();

    let result = run_queue(
        &cfg(&repo, "stamp-dirty", false),
        &queue,
        &agent,
        &tracker,
        &ScriptedClock::never(),
    );

    assert!(
        result.is_err(),
        "dirty tree must abort before any branch work"
    );
    assert!(
        !branch_exists(&repo, "afk/run-stamp-dirty"),
        "no run branch created on a dirty-tree abort"
    );
    assert!(
        agent.planned.borrow().is_empty(),
        "nothing planned on abort"
    );

    fs::remove_dir_all(&repo).ok();
}

#[test]
fn aborts_when_base_is_missing() {
    let repo = init_repo("nobase");
    let queue = vec![issue(1)];
    let agent = ScriptedAgent::new(vec![Outcome::Done]);
    let tracker = RecordingTracker::default();

    let mut config = cfg(&repo, "stamp-nobase", false);
    config.base_branch = "origin/does-not-exist".into();

    let err = run_queue(&config, &queue, &agent, &tracker, &ScriptedClock::never()).unwrap_err();

    assert!(err.to_string().contains("not found"), "got: {err}");
    assert_eq!(current_branch(&repo), "main", "left where it started");
    assert!(
        !branch_exists(&repo, "afk/run-stamp-nobase"),
        "no run branch created on a missing-base abort"
    );
    assert!(
        agent.planned.borrow().is_empty(),
        "nothing planned on abort"
    );

    fs::remove_dir_all(&repo).ok();
}

#[test]
fn gitignore_gets_ralphy_on_first_run() {
    let repo = init_repo("gitignore");
    // Reset .gitignore to one that does NOT mention .ralphy/, and commit it so the
    // tree starts clean.
    fs::write(repo.join(".gitignore"), "target/\n").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-q", "-m", "gitignore without ralphy"]);

    let queue = vec![issue(1)];
    let agent = ScriptedAgent::new(vec![Outcome::Done]);
    let tracker = RecordingTracker::default();

    // Current mode keeps commits in place, so the ensured `.gitignore` edit stays
    // in the working tree — a New-mode run would commit it onto the run branch and
    // then return to the original branch, hiding it from this observation point.
    run_queue(
        &cfg_current(&repo, "stamp-gitignore"),
        &queue,
        &agent,
        &tracker,
        &ScriptedClock::never(),
    )
    .unwrap();

    let body = fs::read_to_string(repo.join(".gitignore")).unwrap();
    assert!(
        body.contains(".ralphy/"),
        ".ralphy/ added to .gitignore on first run: {body}"
    );

    fs::remove_dir_all(&repo).ok();
}

#[test]
fn new_mode_does_not_leak_ralphy_when_base_lacks_ignore() {
    // Regression: the run branch is cut from a base whose `.gitignore` does NOT
    // ignore `.ralphy/`, while the original branch already does (from a prior run).
    // `ensure_ralphy_ignored` must run on the run branch's working tree, not the
    // original's — otherwise it no-ops and the agent's `git add` sweeps scratch
    // (`.ralphy/plan.md`) into the deliverable.
    let repo = init_repo("noleak");
    // `init_repo` committed a `.gitignore` that ignores `.ralphy/` on `main` (orig).
    // Cut a `base` branch whose `.gitignore` does NOT mention `.ralphy/`.
    git(&repo, &["checkout", "-q", "-b", "base"]);
    fs::write(repo.join(".gitignore"), "target/\n").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-q", "-m", "base without ralphy ignore"]);
    git(&repo, &["checkout", "-q", "main"]);

    let queue = vec![issue(1)];
    let agent = ScriptedAgent::new(vec![Outcome::Done]);
    let tracker = RecordingTracker::default();

    let mut cfg = cfg(&repo, "stamp-noleak", false);
    cfg.base_branch = "base".into();
    run_queue(&cfg, &queue, &agent, &tracker, &ScriptedClock::never()).unwrap();

    // The run branch must carry the work but none of the `.ralphy/` scratch.
    let branch = "afk/run-stamp-noleak";
    let tracked = git_ls(&repo, branch);
    assert!(
        !tracked.iter().any(|f| f.starts_with(".ralphy/")),
        "run branch must not track .ralphy/ scratch, got: {tracked:?}"
    );
    // And the ensure landed on the run branch: its `.gitignore` now ignores `.ralphy/`.
    let gi = git_show(&repo, branch, ".gitignore");
    assert!(
        gi.contains(".ralphy/"),
        ".ralphy/ must be ignored on the run branch: {gi:?}"
    );

    fs::remove_dir_all(&repo).ok();
}
