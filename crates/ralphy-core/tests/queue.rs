//! Queue-loop lifecycle tests over a throwaway git repo, with a `ScriptedAgent`,
//! a `RecordingTracker`, and a `ScriptedClock` standing in for the real adapter,
//! `gh`, and the wall clock. These prove the acceptance criteria the core owns:
//! ascending/deduped order, close-on-green with a branch-pointing comment and no
//! label mutation, stop-at-first-non-green with later issues untouched, dry-run
//! closing nothing, and the deadline blocking the next issue.

use std::cell::RefCell;
use std::collections::{HashSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};

use ralphy_core::{
    run_queue, Agent, BranchMode, Issue, IssueTracker, Outcome, Plan, QueueConfig, RunClock,
    StopReason, Verdict, Workspace,
};

/// Plans a feasible step for every issue and returns a scripted sequence of
/// execution outcomes (one per `execute` call). It also records, in order, the
/// issue numbers it was asked to plan and to execute — so a test can assert that
/// issues past a stop were never touched. Each executed issue makes one real
/// commit, so the run branch is non-empty (and is handed back, not dropped).
struct ScriptedAgent {
    outcomes: RefCell<VecDeque<Outcome>>,
    planned: RefCell<Vec<u64>>,
    executed: RefCell<Vec<u64>>,
    /// Open steps written by `plan` — set to 0 to script an infeasible issue.
    steps: usize,
    /// If set, appended to the plan as a `## Acceptance ledger` section.
    ledger: Option<String>,
}

impl ScriptedAgent {
    fn new(outcomes: Vec<Outcome>) -> Self {
        Self {
            outcomes: RefCell::new(outcomes.into()),
            planned: RefCell::new(Vec::new()),
            executed: RefCell::new(Vec::new()),
            steps: 1,
            ledger: None,
        }
    }

    fn with_ledger(mut self, ledger: impl Into<String>) -> Self {
        self.ledger = Some(ledger.into());
        self
    }
}

impl Agent for ScriptedAgent {
    fn plan(&self, issue: &Issue, ws: &Workspace) -> anyhow::Result<Plan> {
        self.planned.borrow_mut().push(issue.number);
        fs::create_dir_all(ws.ralphy_dir())?;
        let path = ws.plan_path();
        let ledger_section = self
            .ledger
            .as_deref()
            .map(|l| format!("\n## Acceptance ledger\n\n{l}"))
            .unwrap_or_default();
        let body = format!(
            "# Plan for #{}\n\n## Steps\n{}{}",
            issue.number,
            "- [ ] do a thing\n".repeat(self.steps),
            ledger_section,
        );
        fs::write(&path, body)?;
        Ok(Plan {
            open_steps: self.steps,
            recommended_model: None,
            path,
        })
    }

    fn execute(&self, _plan: &Plan, ws: &Workspace) -> anyhow::Result<Outcome> {
        // The most recently planned issue is the one being executed.
        let number = *self.planned.borrow().last().unwrap();
        self.executed.borrow_mut().push(number);
        // Make a real commit so the run branch carries work and is handed back.
        let repo = ws.repo_root();
        let marker = repo.join(format!("issue-{number}.txt"));
        fs::write(&marker, "done\n").unwrap();
        git(repo, &["add", "."]);
        git(repo, &["commit", "-q", "-m", &format!("work #{number}")]);
        Ok(self
            .outcomes
            .borrow_mut()
            .pop_front()
            .unwrap_or(Outcome::Done))
    }
}

/// Records every `(number, comment)` close and every `write_evidence` call so
/// tests can assert exactly which issues were closed and what evidence was
/// written. Never mutates labels — there is no label API here, which is the point.
///
/// `closed_issues` is a set of issue numbers to report as closed when `is_closed`
/// is called — numbers absent from the set are reported as open.
#[derive(Default)]
struct RecordingTracker {
    closes: RefCell<Vec<(u64, String)>>,
    evidence_writes: RefCell<Vec<(u64, Vec<Verdict>)>>,
    closed_issues: HashSet<u64>,
}

impl IssueTracker for RecordingTracker {
    fn close(&self, number: u64, comment: &str) -> anyhow::Result<()> {
        self.closes.borrow_mut().push((number, comment.to_string()));
        Ok(())
    }

    fn write_evidence(&self, number: u64, _body: &str, verdicts: &[Verdict]) -> anyhow::Result<()> {
        self.evidence_writes
            .borrow_mut()
            .push((number, verdicts.to_vec()));
        Ok(())
    }

    fn is_closed(&self, number: u64) -> anyhow::Result<bool> {
        Ok(self.closed_issues.contains(&number))
    }
}

/// A clock that reports the deadline passed once it has been polled `after`
/// times — letting a test fast-forward the budget deterministically.
struct ScriptedClock {
    polls: RefCell<usize>,
    after: usize,
}

impl ScriptedClock {
    /// Never expires.
    fn never() -> Self {
        Self {
            polls: RefCell::new(0),
            after: usize::MAX,
        }
    }

    /// Reports the deadline passed starting from the `after`-th poll (0-based).
    fn passes_after(after: usize) -> Self {
        Self {
            polls: RefCell::new(0),
            after,
        }
    }
}

impl RunClock for ScriptedClock {
    fn deadline_passed(&self) -> bool {
        let mut p = self.polls.borrow_mut();
        let passed = *p >= self.after;
        *p += 1;
        passed
    }
}

fn git(repo: &Path, args: &[&str]) {
    let status = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .status()
        .expect("spawn git");
    assert!(status.success(), "git {args:?} failed");
}

fn current_branch(repo: &Path) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .output()
        .expect("spawn git");
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

fn branch_exists(repo: &Path, branch: &str) -> bool {
    Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["rev-parse", "--verify", "--quiet", branch])
        .status()
        .expect("spawn git")
        .success()
}

fn init_repo(name: &str) -> PathBuf {
    static N: AtomicU32 = AtomicU32::new(0);
    let dir = std::env::temp_dir().join(format!(
        "ralphy-queue-{}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed),
        name
    ));
    fs::create_dir_all(&dir).unwrap();
    git(&dir, &["init", "-q", "-b", "main"]);
    git(&dir, &["config", "user.email", "t@example.com"]);
    git(&dir, &["config", "user.name", "Test"]);
    fs::write(dir.join(".gitignore"), ".ralphy/\n").unwrap();
    fs::write(dir.join("README.md"), "hello\n").unwrap();
    git(&dir, &["add", "."]);
    git(&dir, &["commit", "-q", "-m", "init"]);
    dir
}

fn issue(number: u64) -> Issue {
    Issue {
        number,
        title: format!("issue {number}"),
        body: String::new(),
        labels: vec![],
    }
}

fn issue_labeled(number: u64, labels: &[&str]) -> Issue {
    Issue {
        number,
        title: format!("issue {number}"),
        body: String::new(),
        labels: labels.iter().map(|s| s.to_string()).collect(),
    }
}

fn issue_with_body(number: u64, body: impl Into<String>) -> Issue {
    Issue {
        number,
        title: format!("issue {number}"),
        body: body.into(),
        labels: vec![],
    }
}

fn cfg(repo: &Path, stamp: &str, dry_run: bool) -> QueueConfig {
    QueueConfig {
        repo_root: repo.to_path_buf(),
        base_branch: "main".into(),
        dry_run,
        stamp: stamp.into(),
        branch_mode: BranchMode::New,
        only_issue: None,
    }
}

fn cfg_only(repo: &Path, stamp: &str, only: u64) -> QueueConfig {
    QueueConfig {
        repo_root: repo.to_path_buf(),
        base_branch: "main".into(),
        dry_run: false,
        stamp: stamp.into(),
        branch_mode: BranchMode::New,
        only_issue: Some(only),
    }
}

fn cfg_current(repo: &Path, stamp: &str) -> QueueConfig {
    QueueConfig {
        repo_root: repo.to_path_buf(),
        base_branch: "main".into(),
        dry_run: false,
        stamp: stamp.into(),
        branch_mode: BranchMode::Current,
        only_issue: None,
    }
}

#[test]
fn works_issues_in_order_and_closes_each_green() {
    let repo = init_repo("green");
    let queue = vec![issue(2), issue(5), issue(9)];
    let agent = ScriptedAgent::new(vec![Outcome::Done, Outcome::Done, Outcome::Done]);
    let tracker = RecordingTracker::default();

    let report = run_queue(
        &cfg(&repo, "stamp-green", false),
        &queue,
        &agent,
        &tracker,
        &ScriptedClock::never(),
    )
    .unwrap();

    // Worked ascending, all three executed.
    assert_eq!(*agent.executed.borrow(), vec![2, 5, 9]);
    assert!(report.stop.is_none(), "no stop on an all-green run");

    // Each green issue closed exactly once, comment names the run branch, and no
    // label mutation is even possible (the tracker has no label method).
    let closes = tracker.closes.borrow();
    let numbers: Vec<u64> = closes.iter().map(|(n, _)| *n).collect();
    assert_eq!(numbers, vec![2, 5, 9], "every green issue closed once");
    for (_, comment) in closes.iter() {
        assert!(
            comment.contains(&report.branch),
            "comment must point at the run branch: {comment}"
        );
    }

    // Clean New-mode run: the repo is returned to the original branch and the run
    // branch is kept (not deleted) for the human to review and merge by hand.
    assert_eq!(current_branch(&repo), "main", "returned to original branch");
    assert!(
        branch_exists(&repo, &report.branch),
        "run branch kept after a clean run"
    );

    fs::remove_dir_all(&repo).ok();
}

#[test]
fn first_non_green_stops_run_and_leaves_later_issues_untouched() {
    let repo = init_repo("stop");
    let queue = vec![issue(1), issue(2), issue(3)];
    // #1 green, #2 blocked → #3 must never be touched.
    let agent = ScriptedAgent::new(vec![Outcome::Done, Outcome::Blocked("nope".into())]);
    let tracker = RecordingTracker::default();

    let report = run_queue(
        &cfg(&repo, "stamp-stop", false),
        &queue,
        &agent,
        &tracker,
        &ScriptedClock::never(),
    )
    .unwrap();

    assert_eq!(*agent.executed.borrow(), vec![1, 2], "#3 never executed");
    assert_eq!(*agent.planned.borrow(), vec![1, 2], "#3 never planned");

    // Earlier green issue stays closed; the blocked one is not closed.
    let numbers: Vec<u64> = tracker.closes.borrow().iter().map(|(n, _)| *n).collect();
    assert_eq!(numbers, vec![1], "only the green issue closed");

    match report.stop {
        Some(StopReason::NonGreen { number, outcome }) => {
            assert_eq!(number, 2);
            assert_eq!(outcome, Outcome::Blocked("nope".into()));
        }
        other => panic!("expected NonGreen stop, got {other:?}"),
    }

    // Branch handed back with the green commit on it.
    assert_eq!(current_branch(&repo), report.branch);

    fs::remove_dir_all(&repo).ok();
}

#[test]
fn dry_run_closes_nothing_and_restores() {
    let repo = init_repo("dry");
    let queue = vec![issue(1), issue(2)];
    let agent = ScriptedAgent::new(vec![Outcome::Done, Outcome::Done]);
    let tracker = RecordingTracker::default();

    let report = run_queue(
        &cfg(&repo, "stamp-dry", true),
        &queue,
        &agent,
        &tracker,
        &ScriptedClock::never(),
    )
    .unwrap();

    assert!(
        tracker.closes.borrow().is_empty(),
        "dry run must never close an issue"
    );
    assert!(
        agent.executed.borrow().is_empty(),
        "dry run plans only — never executes"
    );
    // Empty branch dropped, repo restored.
    assert_eq!(current_branch(&repo), "main", "restored to original branch");
    assert!(
        !branch_exists(&repo, &report.branch),
        "empty branch dropped"
    );

    fs::remove_dir_all(&repo).ok();
}

#[test]
fn deadline_blocks_starting_the_next_issue() {
    let repo = init_repo("deadline");
    let queue = vec![issue(1), issue(2)];
    let agent = ScriptedAgent::new(vec![Outcome::Done, Outcome::Done]);
    let tracker = RecordingTracker::default();
    // Clock: first poll (before #1) OK, second poll (before #2) reports passed.
    let clock = ScriptedClock::passes_after(1);

    let report = run_queue(
        &cfg(&repo, "stamp-deadline", false),
        &queue,
        &agent,
        &tracker,
        &clock,
    )
    .unwrap();

    assert_eq!(*agent.planned.borrow(), vec![1], "#2 never planned");
    assert_eq!(*agent.executed.borrow(), vec![1], "#2 never executed");
    assert!(matches!(report.stop, Some(StopReason::Deadline)));

    fs::remove_dir_all(&repo).ok();
}

#[test]
fn stop_before_halts_before_labeled_issue() {
    let repo = init_repo("stop-before");
    // #1 is a normal issue; #2 carries the stop-before label; #3 must never be touched.
    let queue = vec![issue(1), issue_labeled(2, &["stop-before"]), issue(3)];
    let agent = ScriptedAgent::new(vec![Outcome::Done]);
    let tracker = RecordingTracker::default();

    let report = run_queue(
        &cfg(&repo, "stamp-stopbefore", false),
        &queue,
        &agent,
        &tracker,
        &ScriptedClock::never(),
    )
    .unwrap();

    // #1 executed; #2 (labeled) and #3 never planned/executed.
    assert_eq!(
        *agent.executed.borrow(),
        vec![1],
        "#2 and #3 never executed"
    );
    assert_eq!(*agent.planned.borrow(), vec![1], "#2 and #3 never planned");

    match report.stop {
        Some(StopReason::StopBefore { number }) => assert_eq!(number, 2),
        other => panic!("expected StopBefore, got {other:?}"),
    }

    // Branch has work (from #1), so it is handed back on the run branch.
    assert_eq!(current_branch(&repo), report.branch);

    fs::remove_dir_all(&repo).ok();
}

#[test]
fn only_issue_ignores_stop_before() {
    let repo = init_repo("only-stop-before");
    // The queue is just the labeled issue; only_issue overrides the stop-before guard.
    let queue = vec![issue_labeled(7, &["stop-before"])];
    let agent = ScriptedAgent::new(vec![Outcome::Done]);
    let tracker = RecordingTracker::default();

    let report = run_queue(
        &cfg_only(&repo, "stamp-only", 7),
        &queue,
        &agent,
        &tracker,
        &ScriptedClock::never(),
    )
    .unwrap();

    // The issue was executed despite the label.
    assert_eq!(*agent.executed.borrow(), vec![7]);
    assert!(
        report.stop.is_none(),
        "no stop when only_issue overrides stop-before"
    );

    fs::remove_dir_all(&repo).ok();
}

#[test]
fn limit_outcome_stops_as_limit() {
    let repo = init_repo("limit");
    let queue = vec![issue(10)];
    let agent = ScriptedAgent::new(vec![Outcome::Limit(Some("15:00".into()))]);
    let tracker = RecordingTracker::default();

    let report = run_queue(
        &cfg(&repo, "stamp-limit", false),
        &queue,
        &agent,
        &tracker,
        &ScriptedClock::never(),
    )
    .unwrap();

    match report.stop {
        Some(StopReason::Limit { number, reset }) => {
            assert_eq!(number, 10);
            assert_eq!(reset, Some("15:00".into()));
        }
        other => panic!("expected Limit stop, got {other:?}"),
    }

    fs::remove_dir_all(&repo).ok();
}

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
fn report_carries_commit_count_and_oneline() {
    let repo = init_repo("report");
    // Three green issues → three commits over the base.
    let queue = vec![issue(1), issue(2), issue(3)];
    let agent = ScriptedAgent::new(vec![Outcome::Done, Outcome::Done, Outcome::Done]);
    let tracker = RecordingTracker::default();

    let report = run_queue(
        &cfg(&repo, "stamp-report", false),
        &queue,
        &agent,
        &tracker,
        &ScriptedClock::never(),
    )
    .unwrap();

    assert_eq!(report.commits, 3, "one commit per green issue");
    assert_eq!(
        report.oneline.len(),
        report.commits,
        "one oneline entry per counted commit"
    );

    fs::remove_dir_all(&repo).ok();
}

#[test]
fn green_close_calls_write_evidence_with_parsed_verdicts() {
    let repo = init_repo("evidence-write");
    // The ledger contains one verified criterion: the runner must call
    // write_evidence with it after the green close.
    let ledger = "- [verified] Some AC — evidence: unit test proves it\n";
    let queue = vec![issue(1)];
    let agent = ScriptedAgent::new(vec![Outcome::Done]).with_ledger(ledger);
    let tracker = RecordingTracker::default();

    run_queue(
        &cfg(&repo, "stamp-evidence", false),
        &queue,
        &agent,
        &tracker,
        &ScriptedClock::never(),
    )
    .unwrap();

    let writes = tracker.evidence_writes.borrow();
    assert_eq!(
        writes.len(),
        1,
        "write_evidence called once for the green issue"
    );
    let (number, verdicts) = &writes[0];
    assert_eq!(*number, 1);
    assert_eq!(verdicts.len(), 1);
    assert_eq!(verdicts[0].criterion, "Some AC");
    assert_eq!(verdicts[0].kind, ralphy_core::VerdictKind::Verified);
    assert_eq!(verdicts[0].evidence, "unit test proves it");

    fs::remove_dir_all(&repo).ok();
}

#[test]
fn green_close_with_no_ledger_skips_write_evidence() {
    let repo = init_repo("evidence-noop");
    // No ledger section in the plan — write_evidence must not be called.
    let queue = vec![issue(2)];
    let agent = ScriptedAgent::new(vec![Outcome::Done]); // no ledger
    let tracker = RecordingTracker::default();

    run_queue(
        &cfg(&repo, "stamp-noop", false),
        &queue,
        &agent,
        &tracker,
        &ScriptedClock::never(),
    )
    .unwrap();

    // Issue was closed (green) but write_evidence was never called.
    assert_eq!(tracker.closes.borrow().len(), 1, "issue still closed");
    assert!(
        tracker.evidence_writes.borrow().is_empty(),
        "no evidence-write when ledger is absent"
    );

    fs::remove_dir_all(&repo).ok();
}

#[test]
fn open_blocker_skips_then_closed_blocker_runs() {
    // Pass 1: #5 declares "## Blocked by\n- #2" and #2 is NOT in the closed set.
    // Expected: #5 skipped (blocked_by == [2]), not closed, no stop; #2 runs normally.
    {
        let repo = init_repo("blocker-open");
        let blocked_body = "## Blocked by\n- #2\n";
        let queue = vec![issue_with_body(5, blocked_body), issue(2)];
        let agent = ScriptedAgent::new(vec![Outcome::Done]); // only #2 executes
                                                             // #2 is NOT in closed_issues → is_closed(2) returns false → #5 is skipped.
        let tracker = RecordingTracker::default();

        let report = run_queue(
            &cfg(&repo, "stamp-blocker-open", false),
            &queue,
            &agent,
            &tracker,
            &ScriptedClock::never(),
        )
        .unwrap();

        // #5 must have been skipped with blocked_by == [2].
        let r5 = report
            .worked
            .iter()
            .find(|r| r.number == 5)
            .expect("#5 in worked");
        assert!(r5.outcome.is_none(), "#5 outcome must be None (skipped)");
        assert!(!r5.closed, "#5 must not be closed");
        assert_eq!(r5.blocked_by, vec![2], "#5 blocked_by must be [2]");

        // #2 must have been planned, executed, and closed.
        assert!(
            agent.planned.borrow().contains(&2),
            "#2 must have been planned"
        );
        assert!(
            agent.executed.borrow().contains(&2),
            "#2 must have been executed"
        );
        let closes: Vec<u64> = tracker.closes.borrow().iter().map(|(n, _)| *n).collect();
        assert_eq!(closes, vec![2], "only #2 closed");

        assert!(report.stop.is_none(), "no stop — later issues continue");

        fs::remove_dir_all(&repo).ok();
    }

    // Pass 2: same queue but #2 IS in the closed set.
    // Expected: #5 is no longer blocked and runs normally (planned, executed, closed).
    {
        let repo = init_repo("blocker-closed");
        let blocked_body = "## Blocked by\n- #2\n";
        let queue = vec![issue_with_body(5, blocked_body), issue(2)];
        let agent = ScriptedAgent::new(vec![Outcome::Done, Outcome::Done]);
        let tracker = RecordingTracker {
            closed_issues: HashSet::from([2]),
            ..Default::default()
        };

        let report = run_queue(
            &cfg(&repo, "stamp-blocker-closed", false),
            &queue,
            &agent,
            &tracker,
            &ScriptedClock::never(),
        )
        .unwrap();

        // #5 must have run (not blocked).
        let r5 = report
            .worked
            .iter()
            .find(|r| r.number == 5)
            .expect("#5 in worked");
        assert!(
            r5.blocked_by.is_empty(),
            "#5 must not be blocked when #2 is closed"
        );
        assert!(r5.closed, "#5 must be closed green");

        assert!(report.stop.is_none());

        fs::remove_dir_all(&repo).ok();
    }
}

#[test]
fn limit_outcome_with_no_reset_carries_none() {
    let repo = init_repo("limit-noreset");
    let queue = vec![issue(11)];
    let agent = ScriptedAgent::new(vec![Outcome::Limit(None)]);
    let tracker = RecordingTracker::default();

    let report = run_queue(
        &cfg(&repo, "stamp-limit-none", false),
        &queue,
        &agent,
        &tracker,
        &ScriptedClock::never(),
    )
    .unwrap();

    match report.stop {
        Some(StopReason::Limit { number, reset }) => {
            assert_eq!(number, 11);
            assert_eq!(reset, None);
        }
        other => panic!("expected Limit stop with None reset, got {other:?}"),
    }

    fs::remove_dir_all(&repo).ok();
}
