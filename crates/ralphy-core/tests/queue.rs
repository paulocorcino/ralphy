//! Queue-loop lifecycle tests over a throwaway git repo, with a `ScriptedAgent`,
//! a `RecordingTracker`, and a `ScriptedClock` standing in for the real adapter,
//! `gh`, and the wall clock. These prove the acceptance criteria the core owns:
//! ascending/deduped order, close-on-green with a branch-pointing comment and no
//! label mutation, stop-at-first-non-green with later issues untouched, dry-run
//! closing nothing, and the deadline blocking the next issue.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};

use ralphy_core::{
    run_queue, Agent, BranchMode, Execution, Issue, IssueTracker, Outcome, Plan, PlanLimit,
    QueueConfig, RunClock, StopReason, Usage, Verdict, WaitOutcome, Workspace,
};

/// A scripted planning result. `Limit(reset)` makes `plan()` return a
/// `PlanLimit` error (a usage limit before any plan was written); once the
/// queue drains, `plan()` succeeds normally.
enum PlanScript {
    Limit(Option<String>),
}

/// Plans a feasible step for every issue and returns a scripted sequence of
/// execution outcomes (one per `execute` call). It also records, in order, the
/// issue numbers it was asked to plan and to execute — so a test can assert that
/// issues past a stop were never touched. Each executed issue makes one real
/// commit, so the run branch is non-empty (and is handed back, not dropped).
struct ScriptedAgent {
    /// Each entry is `(outcome, commits)`: when `commits` is false the execute
    /// makes no commit, leaving `HEAD` unchanged (used to drive the
    /// progress-aware cap on limit-resumes).
    outcomes: RefCell<VecDeque<(Outcome, bool)>>,
    planned: RefCell<Vec<u64>>,
    executed: RefCell<Vec<u64>>,
    /// Scripted planning results consumed before a `plan` succeeds. Each entry
    /// makes one `plan` call fail with a `PlanLimit`; an empty queue plans normally.
    plan_scripts: RefCell<VecDeque<PlanScript>>,
    /// Every `plan` call (including the failing limit attempts), in order.
    plan_attempts: RefCell<usize>,
    /// Open steps written by `plan` — set to 0 to script an infeasible issue.
    steps: usize,
    /// If set, appended to the plan as a `## Acceptance ledger` section.
    ledger: Option<String>,
    /// If set, appended verbatim to the plan body (extra sections such as
    /// `## Feasible`, `## Handoff`, `## Plan friction`).
    extra: Option<String>,
}

impl ScriptedAgent {
    /// Script a sequence of outcomes where every execute makes a commit.
    fn new(outcomes: Vec<Outcome>) -> Self {
        Self::scripted(outcomes.into_iter().map(|o| (o, true)).collect())
    }

    /// Script a sequence of `(outcome, commits)` pairs, controlling whether each
    /// execute leaves a commit behind.
    fn scripted(outcomes: Vec<(Outcome, bool)>) -> Self {
        Self {
            outcomes: RefCell::new(outcomes.into()),
            planned: RefCell::new(Vec::new()),
            executed: RefCell::new(Vec::new()),
            plan_scripts: RefCell::new(VecDeque::new()),
            plan_attempts: RefCell::new(0),
            steps: 1,
            ledger: None,
            extra: None,
        }
    }

    fn with_ledger(mut self, ledger: impl Into<String>) -> Self {
        self.ledger = Some(ledger.into());
        self
    }

    /// Append extra sections verbatim to every plan this agent writes.
    fn with_plan_extra(mut self, extra: impl Into<String>) -> Self {
        self.extra = Some(extra.into());
        self
    }

    /// Script an infeasible plan (zero open steps).
    fn infeasible(mut self) -> Self {
        self.steps = 0;
        self
    }

    /// Script planning failures: each [`PlanScript`] makes one `plan` call return
    /// a `PlanLimit`; once exhausted, `plan` succeeds.
    fn with_plan_scripts(self, scripts: Vec<PlanScript>) -> Self {
        *self.plan_scripts.borrow_mut() = scripts.into();
        self
    }
}

impl Agent for ScriptedAgent {
    fn name(&self) -> &'static str {
        "scripted"
    }

    fn plan(&self, issue: &Issue, ws: &Workspace) -> anyhow::Result<Plan> {
        *self.plan_attempts.borrow_mut() += 1;
        // A scripted limit fails this plan call before any artifact is written,
        // mirroring a usage limit hit mid-planning.
        if let Some(PlanScript::Limit(reset)) = self.plan_scripts.borrow_mut().pop_front() {
            return Err(PlanLimit { reset }.into());
        }
        self.planned.borrow_mut().push(issue.number);
        fs::create_dir_all(ws.ralphy_dir())?;
        let path = ws.plan_path();
        let ledger_section = self
            .ledger
            .as_deref()
            .map(|l| format!("\n## Acceptance ledger\n\n{l}"))
            .unwrap_or_default();
        let extra_section = self
            .extra
            .as_deref()
            .map(|e| format!("\n{e}\n"))
            .unwrap_or_default();
        let body = format!(
            "# Plan for #{}\n\n## Steps\n{}{}{}",
            issue.number,
            "- [ ] do a thing\n".repeat(self.steps),
            ledger_section,
            extra_section,
        );
        fs::write(&path, body)?;
        Ok(Plan {
            open_steps: self.steps,
            recommended_model: None,
            path,
            usage: Usage::default(),
        })
    }

    fn execute(&self, _plan: &Plan, ws: &Workspace) -> anyhow::Result<Execution> {
        // The most recently planned issue is the one being executed.
        let number = *self.planned.borrow().last().unwrap();
        let n = self.executed.borrow().len();
        self.executed.borrow_mut().push(number);
        let (outcome, commits) = self
            .outcomes
            .borrow_mut()
            .pop_front()
            .unwrap_or((Outcome::Done, true));
        // Make a real commit so the run branch carries work and is handed back —
        // unless this call is scripted to make no progress (HEAD unchanged).
        if commits {
            let repo = ws.repo_root();
            // A unique path per execute so a resumed retry still produces a new
            // commit when scripted to.
            let marker = repo.join(format!("issue-{number}-{n}.txt"));
            fs::write(&marker, "done\n").unwrap();
            git(repo, &["add", "."]);
            git(
                repo,
                &["commit", "-q", "-m", &format!("work #{number} ({n})")],
            );
        }
        Ok(Execution {
            outcome,
            usage: Usage::default(),
        })
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
    /// Every `comment` call (handoff at close, infeasible-skip reasoning).
    comments: RefCell<Vec<(u64, String)>>,
    /// Every `add_label` call (the needs-split label on a bundle verdict).
    labels: RefCell<Vec<(u64, String)>>,
    /// Scripted handoff comments returned by `handoff_comment`, by issue number.
    handoffs: HashMap<u64, String>,
    /// Scripted open children returned by `open_children`, by parent number —
    /// the open issues whose `## Parent` references a retired bundle.
    children: HashMap<u64, Vec<u64>>,
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

    fn comment(&self, number: u64, body: &str) -> anyhow::Result<()> {
        self.comments.borrow_mut().push((number, body.to_string()));
        Ok(())
    }

    fn add_label(&self, number: u64, label: &str) -> anyhow::Result<()> {
        self.labels.borrow_mut().push((number, label.to_string()));
        Ok(())
    }

    fn handoff_comment(&self, number: u64) -> anyhow::Result<Option<String>> {
        Ok(self.handoffs.get(&number).cloned())
    }

    fn open_children(&self, number: u64) -> anyhow::Result<Vec<u64>> {
        Ok(self.children.get(&number).cloned().unwrap_or_default())
    }
}

/// A clock that reports the deadline passed once it has been polled `after`
/// times — letting a test fast-forward the budget deterministically. It also
/// records every `wait_for_reset` call (the reset string) and returns a scripted
/// [`WaitOutcome`] (default [`WaitOutcome::Resumed`]) without sleeping.
struct ScriptedClock {
    polls: RefCell<usize>,
    after: usize,
    /// The reset string passed to each `wait_for_reset`, in call order.
    waited_for: RefCell<Vec<String>>,
    /// What each `wait_for_reset` returns (defaults to `Resumed`).
    wait_result: WaitOutcome,
}

impl ScriptedClock {
    /// Never expires.
    fn never() -> Self {
        Self {
            polls: RefCell::new(0),
            after: usize::MAX,
            waited_for: RefCell::new(Vec::new()),
            wait_result: WaitOutcome::Resumed,
        }
    }

    /// Reports the deadline passed starting from the `after`-th poll (0-based).
    fn passes_after(after: usize) -> Self {
        Self {
            polls: RefCell::new(0),
            after,
            waited_for: RefCell::new(Vec::new()),
            wait_result: WaitOutcome::Resumed,
        }
    }

    /// A never-expiring clock whose `wait_for_reset` reports the deadline passed
    /// (the reset lands beyond the run deadline).
    fn deadline_on_wait() -> Self {
        Self {
            polls: RefCell::new(0),
            after: usize::MAX,
            waited_for: RefCell::new(Vec::new()),
            wait_result: WaitOutcome::DeadlinePassed,
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

    fn wait_for_reset(&self, reset: &str) -> WaitOutcome {
        self.waited_for.borrow_mut().push(reset.to_string());
        self.wait_result
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

/// The tracked file paths at `refname` (recursive), one per entry.
fn git_ls(repo: &Path, refname: &str) -> Vec<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["ls-tree", "-r", "--name-only", refname])
        .output()
        .expect("spawn git");
    String::from_utf8(out.stdout)
        .unwrap()
        .lines()
        .map(str::to_string)
        .collect()
}

/// The contents of `path` as committed at `refname` (empty if absent).
fn git_show(repo: &Path, refname: &str, path: &str) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["show", &format!("{refname}:{path}")])
        .output()
        .expect("spawn git");
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// Point the token-usage ledger at a shared temp dir for the whole test binary,
/// once, so `run_queue`'s best-effort ledger writes never touch the operator's
/// real `~/.ralphy/usage`. Each test's repo has a unique path → a unique
/// `path-<hash>` project slug → its own ledger file, so a shared root still keeps
/// per-test lines isolated by filename. Set once (not per-test) to avoid a
/// data race on the process-global env var under parallel test execution.
fn ensure_usage_dir() -> PathBuf {
    use std::sync::OnceLock;
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        let dir = std::env::temp_dir().join(format!("ralphy-usage-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        std::env::set_var("RALPHY_USAGE_DIR", &dir);
        dir
    })
    .clone()
}

fn init_repo(name: &str) -> PathBuf {
    ensure_usage_dir();
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
        stop_on_limit_plan: false,
        stop_on_limit_exec: false,
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
        stop_on_limit_plan: false,
        stop_on_limit_exec: false,
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
        stop_on_limit_plan: false,
        stop_on_limit_exec: false,
    }
}

/// A New-mode config with the auto-resume opt-out (`--stop-on-limit`) enabled for
/// both phases (an explicit `--stop-on-limit` forces plan and execute alike).
fn cfg_stop_on_limit(repo: &Path, stamp: &str) -> QueueConfig {
    QueueConfig {
        repo_root: repo.to_path_buf(),
        base_branch: "main".into(),
        dry_run: false,
        stamp: stamp.into(),
        branch_mode: BranchMode::New,
        only_issue: None,
        stop_on_limit_plan: true,
        stop_on_limit_exec: true,
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
fn stop_on_limit_opt_out_stops_as_limit() {
    // With `--stop-on-limit`, a usage limit stops and reports the reset (the
    // pre-auto-resume behaviour) instead of waiting.
    let repo = init_repo("limit");
    let queue = vec![issue(10)];
    let agent = ScriptedAgent::new(vec![Outcome::Limit(Some("15:00".into()))]);
    let tracker = RecordingTracker::default();
    let clock = ScriptedClock::never();

    let report = run_queue(
        &cfg_stop_on_limit(&repo, "stamp-limit"),
        &queue,
        &agent,
        &tracker,
        &clock,
    )
    .unwrap();

    match report.stop {
        Some(StopReason::Limit { number, reset }) => {
            assert_eq!(number, 10);
            assert_eq!(reset, Some("15:00".into()));
        }
        other => panic!("expected Limit stop, got {other:?}"),
    }
    // The opt-out never waits.
    assert_eq!(
        *agent.executed.borrow(),
        vec![10],
        "executed once, no resume"
    );
    assert!(
        clock.waited_for.borrow().is_empty(),
        "stop-on-limit never calls wait_for_reset"
    );

    fs::remove_dir_all(&repo).ok();
}

#[test]
fn plan_limit_with_stop_on_limit_stops_and_reports() {
    // A usage limit during *planning* (before any plan is written) under
    // `--stop-on-limit` (always the case for Codex) stops the run and reports the
    // reset — it never waits and never executes.
    let repo = init_repo("plan-limit-stop");
    let queue = vec![issue(10)];
    let agent = ScriptedAgent::new(vec![Outcome::Done])
        .with_plan_scripts(vec![PlanScript::Limit(Some("12:23 AM".into()))]);
    let tracker = RecordingTracker::default();
    let clock = ScriptedClock::never();

    let report = run_queue(
        &cfg_stop_on_limit(&repo, "stamp-plan-limit"),
        &queue,
        &agent,
        &tracker,
        &clock,
    )
    .unwrap();

    match report.stop {
        Some(StopReason::Limit { number, reset }) => {
            assert_eq!(number, 10);
            assert_eq!(reset, Some("12:23 AM".into()));
        }
        other => panic!("expected Limit stop, got {other:?}"),
    }
    assert_eq!(*agent.plan_attempts.borrow(), 1, "planned once, no resume");
    assert!(
        agent.executed.borrow().is_empty(),
        "a plan-time limit never reaches execute"
    );
    assert!(
        clock.waited_for.borrow().is_empty(),
        "stop-on-limit never waits for a plan-time reset"
    );
    // The limit is recorded on the issue result, not swallowed as a hard error.
    let worked = &report.worked;
    assert_eq!(worked.len(), 1);
    assert_eq!(worked[0].number, 10);
    assert_eq!(
        worked[0].outcome,
        Some(Outcome::Limit(Some("12:23 AM".into())))
    );
    assert!(!worked[0].closed);

    fs::remove_dir_all(&repo).ok();
}

#[test]
fn plan_limit_auto_resumes_by_replanning() {
    // With auto-resume (the default), a plan-time limit waits for the reset and
    // re-plans the SAME issue, then proceeds to execute and close it green.
    let repo = init_repo("plan-limit-resume");
    let queue = vec![issue(7)];
    let agent = ScriptedAgent::new(vec![Outcome::Done])
        .with_plan_scripts(vec![PlanScript::Limit(Some("15:00".into()))]);
    let tracker = RecordingTracker::default();
    let clock = ScriptedClock::never();

    let report = run_queue(
        &cfg(&repo, "stamp-plan-resume", false),
        &queue,
        &agent,
        &tracker,
        &clock,
    )
    .unwrap();

    assert!(report.stop.is_none(), "resume then green leaves no stop");
    assert_eq!(
        *agent.plan_attempts.borrow(),
        2,
        "planned twice: limit, then success after the reset"
    );
    assert_eq!(*clock.waited_for.borrow(), vec!["15:00".to_string()]);
    assert_eq!(*agent.executed.borrow(), vec![7], "executed after re-plan");
    let closed: Vec<u64> = tracker.closes.borrow().iter().map(|(n, _)| *n).collect();
    assert_eq!(closed, vec![7], "green issue closed after the resume");

    fs::remove_dir_all(&repo).ok();
}

#[test]
fn repeated_plan_limits_hit_the_cap_and_stop() {
    // A reset that never actually clears (e.g. a past/garbage hint) must not spin
    // the resume loop forever: after MAX_PLAN_LIMIT_RESUMES no-progress waits the
    // runner stops and reports the limit.
    let repo = init_repo("plan-limit-cap");
    let queue = vec![issue(3)];
    let agent = ScriptedAgent::new(vec![Outcome::Done]).with_plan_scripts(vec![
        PlanScript::Limit(Some("09:00".into())),
        PlanScript::Limit(Some("09:00".into())),
        PlanScript::Limit(Some("09:00".into())),
    ]);
    let tracker = RecordingTracker::default();
    let clock = ScriptedClock::never();

    let report = run_queue(
        &cfg(&repo, "stamp-plan-cap", false),
        &queue,
        &agent,
        &tracker,
        &clock,
    )
    .unwrap();

    assert_eq!(
        *agent.plan_attempts.borrow(),
        3,
        "two resumes then the third limit hits the cap"
    );
    assert_eq!(
        *clock.waited_for.borrow(),
        vec!["09:00".to_string(), "09:00".to_string()],
        "waited twice before the cap stopped it"
    );
    assert!(agent.executed.borrow().is_empty(), "never executed");
    match report.stop {
        Some(StopReason::Limit { number, reset }) => {
            assert_eq!(number, 3);
            assert_eq!(reset, Some("09:00".into()));
        }
        other => panic!("expected Limit stop, got {other:?}"),
    }

    fs::remove_dir_all(&repo).ok();
}

#[test]
fn plan_limit_deadline_beats_resume() {
    // A plan-time reset landing past the run deadline stops the run (deadline beats
    // resume) instead of waiting, just like an execute-time limit.
    let repo = init_repo("plan-limit-deadline");
    let queue = vec![issue(4)];
    let agent = ScriptedAgent::new(vec![Outcome::Done])
        .with_plan_scripts(vec![PlanScript::Limit(Some("15:00".into()))]);
    let tracker = RecordingTracker::default();
    let clock = ScriptedClock::deadline_on_wait();

    let report = run_queue(
        &cfg(&repo, "stamp-plan-deadline", false),
        &queue,
        &agent,
        &tracker,
        &clock,
    )
    .unwrap();

    assert!(matches!(report.stop, Some(StopReason::Deadline)));
    assert_eq!(*clock.waited_for.borrow(), vec!["15:00".to_string()]);
    assert_eq!(*agent.plan_attempts.borrow(), 1, "planned once, then cut");
    assert!(agent.executed.borrow().is_empty(), "never executed");

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
fn green_close_posts_handoff_and_friction_comment() {
    let repo = init_repo("handoff-close");
    let extra = "## Handoff\n\n- **Delivered**: lab fixtures (abc1234)\n- **Residue**: Setup-Lab.ps1 never ran clean-slate\n\n## Plan friction\n\n- the plan treated the lab as a given precondition";
    let queue = vec![issue(1)];
    let agent = ScriptedAgent::new(vec![Outcome::Done]).with_plan_extra(extra);
    let tracker = RecordingTracker::default();

    run_queue(
        &cfg(&repo, "stamp-handoff", false),
        &queue,
        &agent,
        &tracker,
        &ScriptedClock::never(),
    )
    .unwrap();

    let comments = tracker.comments.borrow();
    assert_eq!(comments.len(), 1, "one handoff comment for the green issue");
    let (number, body) = &comments[0];
    assert_eq!(*number, 1);
    assert!(body.contains("## Handoff"), "comment carries the handoff");
    assert!(
        body.contains("never ran clean-slate"),
        "residue reaches the issue"
    );
    assert!(
        body.contains("## Plan friction") && body.contains("given precondition"),
        "friction reaches the issue"
    );

    fs::remove_dir_all(&repo).ok();
}

#[test]
fn infeasible_plan_posts_planner_reasoning_comment() {
    let repo = init_repo("infeasible-comment");
    let extra =
        "## Feasible: no\nThe issue bundles six PRD breakdown tasks; split into W1-T01..T06.";
    let queue = vec![issue(3)];
    let agent = ScriptedAgent::new(vec![])
        .infeasible()
        .with_plan_extra(extra);
    let tracker = RecordingTracker::default();

    let report = run_queue(
        &cfg(&repo, "stamp-infeasible", false),
        &queue,
        &agent,
        &tracker,
        &ScriptedClock::never(),
    )
    .unwrap();

    // The skip stays a skip: not closed, not executed, run continues.
    assert!(
        tracker.closes.borrow().is_empty(),
        "infeasible never closes"
    );
    assert!(
        agent.executed.borrow().is_empty(),
        "infeasible never executes"
    );
    assert!(report.stop.is_none(), "infeasible does not stop the run");

    // But the verdict is no longer silent: the reasoning lands on the issue.
    // This reason carries the word "bundle", so it routes to the bundle path:
    // the needs-split label is applied and the comment names the human step.
    let comments = tracker.comments.borrow();
    assert_eq!(comments.len(), 1, "one skip comment");
    let (number, body) = &comments[0];
    assert_eq!(*number, 3);
    assert!(
        body.contains("bundles six PRD breakdown tasks"),
        "planner reasoning reaches the issue: {body}"
    );
    assert!(
        body.contains("/to-issues"),
        "bundle comment names the human split step: {body}"
    );
    let labels = tracker.labels.borrow();
    assert_eq!(
        labels.as_slice(),
        &[(3u64, "needs-split".to_string())],
        "bundle verdict applies the needs-split label"
    );

    fs::remove_dir_all(&repo).ok();
}

#[test]
fn infeasible_plan_without_bundle_verdict_stays_generic() {
    // A reason without the word "bundle" takes the generic infeasible path:
    // no label, and the comment is the respecify-oriented one.
    let repo = init_repo("infeasible-generic");
    let extra = "## Feasible: no\nNo acceptance criteria and no verifiable done condition.";
    let queue = vec![issue(4)];
    let agent = ScriptedAgent::new(vec![])
        .infeasible()
        .with_plan_extra(extra);
    let tracker = RecordingTracker::default();

    let report = run_queue(
        &cfg(&repo, "stamp-generic", false),
        &queue,
        &agent,
        &tracker,
        &ScriptedClock::never(),
    )
    .unwrap();

    assert!(report.stop.is_none(), "infeasible does not stop the run");
    assert!(
        tracker.labels.borrow().is_empty(),
        "no needs-split label without a bundle verdict"
    );
    let comments = tracker.comments.borrow();
    assert_eq!(comments.len(), 1, "one skip comment");
    let (_, body) = &comments[0];
    assert!(
        body.contains("stays open"),
        "generic comment tells the human the issue was not closed: {body}"
    );
    assert!(
        !body.contains("/to-issues"),
        "generic comment does not prescribe a split: {body}"
    );

    fs::remove_dir_all(&repo).ok();
}

#[test]
fn closed_blockers_handoffs_feed_the_planner_and_stale_file_is_removed() {
    // Pass 1: #5 depends on closed #2, which left a handoff comment. The runner
    // must write `.ralphy/handoffs.md` before planning #5.
    {
        let repo = init_repo("handoff-feed");
        let queue = vec![issue_with_body(5, "## Blocked by\n- #2\n")];
        let agent = ScriptedAgent::new(vec![Outcome::Done]);
        let tracker = RecordingTracker {
            closed_issues: HashSet::from([2]),
            handoffs: HashMap::from([(
                2u64,
                "## Handoff\n\n- **Delivered**: lab fixtures\n- **Commands that work**: docker compose up -d".to_string(),
            )]),
            ..Default::default()
        };

        run_queue(
            &cfg(&repo, "stamp-feed", false),
            &queue,
            &agent,
            &tracker,
            &ScriptedClock::never(),
        )
        .unwrap();

        let handoffs_md = fs::read_to_string(repo.join(".ralphy").join("handoffs.md"))
            .expect("handoffs.md written");
        assert!(handoffs_md.contains("## From #2"), "names the source issue");
        assert!(
            handoffs_md.contains("lab fixtures") && handoffs_md.contains("docker compose up -d"),
            "carries the predecessor's handoff content"
        );
        assert!(
            handoffs_md.contains("leads, not truths"),
            "carries the staleness caveat"
        );

        fs::remove_dir_all(&repo).ok();
    }

    // Pass 2: an issue with no blockers must not inherit a stale handoffs.md
    // from a previous issue — the runner removes it.
    {
        let repo = init_repo("handoff-stale");
        let ralphy = repo.join(".ralphy");
        fs::create_dir_all(&ralphy).unwrap();
        fs::write(
            ralphy.join("handoffs.md"),
            "# stale from a previous issue\n",
        )
        .unwrap();

        let queue = vec![issue(7)];
        let agent = ScriptedAgent::new(vec![Outcome::Done]);
        let tracker = RecordingTracker::default();

        run_queue(
            &cfg(&repo, "stamp-stale", false),
            &queue,
            &agent,
            &tracker,
            &ScriptedClock::never(),
        )
        .unwrap();

        assert!(
            !ralphy.join("handoffs.md").exists(),
            "stale handoffs.md removed for an issue with no closed blockers"
        );

        fs::remove_dir_all(&repo).ok();
    }
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
fn closed_blocker_with_open_children_still_blocks() {
    // #4 declares "Blocked by #3"; #3 is closed but was a retired bundle whose
    // work moved into open children #16/#17 (their `## Parent` references #3).
    // Expected: #4 is skipped, blocked on the children — not on the closed #3.
    {
        let repo = init_repo("split-blocks");
        let queue = vec![issue_with_body(4, "## Blocked by\n- #3\n")];
        let agent = ScriptedAgent::new(vec![]);
        let tracker = RecordingTracker {
            closed_issues: HashSet::from([3]),
            children: HashMap::from([(3u64, vec![16, 17])]),
            ..Default::default()
        };

        let report = run_queue(
            &cfg(&repo, "stamp-split", false),
            &queue,
            &agent,
            &tracker,
            &ScriptedClock::never(),
        )
        .unwrap();

        let r4 = report
            .worked
            .iter()
            .find(|r| r.number == 4)
            .expect("#4 in worked");
        assert!(r4.outcome.is_none(), "#4 skipped, never planned");
        assert!(!r4.closed);
        assert_eq!(r4.blocked_by, vec![16, 17], "blocked on the open children");
        assert!(
            agent.planned.borrow().is_empty(),
            "#4 never reached the planner"
        );
        assert!(report.stop.is_none(), "a skip never stops the run");

        fs::remove_dir_all(&repo).ok();
    }

    // Same shape but the children are all closed (none open): the closed
    // blocker counts as done and #4 runs normally.
    {
        let repo = init_repo("split-drained");
        let queue = vec![issue_with_body(4, "## Blocked by\n- #3\n")];
        let agent = ScriptedAgent::new(vec![Outcome::Done]);
        let tracker = RecordingTracker {
            closed_issues: HashSet::from([3]),
            children: HashMap::new(), // no OPEN children remain
            ..Default::default()
        };

        let report = run_queue(
            &cfg(&repo, "stamp-drained", false),
            &queue,
            &agent,
            &tracker,
            &ScriptedClock::never(),
        )
        .unwrap();

        let r4 = report
            .worked
            .iter()
            .find(|r| r.number == 4)
            .expect("#4 in worked");
        assert!(r4.blocked_by.is_empty(), "no open children → unblocked");
        assert!(r4.closed, "#4 closed green");

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

#[test]
fn limit_resume_reexecutes_same_issue_only() {
    // A scripted `Limit(Some(reset))` then `Done` must re-run execute() for the
    // SAME issue (executed [n, n]) without advancing the queue or re-planning,
    // and the issue closes green. The clock records the parsed reset it waited on.
    let repo = init_repo("limit-resume");
    let queue = vec![issue(20)];
    let agent = ScriptedAgent::scripted(vec![
        (Outcome::Limit(Some("15:00".into())), true),
        (Outcome::Done, true),
    ]);
    let tracker = RecordingTracker::default();
    let clock = ScriptedClock::never();

    let report = run_queue(
        &cfg(&repo, "stamp-resume", false),
        &queue,
        &agent,
        &tracker,
        &clock,
    )
    .unwrap();

    // execute() ran twice for #20; plan() ran once (never re-planned).
    assert_eq!(*agent.executed.borrow(), vec![20, 20], "execute-only retry");
    assert_eq!(*agent.planned.borrow(), vec![20], "plan() never re-run");
    // The queue was not advanced and the issue closed green.
    assert!(report.stop.is_none(), "resume → green, no stop");
    let closes: Vec<u64> = tracker.closes.borrow().iter().map(|(n, _)| *n).collect();
    assert_eq!(closes, vec![20], "same issue closed green after resume");
    // wait_for_reset was called once, with the parsed reset string.
    assert_eq!(*clock.waited_for.borrow(), vec!["15:00".to_string()]);

    fs::remove_dir_all(&repo).ok();
}

#[test]
fn two_no_commit_limit_resumes_abandon_the_issue() {
    // Two consecutive limit-resumes that commit nothing abandon the issue with
    // StopReason::Limit; a commit between resumes resets the counter.
    {
        let repo = init_repo("limit-cap");
        let queue = vec![issue(30)];
        // Both resumes make no commit (HEAD unchanged) → cap fires.
        let agent = ScriptedAgent::scripted(vec![
            (Outcome::Limit(Some("09:00".into())), false),
            (Outcome::Limit(Some("09:00".into())), false),
        ]);
        let tracker = RecordingTracker::default();
        let clock = ScriptedClock::never();

        let report = run_queue(
            &cfg(&repo, "stamp-cap", false),
            &queue,
            &agent,
            &tracker,
            &clock,
        )
        .unwrap();

        assert_eq!(
            *agent.executed.borrow(),
            vec![30, 30],
            "two resumes then cap"
        );
        // The cap is reached via the resume path, not by skipping the wait.
        assert_eq!(
            *clock.waited_for.borrow(),
            vec!["09:00".to_string()],
            "the first limit waited for its reset before the cap fired"
        );
        match report.stop {
            Some(StopReason::Limit { number, reset }) => {
                assert_eq!(number, 30);
                assert_eq!(reset, Some("09:00".into()));
            }
            other => panic!("expected Limit stop from the progress cap, got {other:?}"),
        }
        assert!(tracker.closes.borrow().is_empty(), "abandoned, not closed");

        fs::remove_dir_all(&repo).ok();
    }

    // A commit between the two no-commit resumes resets the counter, so the cap
    // does not fire at the second resume; the run continues to green.
    {
        let repo = init_repo("limit-cap-reset");
        let queue = vec![issue(31)];
        let agent = ScriptedAgent::scripted(vec![
            (Outcome::Limit(Some("09:00".into())), false), // streak 1
            (Outcome::Limit(Some("09:00".into())), true),  // commit → streak 0
            (Outcome::Limit(Some("09:00".into())), false), // streak 1
            (Outcome::Done, true),                         // green before cap
        ]);
        let tracker = RecordingTracker::default();
        let clock = ScriptedClock::never();

        let report = run_queue(
            &cfg(&repo, "stamp-cap-reset", false),
            &queue,
            &agent,
            &tracker,
            &clock,
        )
        .unwrap();

        assert_eq!(
            *agent.executed.borrow(),
            vec![31, 31, 31, 31],
            "commit between resumes lets the run continue"
        );
        assert!(report.stop.is_none(), "reached green, no abandon");
        let closes: Vec<u64> = tracker.closes.borrow().iter().map(|(n, _)| *n).collect();
        assert_eq!(closes, vec![31], "issue closed green");

        fs::remove_dir_all(&repo).ok();
    }
}

#[test]
fn deadline_during_wait_short_circuits_with_deadline() {
    // A reset that lands beyond the run deadline (clock reports DeadlinePassed
    // from wait_for_reset) stops the run with StopReason::Deadline, without a
    // further execute.
    let repo = init_repo("limit-deadline");
    let queue = vec![issue(40)];
    let agent = ScriptedAgent::scripted(vec![(Outcome::Limit(Some("15:00".into())), true)]);
    let tracker = RecordingTracker::default();
    let clock = ScriptedClock::deadline_on_wait();

    let report = run_queue(
        &cfg(&repo, "stamp-deadline-wait", false),
        &queue,
        &agent,
        &tracker,
        &clock,
    )
    .unwrap();

    assert_eq!(
        *agent.executed.borrow(),
        vec![40],
        "no resume past the deadline"
    );
    assert_eq!(*clock.waited_for.borrow(), vec!["15:00".to_string()]);
    assert!(
        matches!(report.stop, Some(StopReason::Deadline)),
        "deadline beats resume: {:?}",
        report.stop
    );

    fs::remove_dir_all(&repo).ok();
}

#[test]
fn green_issue_writes_one_plan_and_one_execute_ledger_line() {
    // Working one green issue must append exactly one `"phase":"plan"` line and
    // one `"phase":"execute"` line to the project's ledger, each carrying the
    // agent label, the issue number, and an outcome (ADR-0008 D6). The ledger
    // root is the shared temp dir set by `init_repo`; this repo's unique slug
    // gives it its own file.
    let repo = init_repo("ledger");
    let usage_dir = ensure_usage_dir();
    let queue = vec![issue(77)];
    let agent = ScriptedAgent::new(vec![Outcome::Done]);
    let tracker = RecordingTracker::default();

    run_queue(
        &cfg(&repo, "stamp-ledger", false),
        &queue,
        &agent,
        &tracker,
        &ScriptedClock::never(),
    )
    .unwrap();

    // The ledger file is keyed by the project slug with `/` sanitized to `-`.
    let slug = ralphy_core::git::project_slug(&repo);
    let file = usage_dir.join(format!("{}.jsonl", slug.replace('/', "-")));
    let content = fs::read_to_string(&file)
        .unwrap_or_else(|e| panic!("ledger file {} unreadable: {e}", file.display()));

    let plan_lines: Vec<&str> = content
        .lines()
        .filter(|l| l.contains("\"phase\":\"plan\""))
        .collect();
    let exec_lines: Vec<&str> = content
        .lines()
        .filter(|l| l.contains("\"phase\":\"execute\""))
        .collect();
    assert_eq!(plan_lines.len(), 1, "exactly one plan line: {content}");
    assert_eq!(exec_lines.len(), 1, "exactly one execute line: {content}");

    // Each line carries agent, issue, and outcome.
    for line in [plan_lines[0], exec_lines[0]] {
        assert!(line.contains("\"agent\":\"scripted\""), "agent: {line}");
        assert!(line.contains("\"issue\":77"), "issue: {line}");
        assert!(line.contains("\"outcome\":"), "outcome: {line}");
    }
    // The execute line records the terminal `done`; the plan line records `ok`.
    assert!(
        exec_lines[0].contains("\"outcome\":\"done\""),
        "execute outcome: {}",
        exec_lines[0]
    );
    assert!(
        plan_lines[0].contains("\"outcome\":\"ok\""),
        "plan outcome: {}",
        plan_lines[0]
    );

    fs::remove_dir_all(&repo).ok();
}
