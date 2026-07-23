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
use std::time::Duration;

use ralphy_core::{
    resolve_queue_view, run_queue, Agent, BranchMode, Execution, Issue, IssueTracker, Outcome,
    Plan, PlanLimit, QueueConfig, QueueStatus, RunClock, StopReason, Usage, Verdict, WaitOutcome,
    Workspace,
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
    /// Per-issue plan extras that override `extra` for the matching issue number.
    /// Lets one queue mix, say, a failing and a passing `## Verify` gate.
    extra_by_issue: Vec<(u64, String)>,
    /// When true, `plan` writes a protocol-dirty plan (unticked steps, no
    /// `## Handoff` / `## Plan friction`) so the ADR-0015 lint fires. The
    /// default is a lint-clean plan, keeping the lint transparent to tests
    /// that are not about it.
    lint_dirty: bool,
    /// When true, an `execute` that finds `.ralphy/protocol-failure.md` (the
    /// ADR-0015 bounce brief) repairs the plan: ticks every step and appends
    /// the missing closing sections — a well-behaved executor.
    fix_protocol: bool,
    /// Per-attempt `Usage` to hand back from `execute`, one per call, in
    /// order; once exhausted, `Usage::default()` (no model). Lets a test
    /// script the resume loop's model-folding behavior.
    exec_usages: RefCell<VecDeque<Usage>>,
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
            extra_by_issue: Vec::new(),
            lint_dirty: false,
            fix_protocol: false,
            exec_usages: RefCell::new(VecDeque::new()),
        }
    }

    /// Write protocol-dirty plans (unticked steps, no closing sections) so the
    /// ADR-0015 lint fails on the first DONE.
    fn lint_dirty(mut self) -> Self {
        self.lint_dirty = true;
        self
    }

    /// Repair the plan when a bounce brief (`protocol-failure.md`) is present.
    fn with_protocol_fix(mut self) -> Self {
        self.fix_protocol = true;
        self
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

    /// Append an extra section to ONLY the plan for `number`, overriding
    /// `with_plan_extra` for that issue. Lets a single queue carry both a failing
    /// and a passing verify gate.
    fn with_plan_extra_for(mut self, number: u64, extra: impl Into<String>) -> Self {
        self.extra_by_issue.push((number, extra.into()));
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

    /// Script the `Usage` (including `model`) each `execute` call hands back,
    /// one per call in order.
    fn with_exec_usages(self, usages: Vec<Usage>) -> Self {
        *self.exec_usages.borrow_mut() = usages.into();
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
        let extra = self
            .extra_by_issue
            .iter()
            .find(|(n, _)| *n == issue.number)
            .map(|(_, e)| e.as_str())
            .or(self.extra.as_deref());
        let extra_section = extra.map(|e| format!("\n{e}\n")).unwrap_or_default();
        // Lint-clean by default (ADR-0015): steps ticked and the closing
        // sections present, so tests not about the protocol lint never trip it.
        // `lint_dirty` scripts the opposite; an extra already carrying one of
        // the closing headings keeps its own version.
        let step_line = if self.lint_dirty {
            "- [ ] do a thing\n"
        } else {
            "- [x] do a thing\n"
        };
        let mut body = format!(
            "# Plan for #{}\n\n## Steps\n{}{}{}",
            issue.number,
            step_line.repeat(self.steps),
            ledger_section,
            extra_section,
        );
        if !self.lint_dirty {
            if !body.contains("## Handoff") {
                body.push_str("\n## Handoff\n\n- **Delivered**: scripted work\n");
            }
            if !body.contains("## Plan friction") {
                body.push_str("\n## Plan friction\n\n- none\n");
            }
        }
        fs::write(&path, body)?;
        Ok(Plan {
            open_steps: self.steps,
            recommended_model: None,
            path,
            usage: Usage::default(),
            session_id: None,
        })
    }

    fn execute(&self, _plan: &Plan, ws: &Workspace) -> anyhow::Result<Execution> {
        // The most recently planned issue is the one being executed.
        let number = *self.planned.borrow().last().unwrap();
        // A well-behaved executor's reaction to the ADR-0015 bounce brief:
        // tick every step and append the missing closing sections.
        if self.fix_protocol && ws.ralphy_dir().join("protocol-failure.md").exists() {
            let plan_md = fs::read_to_string(ws.plan_path())?;
            let fixed = plan_md.replace("- [ ]", "- [x]")
                + "\n## Handoff\n\n- **Delivered**: repaired\n\n## Plan friction\n\n- none\n";
            fs::write(ws.plan_path(), fixed)?;
        }
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
            usage: self
                .exec_usages
                .borrow_mut()
                .pop_front()
                .unwrap_or_default(),
            session_id: None,
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
    /// Every `remove_label` call (the label swaps `ralphy triage` performs).
    removed_labels: RefCell<Vec<(u64, String)>>,
    /// Scripted handoff comments returned by `handoff_comment`, by issue number.
    handoffs: HashMap<u64, String>,
    /// Scripted comment threads returned by `issue_comments`, by issue number —
    /// the discussion the runner attaches to an issue before planning.
    comment_threads: HashMap<u64, Vec<String>>,
    /// Scripted open children returned by `open_children`, by parent number —
    /// the open issues whose `## Parent` references a retired bundle.
    children: HashMap<u64, Vec<u64>>,
    /// Scripted labels returned by `issue_labels`, by issue number — lets a test
    /// mark an open blocker as a human gate (`ready-for-human`/`HITL`, ADR-0014).
    issue_labels: HashMap<u64, Vec<String>>,
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

    fn remove_label(&self, number: u64, label: &str) -> anyhow::Result<()> {
        self.removed_labels
            .borrow_mut()
            .push((number, label.to_string()));
        Ok(())
    }

    fn handoff_comment(&self, number: u64) -> anyhow::Result<Option<String>> {
        Ok(self.handoffs.get(&number).cloned())
    }

    fn issue_comments(&self, number: u64) -> anyhow::Result<Vec<String>> {
        Ok(self
            .comment_threads
            .get(&number)
            .cloned()
            .unwrap_or_default())
    }

    fn open_children(&self, number: u64) -> anyhow::Result<Vec<u64>> {
        Ok(self.children.get(&number).cloned().unwrap_or_default())
    }

    fn issue_labels(&self, number: u64) -> anyhow::Result<Vec<String>> {
        Ok(self.issue_labels.get(&number).cloned().unwrap_or_default())
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

/// The commit SHA `refname` resolves to, or `None` when it does not resolve
/// (e.g. a deleted tag).
fn rev_parse(repo: &Path, refname: &str) -> Option<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args([
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("{refname}^{{commit}}"),
        ])
        .output()
        .expect("spawn git");
    out.status
        .success()
        .then(|| String::from_utf8(out.stdout).unwrap().trim().to_string())
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
        comments: vec![],
    }
}

fn issue_labeled(number: u64, labels: &[&str]) -> Issue {
    Issue {
        number,
        title: format!("issue {number}"),
        body: String::new(),
        labels: labels.iter().map(|s| s.to_string()).collect(),
        comments: vec![],
    }
}

fn issue_with_body(number: u64, body: impl Into<String>) -> Issue {
    Issue {
        number,
        title: format!("issue {number}"),
        body: body.into(),
        labels: vec![],
        comments: vec![],
    }
}

/// The canonical human-return label set (ADR-0016) the CLI resolves and passes
/// to the core. Tests use it verbatim; a test needing a custom-mapped name
/// overrides the field after construction.
fn default_human_return() -> Vec<String> {
    [
        "ready-for-human",
        "HITL",
        "needs-info",
        "needs-triage",
        "wontfix",
        "triage-agent",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

fn cfg(repo: &Path, stamp: &str, dry_run: bool) -> QueueConfig {
    QueueConfig {
        repo_root: repo.to_path_buf(),
        base_branch: "main".into(),
        dry_run,
        stamp: stamp.into(),
        branch_mode: BranchMode::New,
        forced_issues: Vec::new(),
        stop_on_limit_plan: false,
        stop_on_limit_exec: false,
        verify_fallback: None,
        verify_timeout: Duration::from_secs(60),
        require_verify_gate: false,
        done_signal: "DONE_TOKEN".into(),
        human_return_labels: default_human_return(),
    }
}

fn cfg_only(repo: &Path, stamp: &str, only: u64) -> QueueConfig {
    QueueConfig {
        repo_root: repo.to_path_buf(),
        base_branch: "main".into(),
        dry_run: false,
        stamp: stamp.into(),
        branch_mode: BranchMode::New,
        forced_issues: vec![only],
        stop_on_limit_plan: false,
        stop_on_limit_exec: false,
        verify_fallback: None,
        verify_timeout: Duration::from_secs(60),
        require_verify_gate: false,
        done_signal: "DONE_TOKEN".into(),
        human_return_labels: default_human_return(),
    }
}

fn cfg_forced(repo: &Path, stamp: &str, forced: Vec<u64>) -> QueueConfig {
    QueueConfig {
        repo_root: repo.to_path_buf(),
        base_branch: "main".into(),
        dry_run: false,
        stamp: stamp.into(),
        branch_mode: BranchMode::New,
        forced_issues: forced,
        stop_on_limit_plan: false,
        stop_on_limit_exec: false,
        verify_fallback: None,
        verify_timeout: Duration::from_secs(60),
        require_verify_gate: false,
        done_signal: "DONE_TOKEN".into(),
        human_return_labels: default_human_return(),
    }
}

fn cfg_current(repo: &Path, stamp: &str) -> QueueConfig {
    QueueConfig {
        repo_root: repo.to_path_buf(),
        base_branch: "main".into(),
        dry_run: false,
        stamp: stamp.into(),
        branch_mode: BranchMode::Current,
        forced_issues: Vec::new(),
        stop_on_limit_plan: false,
        stop_on_limit_exec: false,
        verify_fallback: None,
        verify_timeout: Duration::from_secs(60),
        require_verify_gate: false,
        done_signal: "DONE_TOKEN".into(),
        human_return_labels: default_human_return(),
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
        forced_issues: Vec::new(),
        stop_on_limit_plan: true,
        stop_on_limit_exec: true,
        verify_fallback: None,
        verify_timeout: Duration::from_secs(60),
        require_verify_gate: false,
        done_signal: "DONE_TOKEN".into(),
        human_return_labels: default_human_return(),
    }
}

/// A New-mode config with the per-phase asymmetry a split run produces: the
/// planner auto-resumes through a plan-time limit (`stop_on_limit_plan = false`,
/// e.g. a Claude planner) while the executor stops on an execute-time limit
/// (`stop_on_limit_exec = true`, e.g. an OpenCode executor). See docs/adr/0009.
fn cfg_split_limit(repo: &Path, stamp: &str) -> QueueConfig {
    QueueConfig {
        repo_root: repo.to_path_buf(),
        base_branch: "main".into(),
        dry_run: false,
        stamp: stamp.into(),
        branch_mode: BranchMode::New,
        forced_issues: Vec::new(),
        stop_on_limit_plan: false,
        stop_on_limit_exec: true,
        verify_fallback: None,
        verify_timeout: Duration::from_secs(60),
        require_verify_gate: false,
        done_signal: "DONE_TOKEN".into(),
        human_return_labels: default_human_return(),
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

/// The event fields a capture cares about (#96): the message plus the two raw plan
/// snapshots and the serialized steps carried on the plan-lifecycle emissions.
///
/// #219 generalizes it into a characterization harness: `level`/`target` come off
/// the event metadata and `all` holds EVERY field rendered as a string, so a
/// vocabulary pin can assert the exact key set and the observed encoding
/// (`%order` arrives as `a -> b`, `?blockers` as `[139]`).
#[derive(Default)]
struct CapturedFields {
    message: String,
    plan_md: Option<String>,
    steps_json: Option<String>,
    level: Option<tracing::Level>,
    target: String,
    all: std::collections::BTreeMap<String, String>,
}

impl CapturedFields {
    /// The sorted field names present on this event, `message` included — the
    /// shape a pin asserts so an ADDED or DROPPED field reds.
    fn keys(&self) -> Vec<&str> {
        self.all.keys().map(String::as_str).collect()
    }

    /// The rendered value of `name`, or `""` when the field is absent.
    fn get(&self, name: &str) -> &str {
        self.all.get(name).map_or("", String::as_str)
    }
}

impl tracing::field::Visit for CapturedFields {
    fn record_u64(&mut self, field: &tracing::field::Field, value: u64) {
        self.all.insert(field.name().to_string(), value.to_string());
    }

    fn record_i64(&mut self, field: &tracing::field::Field, value: i64) {
        self.all.insert(field.name().to_string(), value.to_string());
    }

    fn record_f64(&mut self, field: &tracing::field::Field, value: f64) {
        self.all.insert(field.name().to_string(), value.to_string());
    }

    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        self.all.insert(field.name().to_string(), value.to_string());
    }

    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        // `%plan_md`/`%steps_json` (Display) and the event message all arrive here.
        let rendered = format!("{value:?}");
        self.all.insert(field.name().to_string(), rendered.clone());
        match field.name() {
            "message" => self.message = rendered,
            "plan_md" => self.plan_md = Some(rendered),
            "steps_json" => self.steps_json = Some(rendered),
            _ => {}
        }
    }
}

std::thread_local! {
    /// The active capture target for THIS thread, if any. A `#[test]` runs on its own
    /// thread, so a test that sets this captures only its own run's events.
    static CAPTURE_TARGET: RefCell<Option<std::sync::Arc<std::sync::Mutex<Vec<CapturedFields>>>>> =
        const { RefCell::new(None) };
}

/// A process-global `tracing::Subscriber` that routes every event to the current
/// thread's [`CAPTURE_TARGET`] (a no-op when none is set). Installed ONCE as the
/// global default so callsite interest caches as enabled and a concurrent
/// no-subscriber `run_queue` in a sibling test can never poison it (the failure mode
/// of a bare `with_default`, whose thread-local dispatcher is not consulted when a
/// callsite is first registered on another thread). No `tracing-subscriber` dep.
struct GlobalCapture;

impl tracing::Subscriber for GlobalCapture {
    fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
        true
    }
    fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }
    fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
    fn event(&self, event: &tracing::Event<'_>) {
        CAPTURE_TARGET.with(|t| {
            if let Some(target) = t.borrow().as_ref() {
                let mut f = CapturedFields {
                    level: Some(*event.metadata().level()),
                    target: event.metadata().target().to_string(),
                    ..Default::default()
                };
                event.record(&mut f);
                target.lock().unwrap().push(f);
            }
        });
    }
    fn enter(&self, _: &tracing::span::Id) {}
    fn exit(&self, _: &tracing::span::Id) {}
}

/// Install [`GlobalCapture`] as the process's global tracing default exactly once;
/// a subsequent call (or a global default set elsewhere) is a harmless no-op.
fn install_global_capture() {
    use std::sync::OnceLock;
    static ONCE: OnceLock<()> = OnceLock::new();
    ONCE.get_or_init(|| {
        let _ = tracing::subscriber::set_global_default(GlobalCapture);
    });
}

#[test]
fn runner_emits_plan_written_steps_and_plan_opened_closed_snapshots() {
    // A single green issue exercises the plan-write point (plan written + plan
    // opened) and the close read (plan closed). Capture the run's tracing stream and
    // assert the three plan-lifecycle emissions carry their #96 fields.
    let repo = init_repo("plan-events");
    let queue = vec![issue(7)];
    let agent = ScriptedAgent::new(vec![Outcome::Done]);
    let tracker = RecordingTracker::default();

    install_global_capture();
    let captured = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    CAPTURE_TARGET.with(|t| *t.borrow_mut() = Some(captured.clone()));
    let report = run_queue(
        &cfg(&repo, "stamp-plan-events", false),
        &queue,
        &agent,
        &tracker,
        &ScriptedClock::never(),
    );
    CAPTURE_TARGET.with(|t| *t.borrow_mut() = None);
    let report = report.unwrap();
    assert!(report.stop.is_none(), "a single green issue completes");

    let events = captured.lock().unwrap();
    let find = |msg: &str| events.iter().find(|f| f.message == msg);

    // `plan written` now carries the serialized steps ([{text,status}]).
    let written = find("plan written").expect("a plan written event");
    let steps = written.steps_json.as_deref().expect("steps_json present");
    assert!(
        steps.contains("do a thing") && steps.contains("checked"),
        "steps_json must carry the checked step: {steps}"
    );

    // `plan opened` carries the raw plan markdown at the write point.
    let opened = find("plan opened").expect("a plan opened event");
    assert!(
        opened
            .plan_md
            .as_deref()
            .is_some_and(|m| m.contains("## Steps")),
        "plan opened must carry the raw plan_md: {:?}",
        opened.plan_md
    );

    // `plan closed` carries the raw plan markdown at the close read.
    let closed = find("plan closed").expect("a plan closed event");
    assert!(
        closed
            .plan_md
            .as_deref()
            .is_some_and(|m| m.contains("## Steps")),
        "plan closed must carry the raw plan_md: {:?}",
        closed.plan_md
    );

    fs::remove_dir_all(&repo).ok();
}

// ── #219: characterization pins over the core-emitted event vocabulary ───────
//
// Each pin asserts the FULL `(level, target, message, field-key-set)` triple of a
// consumed message, plus the observed encoding of the interesting values (`%order`
// arrives rendered, `?blockers` as a Debug list). An added, dropped, renamed, or
// re-sigiled field reds the pin — that is the drift class ADR-0039 §2 names. The
// CLI-side decoder that consumes these lives in
// `crates/ralphy-cli/src/runstate/event.rs`; the remaining 14 messages are pinned
// there (`runstate::capture`).

/// The `tracing` target every migrated emission carries (ADR-0039 §1): a helper
/// in `ralphy_core::emit` builds tracing's `static` callsite `Metadata`, so the
/// target is the helper's module — it physically cannot forward the caller's.
/// The decoder ignores `target`, so this is the migration's ONE observable change.
const T_EMIT: &str = "ralphy_core::emit";

/// Run `f` with this thread's `tracing` events captured, in order.
fn capture_run<T>(f: impl FnOnce() -> T) -> (T, Vec<CapturedFields>) {
    install_global_capture();
    let sink = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    CAPTURE_TARGET.with(|t| *t.borrow_mut() = Some(sink.clone()));
    let out = f();
    CAPTURE_TARGET.with(|t| *t.borrow_mut() = None);
    let events = std::mem::take(&mut *sink.lock().unwrap());
    (out, events)
}

/// Assert the `(level, target, message, field-key-set)` triple of `message` and
/// hand the event back for per-field value assertions.
#[track_caller]
fn pin<'a>(
    events: &'a [CapturedFields],
    message: &str,
    target: &str,
    keys: &[&str],
) -> &'a CapturedFields {
    let ev = events
        .iter()
        .find(|f| f.message == message)
        .unwrap_or_else(|| {
            let seen: Vec<&str> = events.iter().map(|e| e.message.as_str()).collect();
            panic!("no `{message}` event was emitted; captured: {seen:?}")
        });
    assert_eq!(
        ev.level,
        Some(tracing::Level::INFO),
        "`{message}` must stay INFO — a WARN/ERROR decodes as a generic Notice"
    );
    assert_eq!(ev.target, target, "`{message}` target drifted");
    assert_eq!(ev.keys(), keys, "`{message}` field set drifted");
    ev
}

#[test]
fn pins_green_run_vocabulary() {
    let repo = init_repo("pins-green");
    let queue = vec![issue(7)];
    let agent = ScriptedAgent::new(vec![Outcome::Done]);
    let tracker = RecordingTracker::default();

    let (report, events) = capture_run(|| {
        run_queue(
            &cfg(&repo, "stamp-pins-green", false),
            &queue,
            &agent,
            &tracker,
            &ScriptedClock::never(),
        )
    });
    assert!(
        report.unwrap().stop.is_none(),
        "a single green issue completes"
    );

    let started = pin(
        &events,
        "issue started",
        T_EMIT,
        &["message", "number", "title"],
    );
    assert_eq!(started.get("number"), "7");
    assert_eq!(started.get("title"), "issue 7");

    let written = pin(
        &events,
        "plan written",
        T_EMIT,
        &[
            "cr",
            "cw",
            "message",
            "model",
            "number",
            "open_steps",
            "out",
            "steps_json",
            "up",
        ],
    );
    assert_eq!(written.get("number"), "7");
    assert_eq!(written.get("open_steps"), "1");
    // `%steps_json` (Display) arrives as the raw JSON array, NOT a quoted Debug form.
    assert!(
        written.get("steps_json").starts_with('['),
        "steps_json must arrive Display-rendered: {}",
        written.get("steps_json")
    );

    let opened = pin(
        &events,
        "plan opened",
        T_EMIT,
        &["message", "number", "plan_md"],
    );
    assert_eq!(opened.get("number"), "7");
    assert!(opened.get("plan_md").contains("## Steps"));

    let closed = pin(
        &events,
        "plan closed",
        T_EMIT,
        &["message", "number", "plan_md"],
    );
    assert_eq!(closed.get("number"), "7");
    assert!(closed.get("plan_md").contains("## Steps"));

    let green = pin(
        &events,
        "green — issue closed",
        T_EMIT,
        &[
            "cr",
            "cw",
            "invocations",
            "message",
            "model",
            "number",
            "out",
            "tokens",
            "up",
        ],
    );
    assert_eq!(green.get("number"), "7");
    // A clean green issue is two vendor spawns — plan + execute, no repair/protocol
    // bounce — so the invocation count is 2.
    assert_eq!(green.get("invocations"), "2");

    fs::remove_dir_all(&repo).ok();
}

// ── #225: exec_usage folds model instead of dropping it on accumulate ────────

#[test]
fn exec_usage_single_attempt_keeps_model() {
    let repo = init_repo("exec-usage-single");
    let queue = vec![issue(7)];
    let agent = ScriptedAgent::new(vec![Outcome::Done]).with_exec_usages(vec![Usage {
        input: 100,
        output: 400,
        cache_read: 200,
        cache_creation: 300,
        model: Some("claude-opus-4-8".into()),
    }]);
    let tracker = RecordingTracker::default();

    let (report, events) = capture_run(|| {
        run_queue(
            &cfg(&repo, "stamp-exec-usage-single", false),
            &queue,
            &agent,
            &tracker,
            &ScriptedClock::never(),
        )
    });
    assert!(report.unwrap().stop.is_none());

    let green = pin(
        &events,
        "green — issue closed",
        T_EMIT,
        &[
            "cr",
            "cw",
            "invocations",
            "message",
            "model",
            "number",
            "out",
            "tokens",
            "up",
        ],
    );
    assert_eq!(green.get("model"), "claude-opus-4-8");
    assert_eq!(green.get("up"), "100");
    assert_eq!(green.get("out"), "400");

    fs::remove_dir_all(&repo).ok();
}

#[test]
fn exec_usage_resume_loop_folds_heaviest_model() {
    let repo = init_repo("exec-usage-resume");
    let queue = vec![issue(7)];
    let agent = ScriptedAgent::scripted(vec![
        (Outcome::Limit(Some("15:00".into())), false),
        (Outcome::Done, true),
    ])
    .with_exec_usages(vec![
        Usage {
            input: 100,
            output: 400,
            cache_read: 200,
            cache_creation: 300,
            model: Some("claude-haiku-4-5".into()),
        },
        Usage {
            input: 1000,
            output: 4000,
            cache_read: 2000,
            cache_creation: 3000,
            model: Some("claude-opus-4-8".into()),
        },
    ]);
    let tracker = RecordingTracker::default();

    let (report, events) = capture_run(|| {
        run_queue(
            &cfg(&repo, "stamp-exec-usage-resume", false),
            &queue,
            &agent,
            &tracker,
            &ScriptedClock::never(),
        )
    });
    assert!(report.unwrap().stop.is_none());

    let green = pin(
        &events,
        "green — issue closed",
        T_EMIT,
        &[
            "cr",
            "cw",
            "invocations",
            "message",
            "model",
            "number",
            "out",
            "tokens",
            "up",
        ],
    );
    assert_eq!(green.get("model"), "claude-opus-4-8");
    assert_eq!(green.get("up"), "1100");
    assert_eq!(green.get("cr"), "2200");
    assert_eq!(green.get("cw"), "3300");
    assert_eq!(green.get("out"), "4400");

    fs::remove_dir_all(&repo).ok();
}

#[test]
fn exec_usage_without_model_stays_unattributed() {
    let repo = init_repo("exec-usage-unattributed");
    let queue = vec![issue(7)];
    let agent = ScriptedAgent::new(vec![Outcome::Done]).with_exec_usages(vec![Usage {
        input: 10,
        output: 0,
        cache_read: 0,
        cache_creation: 0,
        model: None,
    }]);
    let tracker = RecordingTracker::default();

    let (report, events) = capture_run(|| {
        run_queue(
            &cfg(&repo, "stamp-exec-usage-unattributed", false),
            &queue,
            &agent,
            &tracker,
            &ScriptedClock::never(),
        )
    });
    assert!(report.unwrap().stop.is_none());

    let green = pin(
        &events,
        "green — issue closed",
        T_EMIT,
        &[
            "cr",
            "cw",
            "invocations",
            "message",
            "model",
            "number",
            "out",
            "tokens",
            "up",
        ],
    );
    assert_eq!(green.get("model"), "");
    assert_eq!(green.get("up"), "10");

    fs::remove_dir_all(&repo).ok();
}

#[test]
fn pins_skip_and_stop_vocabulary() {
    // Non-green stop: #1 green, #2 blocked.
    let repo = init_repo("pins-nongreen");
    let queue = vec![issue(1), issue(2)];
    let agent = ScriptedAgent::new(vec![Outcome::Done, Outcome::Blocked("nope".into())]);
    let (_r, events) = capture_run(|| {
        run_queue(
            &cfg(&repo, "stamp-pins-nongreen", false),
            &queue,
            &agent,
            &RecordingTracker::default(),
            &ScriptedClock::never(),
        )
    });
    let non_green = pin(
        &events,
        "non-green — stopping run",
        T_EMIT,
        &["message", "number", "outcome"],
    );
    assert_eq!(non_green.get("number"), "2");
    // `?outcome` (Debug on the shorthand) — the decoder reads this rendered form.
    assert_eq!(non_green.get("outcome"), "Blocked(\"nope\")");
    fs::remove_dir_all(&repo).ok();

    // Deadline: the clock reports passed before #2.
    let repo = init_repo("pins-deadline");
    let queue = vec![issue(1), issue(2)];
    let agent = ScriptedAgent::new(vec![Outcome::Done, Outcome::Done]);
    let (_r, events) = capture_run(|| {
        run_queue(
            &cfg(&repo, "stamp-pins-deadline", false),
            &queue,
            &agent,
            &RecordingTracker::default(),
            &ScriptedClock::passes_after(1),
        )
    });
    assert_eq!(
        pin(
            &events,
            "deadline passed — not starting issue",
            T_EMIT,
            &["message", "number"],
        )
        .get("number"),
        "2"
    );
    fs::remove_dir_all(&repo).ok();

    // Stop-before label on #2.
    let repo = init_repo("pins-stopbefore");
    let queue = vec![issue(1), issue_labeled(2, &["stop-before"]), issue(3)];
    let agent = ScriptedAgent::new(vec![Outcome::Done]);
    let (_r, events) = capture_run(|| {
        run_queue(
            &cfg(&repo, "stamp-pins-stopbefore", false),
            &queue,
            &agent,
            &RecordingTracker::default(),
            &ScriptedClock::never(),
        )
    });
    assert_eq!(
        pin(
            &events,
            "stop-before label — halting run before this issue",
            T_EMIT,
            &["message", "number"],
        )
        .get("number"),
        "2"
    );
    fs::remove_dir_all(&repo).ok();

    // Human-return label on #1; the queue continues to #2.
    let repo = init_repo("pins-humanreturn");
    let queue = vec![issue_labeled(1, &["AFK", "needs-info"]), issue(2)];
    let agent = ScriptedAgent::new(vec![Outcome::Done]);
    let (_r, events) = capture_run(|| {
        run_queue(
            &cfg(&repo, "stamp-pins-hr", false),
            &queue,
            &agent,
            &RecordingTracker::default(),
            &ScriptedClock::never(),
        )
    });
    let hr = pin(
        &events,
        "human-return label — skipping issue",
        T_EMIT,
        &["label", "message", "number"],
    );
    assert_eq!(hr.get("number"), "1");
    // `%label` (Display) — the decoder's `clean_opt` strips no quotes here.
    assert_eq!(hr.get("label"), "needs-info");
    fs::remove_dir_all(&repo).ok();

    // Verify gate red past the repair budget on #1; the queue continues to #2.
    let repo = init_repo("pins-verifyfail");
    let queue = vec![issue(1), issue(2)];
    let agent = ScriptedAgent::new(vec![Outcome::Done, Outcome::Done])
        .with_plan_extra_for(1, format!("## Verify\n\n{}\n", verify_fail_line()))
        .with_plan_extra_for(2, format!("## Verify\n\n{}\n", verify_ok_line()));
    let (_r, events) = capture_run(|| {
        run_queue(
            &cfg(&repo, "stamp-pins-verifyfail", false),
            &queue,
            &agent,
            &RecordingTracker::default(),
            &ScriptedClock::never(),
        )
    });
    let vg = pin(
        &events,
        "verify gate failed — skipping issue",
        T_EMIT,
        &["message", "number", "summary"],
    );
    assert_eq!(vg.get("number"), "1");
    assert!(
        !vg.get("summary").is_empty(),
        "the `%summary` field must carry the failure text"
    );
    fs::remove_dir_all(&repo).ok();
}

#[test]
fn pins_blocked_and_split_vocabulary() {
    // #5 blocked by open, agent-owned #2.
    let repo = init_repo("pins-blocked");
    let queue = vec![issue_with_body(5, "## Blocked by\n- #2\n"), issue(2)];
    let agent = ScriptedAgent::new(vec![Outcome::Done]);
    let (_r, events) = capture_run(|| {
        run_queue(
            &cfg(&repo, "stamp-pins-blocked", false),
            &queue,
            &agent,
            &RecordingTracker::default(),
            &ScriptedClock::never(),
        )
    });
    let blocked = pin(
        &events,
        "blocked by open issue(s) — skipping",
        T_EMIT,
        &["blockers", "message", "number"],
    );
    assert_eq!(blocked.get("number"), "5");
    // `?blockers` — a Debug-rendered `Vec<u64>` the decoder parses the numbers out of.
    assert_eq!(blocked.get("blockers"), "[2]");
    fs::remove_dir_all(&repo).ok();

    // #5 blocked by open #2, which carries a human gate.
    let repo = init_repo("pins-humangate");
    let queue = vec![issue_with_body(5, "## Blocked by\n- #2\n"), issue(7)];
    let agent = ScriptedAgent::new(vec![Outcome::Done]);
    let tracker = RecordingTracker {
        issue_labels: HashMap::from([(2u64, vec!["ready-for-human".to_string()])]),
        ..Default::default()
    };
    let (_r, events) = capture_run(|| {
        run_queue(
            &cfg(&repo, "stamp-pins-humangate", false),
            &queue,
            &agent,
            &tracker,
            &ScriptedClock::never(),
        )
    });
    let hb = pin(
        &events,
        "blocked — waiting on human",
        T_EMIT,
        &["blockers", "human_blockers", "message", "number"],
    );
    assert_eq!(hb.get("number"), "5");
    assert_eq!(hb.get("blockers"), "[2]");
    assert_eq!(hb.get("human_blockers"), "[2]");
    fs::remove_dir_all(&repo).ok();

    // An infeasible plan whose reason names a bundle → the needs-split verdict.
    let repo = init_repo("pins-bundle");
    let queue = vec![issue(3)];
    let agent = ScriptedAgent::new(vec![]).infeasible().with_plan_extra(
        "## Feasible: no\nThe issue bundles six PRD breakdown tasks; split into W1-T01..T06.",
    );
    let (_r, events) = capture_run(|| {
        run_queue(
            &cfg(&repo, "stamp-pins-bundle", false),
            &queue,
            &agent,
            &RecordingTracker::default(),
            &ScriptedClock::never(),
        )
    });
    assert_eq!(
        pin(
            &events,
            "bundle plan — needs split",
            T_EMIT,
            &["message", "number"],
        )
        .get("number"),
        "3"
    );
    fs::remove_dir_all(&repo).ok();
}

#[test]
fn pins_usage_limit_vocabulary() {
    // The two sleep-boundary emissions live on the REAL `WallClock`, not the
    // `ScriptedClock` the runner tests inject — so drive it directly. A reset
    // instant more than the 5-minute wake buffer in the past makes the wait
    // resolve on its first loop turn, with no sleep.
    let past = (chrono::Local::now() - chrono::Duration::minutes(30)).to_rfc3339();
    let (outcome, events) =
        capture_run(|| ralphy_core::WallClock { deadline: None }.wait_for_reset(&past));
    assert_eq!(outcome, ralphy_core::WaitOutcome::Resumed);

    let sleep = pin(
        &events,
        "usage limit — waiting for reset",
        T_EMIT,
        &["hint", "message", "reset", "target_epoch"],
    );
    // `%reset` is the WAKE time-of-day (`HH:MM`), not the raw hint — the decoder
    // carries it straight into the card's countdown label.
    assert_eq!(
        sleep.get("reset").len(),
        5,
        "reset is HH:MM: {}",
        sleep.get("reset")
    );
    assert_eq!(sleep.get("hint"), past, "the raw hint stays on `hint`");
    assert!(
        sleep.get("target_epoch").parse::<i64>().is_ok(),
        "target_epoch is an i64 timestamp: {}",
        sleep.get("target_epoch")
    );

    pin(&events, "reset reached — resuming", T_EMIT, &["message"]);
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
fn view_and_run_agree_issue_for_issue() {
    // ADR-0020 criterion #7: the read-only queue view (`resolve_queue_view`) and a
    // real `run_queue` over the SAME fixture + tracker must classify every issue
    // identically — stop-before, blocked, human-return, eligible — so the CLI
    // listing can never disagree with what the run does. FAILS if either the view
    // or the runner drifts from the shared precedence.
    let repo = init_repo("view-run-agree");
    let queue = vec![
        issue_with_body(5, "## Blocked by\n- #2\n"), // #2 open → Blocked
        issue_labeled(3, &["needs-info"]),           // human-return → Skipped
        issue(7),                                    // clean → Eligible
        issue_labeled(9, &["stop-before"]),          // first stop-before → halts
    ];
    let agent = ScriptedAgent::new(vec![Outcome::Done]); // only #7 executes
    let tracker = RecordingTracker::default(); // #2 open; no children/labels

    // Resolve the view over the SAME fixture + tracker the run consumes.
    let view = resolve_queue_view(&queue, &[], &default_human_return(), &tracker).unwrap();

    let report = run_queue(
        &cfg(&repo, "stamp-view-run-agree", false),
        &queue,
        &agent,
        &tracker,
        &ScriptedClock::never(),
    )
    .unwrap();

    let vs = |n: u64| view.issues.iter().find(|i| i.number == n).unwrap();
    let worked = |n: u64| report.worked.iter().find(|r| r.number == n);

    // Stop-before: the view marks #9 and the run halts there, producing no row.
    assert_eq!(vs(9).queue_status, QueueStatus::StopBefore);
    assert_eq!(view.stop_before, Some(9));
    match &report.stop {
        Some(StopReason::StopBefore { number }) => assert_eq!(*number, 9),
        other => panic!("expected StopBefore, got {other:?}"),
    }
    assert!(worked(9).is_none(), "the stop-before issue is never worked");

    // Blocked: the view's blocked_by equals the run's IssueResult.blocked_by.
    assert_eq!(vs(5).queue_status, QueueStatus::Blocked);
    let r5 = worked(5).expect("#5 produces a skip row");
    assert!(r5.outcome.is_none() && !r5.closed);
    assert_eq!(vs(5).blocked_by, r5.blocked_by);
    assert_eq!(vs(5).blocked_by, vec![2]);

    // Human-return: view Skipped ⇔ the run skips it (no outcome, empty blocked_by).
    assert_eq!(vs(3).queue_status, QueueStatus::Skipped);
    let r3 = worked(3).expect("#3 produces a skip row");
    assert!(r3.outcome.is_none() && !r3.closed && r3.blocked_by.is_empty());
    assert_eq!(vs(3).skip_reason.as_deref(), Some("needs-info"));

    // Eligible: view Eligible ⇔ the run actually worked it (an outcome present).
    assert_eq!(vs(7).queue_status, QueueStatus::Eligible);
    assert_eq!(vs(7).position, Some(1));
    let r7 = worked(7).expect("#7 is worked");
    assert!(r7.outcome.is_some(), "eligible issue was executed");
    assert!(agent.executed.borrow().contains(&7));

    fs::remove_dir_all(&repo).ok();
}

#[test]
fn view_and_run_agree_under_assignee_filter() {
    // ADR-0021 criterion #8: the `--assignee` filter is FETCH-ONLY — it narrows
    // which issues `list_queue` returns, and never touches judgment. So the runner
    // and the view both see only the surviving (filtered) subset, and blocked-by
    // must STILL consult the tracker (`is_closed`), so an issue blocked by an OPEN
    // issue OUTSIDE the filtered subset stays `Blocked`. This models the filtered
    // queue as the already-narrowed subset and asserts view/run parity over it.
    let repo = init_repo("view-run-agree-assignee");
    // The filtered subset the runner receives (say `@me` is assigned #7 and #9).
    // #4 is a colleague's OPEN issue — outside the subset, but #7 is blocked by it.
    let queue = vec![
        issue_with_body(7, "## Blocked by\n- #4\n"), // #4 open & out-of-subset → Blocked
        issue(9),                                    // clean → Eligible
    ];
    let agent = ScriptedAgent::new(vec![Outcome::Done]); // only #9 executes
    let tracker = RecordingTracker::default(); // #4 absent from closed_issues → open

    let view = resolve_queue_view(&queue, &[], &default_human_return(), &tracker).unwrap();

    let report = run_queue(
        &cfg(&repo, "stamp-view-run-assignee", false),
        &queue,
        &agent,
        &tracker,
        &ScriptedClock::never(),
    )
    .unwrap();

    let vs = |n: u64| view.issues.iter().find(|i| i.number == n).unwrap();
    let worked = |n: u64| report.worked.iter().find(|r| r.number == n);

    // Blocked-by still consults the tracker even though #4 is not in the subset:
    // the view marks #7 Blocked and the run records the same blocker.
    assert_eq!(vs(7).queue_status, QueueStatus::Blocked);
    let r7 = worked(7).expect("#7 produces a skip row");
    assert!(r7.outcome.is_none() && !r7.closed);
    assert_eq!(vs(7).blocked_by, r7.blocked_by);
    assert_eq!(
        vs(7).blocked_by,
        vec![4],
        "blocked by the out-of-subset open #4"
    );

    // Eligible: view Eligible ⇔ the run actually worked #9.
    assert_eq!(vs(9).queue_status, QueueStatus::Eligible);
    let r9 = worked(9).expect("#9 is worked");
    assert!(r9.outcome.is_some(), "eligible issue was executed");
    assert!(agent.executed.borrow().contains(&9));
    assert!(!agent.executed.borrow().contains(&7), "#7 stayed blocked");

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
fn forced_issues_list_ignores_stop_before_across_the_list() {
    let repo = init_repo("forced-list-stop-before");
    // `--issues 1,2`: an explicit, ordered list. #2 carries `stop-before`, but both
    // are named, so the run works the whole list in order without halting — the
    // generalization of `--only-issue` to a set.
    let queue = vec![issue(1), issue_labeled(2, &["stop-before"])];
    let agent = ScriptedAgent::new(vec![Outcome::Done, Outcome::Done]);
    let tracker = RecordingTracker::default();

    let report = run_queue(
        &cfg_forced(&repo, "stamp-forced-list", vec![1, 2]),
        &queue,
        &agent,
        &tracker,
        &ScriptedClock::never(),
    )
    .unwrap();

    assert_eq!(
        *agent.executed.borrow(),
        vec![1, 2],
        "both listed issues run, in order, despite stop-before on #2"
    );
    assert!(
        report.stop.is_none(),
        "a listed issue's stop-before never halts a forced run"
    );

    fs::remove_dir_all(&repo).ok();
}

// ── ADR-0016: human-return labels outrank queue labels ───────────────────────

#[test]
fn human_return_label_skips_issue_and_continues() {
    let repo = init_repo("human-return-skip");
    // #1 carries a queue label PLUS a human-return label; #2 is a plain queue
    // issue. #1 must be skipped (not planned, not executed, not closed) and the
    // queue must continue to #2.
    let queue = vec![issue_labeled(1, &["AFK", "needs-info"]), issue(2)];
    let agent = ScriptedAgent::new(vec![Outcome::Done]);
    let tracker = RecordingTracker::default();

    let report = run_queue(
        &cfg(&repo, "stamp-hr-skip", false),
        &queue,
        &agent,
        &tracker,
        &ScriptedClock::never(),
    )
    .unwrap();

    assert_eq!(*agent.planned.borrow(), vec![2], "#1 never planned");
    assert_eq!(*agent.executed.borrow(), vec![2], "#1 never executed");
    // #1 recorded as a skip (outcome None, not closed); #2 worked. Run continues.
    assert_eq!(report.worked.len(), 2, "both issues produce a result row");
    let skipped = report.worked.iter().find(|r| r.number == 1).unwrap();
    assert!(skipped.outcome.is_none(), "#1 skipped, no outcome");
    assert!(!skipped.closed, "#1 not closed");
    assert!(
        !tracker.closes.borrow().iter().any(|(n, _)| *n == 1),
        "#1 never closed on the tracker"
    );
    assert!(report.stop.is_none(), "the run continues past the skip");

    fs::remove_dir_all(&repo).ok();
}

#[test]
fn reparked_issue_is_not_reworked_on_next_run() {
    // Regression for the ADR-0015 re-park bug (ADR-0016 amendment): a verify-gate
    // park leaves the issue labeled `ready-for-human` while its queue label stays.
    // On the next run that exact label state must NOT re-queue the issue.
    let repo = init_repo("reparked");
    let queue = vec![issue_labeled(1, &["AFK", "ready-for-human"])];
    let agent = ScriptedAgent::new(vec![Outcome::Done]);
    let tracker = RecordingTracker::default();

    let report = run_queue(
        &cfg(&repo, "stamp-reparked", false),
        &queue,
        &agent,
        &tracker,
        &ScriptedClock::never(),
    )
    .unwrap();

    assert!(
        agent.planned.borrow().is_empty(),
        "parked issue not planned"
    );
    assert!(
        agent.executed.borrow().is_empty(),
        "parked issue not executed"
    );
    assert!(
        tracker.closes.borrow().is_empty(),
        "parked issue not closed"
    );
    let row = report.worked.iter().find(|r| r.number == 1).unwrap();
    assert!(!row.closed, "parked issue stays open");

    fs::remove_dir_all(&repo).ok();
}

#[test]
fn only_issue_does_not_override_human_return() {
    // Unlike stop-before, `--only-issue` must NOT run a human-return-labelled
    // issue: the label may record someone else's state (ADR-0016).
    let repo = init_repo("only-human-return");
    let queue = vec![issue_labeled(1, &["AFK", "HITL"])];
    let agent = ScriptedAgent::new(vec![Outcome::Done]);
    let tracker = RecordingTracker::default();

    let report = run_queue(
        &cfg_only(&repo, "stamp-only-hr", 1),
        &queue,
        &agent,
        &tracker,
        &ScriptedClock::never(),
    )
    .unwrap();

    assert!(
        agent.planned.borrow().is_empty(),
        "only_issue does not override the human-return skip"
    );
    assert!(agent.executed.borrow().is_empty());
    assert!(tracker.closes.borrow().is_empty());
    assert!(report.stop.is_none(), "a skip continues, it does not stop");

    fs::remove_dir_all(&repo).ok();
}

#[test]
fn custom_mapped_human_return_label_skips() {
    // The core honours whatever resolved set the CLI passes: a repo that renames
    // `needs-info` to `waiting-reporter` still parks the issue.
    let repo = init_repo("custom-hr");
    let queue = vec![issue_labeled(1, &["AFK", "waiting-reporter"]), issue(2)];
    let agent = ScriptedAgent::new(vec![Outcome::Done]);
    let tracker = RecordingTracker::default();

    let mut config = cfg(&repo, "stamp-custom-hr", false);
    config.human_return_labels = vec!["waiting-reporter".into()];

    let report = run_queue(&config, &queue, &agent, &tracker, &ScriptedClock::never()).unwrap();

    assert_eq!(*agent.executed.borrow(), vec![2], "#1 skipped, #2 worked");
    assert!(report.stop.is_none());

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
fn per_phase_limit_plans_resume_but_execute_stops() {
    // The split-run asymmetry (ADR-0009): with `stop_on_limit_plan = false` and
    // `stop_on_limit_exec = true`, a plan-time limit auto-resumes (the planner
    // waits and re-plans) while an execute-time limit stops the run and reports
    // the reset. A single `stop_on_limit` field could not express this split — so
    // this test FAILS before the field was split and PASSES after.
    let repo = init_repo("split-limit");
    let queue = vec![issue(8)];
    // One plan-time limit (resolves via resume), then an execute-time limit (stops).
    let agent = ScriptedAgent::new(vec![Outcome::Limit(Some("15:00".into()))])
        .with_plan_scripts(vec![PlanScript::Limit(Some("12:00".into()))]);
    let tracker = RecordingTracker::default();
    let clock = ScriptedClock::never();

    let report = run_queue(
        &cfg_split_limit(&repo, "stamp-split-limit"),
        &queue,
        &agent,
        &tracker,
        &clock,
    )
    .unwrap();

    // Plan auto-resumed: two plan attempts, and the only wait was for the plan
    // reset — the execute-time limit stopped immediately without waiting.
    assert_eq!(
        *agent.plan_attempts.borrow(),
        2,
        "plan auto-resumed once through the plan-time limit"
    );
    assert_eq!(
        *clock.waited_for.borrow(),
        vec!["12:00".to_string()],
        "waited only for the plan reset; the execute limit never waits"
    );
    assert_eq!(
        *agent.executed.borrow(),
        vec![8],
        "executed once, then the execute-time limit stopped the run"
    );

    // Execute stopped as a reported limit carrying the execute-time reset.
    match report.stop {
        Some(StopReason::Limit { number, reset }) => {
            assert_eq!(number, 8);
            assert_eq!(reset, Some("15:00".into()));
        }
        other => panic!("expected Limit stop from the execute phase, got {other:?}"),
    }
    assert!(
        tracker.closes.borrow().is_empty(),
        "a limit-stopped issue is never closed"
    );

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
    // A handoff without a `Knowledge used` block warns but records nothing.
    assert!(
        !Workspace::new(&repo).citations_path().exists(),
        "no citations.jsonl entry when the field is absent"
    );

    fs::remove_dir_all(&repo).ok();
}

#[test]
fn green_close_appends_knowledge_used_citations() {
    let repo = init_repo("citations-close");
    let cited = "## Handoff\n\n- **Delivered**: fix (abc1234)\n- **Knowledge used**:\n  - \"Toolchain & platform\" — cargo test needs docker up first\n  - handoffs.md #5: schema rejects empty DEVICEID\n\n## Plan friction\n\n- none";
    let none = "## Handoff\n\n- **Delivered**: docs (def5678)\n- **Knowledge used**: none\n\n## Plan friction\n\n- none";
    let queue = vec![issue(1), issue(2)];
    let agent = ScriptedAgent::new(vec![Outcome::Done, Outcome::Done])
        .with_plan_extra_for(1, cited)
        .with_plan_extra_for(2, none);
    let tracker = RecordingTracker::default();

    run_queue(
        &cfg(&repo, "stamp-citations", false),
        &queue,
        &agent,
        &tracker,
        &ScriptedClock::never(),
    )
    .unwrap();

    // One JSON line per green close, in queue order — the hit-rate log the
    // consolidation curator prunes against.
    let content = fs::read_to_string(Workspace::new(&repo).citations_path())
        .expect("citations.jsonl written");
    let entries: Vec<ralphy_core::knowledge::CitationEntry> = content
        .lines()
        .map(|l| serde_json::from_str(l).expect("each line is one CitationEntry"))
        .collect();
    assert_eq!(entries.len(), 2, "one entry per green close: {content}");
    assert_eq!(entries[0].issue, 1);
    assert_eq!(entries[0].stamp, "stamp-citations");
    assert_eq!(
        entries[0].citations,
        vec![
            "\"Toolchain & platform\" — cargo test needs docker up first".to_string(),
            "handoffs.md #5: schema rejects empty DEVICEID".to_string(),
        ]
    );
    assert_eq!(entries[1].issue, 2);
    assert_eq!(
        entries[1].citations,
        Vec::<String>::new(),
        "an honest `none` is recorded as an empty list"
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
fn issue_comments_are_attached_to_the_planner_issue_json() {
    // The runner fetches the selected issue's comment thread and folds it into
    // `.ralphy/issue.json` (the `comments` array), so the planner reads the
    // discussion, not just the body.
    let repo = init_repo("comments-attach");
    let queue = vec![issue_with_body(5, "original body")];
    let agent = ScriptedAgent::new(vec![Outcome::Done]);
    let tracker = RecordingTracker {
        comment_threads: HashMap::from([(
            5u64,
            vec![
                "first clarification from a human".to_string(),
                "second: use the staging endpoint".to_string(),
            ],
        )]),
        ..Default::default()
    };

    run_queue(
        &cfg(&repo, "stamp-comments", false),
        &queue,
        &agent,
        &tracker,
        &ScriptedClock::never(),
    )
    .unwrap();

    let issue_json =
        fs::read_to_string(repo.join(".ralphy").join("issue.json")).expect("issue.json written");
    let parsed: serde_json::Value = serde_json::from_str(&issue_json).expect("valid JSON");
    let comments = parsed["comments"].as_array().expect("comments array");
    assert_eq!(comments.len(), 2, "both comments carried into issue.json");
    assert_eq!(comments[0], "first clarification from a human");
    assert_eq!(comments[1], "second: use the staging endpoint");

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
fn human_gate_blocker_is_classified_and_run_continues() {
    // #5 is blocked by #2, an OPEN issue carrying `ready-for-human` (a human
    // gate, ADR-0014). #7 is independent and runnable. Expected: #5 is skipped
    // with #2 recorded in BOTH blocked_by and human_blockers; the run does NOT
    // stop — #7 still runs to a green close. Only #5's chain stalls.
    let repo = init_repo("human-gate");
    let queue = vec![issue_with_body(5, "## Blocked by\n- #2\n"), issue(7)];
    let agent = ScriptedAgent::new(vec![Outcome::Done]); // only #7 executes
    let tracker = RecordingTracker {
        // #2 is open (absent from closed_issues) and carries the human gate.
        issue_labels: HashMap::from([(2u64, vec!["ready-for-human".to_string()])]),
        ..Default::default()
    };

    let report = run_queue(
        &cfg(&repo, "stamp-human-gate", false),
        &queue,
        &agent,
        &tracker,
        &ScriptedClock::never(),
    )
    .unwrap();

    let r5 = report
        .worked
        .iter()
        .find(|r| r.number == 5)
        .expect("#5 in worked");
    assert!(r5.outcome.is_none(), "#5 skipped, never planned");
    assert!(!r5.closed, "#5 not closed");
    assert_eq!(r5.blocked_by, vec![2], "#5 still records its open blocker");
    assert_eq!(
        r5.human_blockers,
        vec![2],
        "#2 is classified as a human gate"
    );

    // The run continued: #7 ran and closed green; no stop.
    assert!(agent.executed.borrow().contains(&7), "#7 must have run");
    let closes: Vec<u64> = tracker.closes.borrow().iter().map(|(n, _)| *n).collect();
    assert_eq!(closes, vec![7], "only #7 closed");
    assert!(
        report.stop.is_none(),
        "a human gate never stops the whole run"
    );

    fs::remove_dir_all(&repo).ok();
}

#[test]
fn ordinary_open_blocker_is_not_a_human_gate() {
    // #5 is blocked by open #2 carrying only `ready-for-agent` (ordinary agent
    // work the queue will clear). Expected: skipped, blocked_by == [2], but
    // human_blockers empty — it is NOT a human gate.
    let repo = init_repo("agent-blocker");
    let queue = vec![issue_with_body(5, "## Blocked by\n- #2\n")];
    let agent = ScriptedAgent::new(vec![]);
    let tracker = RecordingTracker {
        issue_labels: HashMap::from([(2u64, vec!["ready-for-agent".to_string()])]),
        ..Default::default()
    };

    let report = run_queue(
        &cfg(&repo, "stamp-agent-blocker", false),
        &queue,
        &agent,
        &tracker,
        &ScriptedClock::never(),
    )
    .unwrap();

    let r5 = report
        .worked
        .iter()
        .find(|r| r.number == 5)
        .expect("#5 in worked");
    assert_eq!(r5.blocked_by, vec![2]);
    assert!(
        r5.human_blockers.is_empty(),
        "an agent-work blocker is not a human gate"
    );

    fs::remove_dir_all(&repo).ok();
}

#[test]
fn blocked_by_in_consolidated_comment_gates_issue() {
    // #5's body has NO `## Blocked by`; its marked consolidated-spec comment does
    // (ADR-0017). The open blocker #9 named there must gate the issue exactly
    // like one in the body.
    let repo = init_repo("blocked-in-comment");
    let queue = vec![issue_with_body(
        5,
        "Just a prose spec, no blocked-by section.",
    )];
    let agent = ScriptedAgent::new(vec![]); // #5 never planned
    let marker = "<!-- ralphy:consolidated-spec -->";
    let tracker = RecordingTracker {
        comment_threads: HashMap::from([(
            5u64,
            vec![format!(
                "{marker}\n## Consolidated spec\n\n## Blocked by\n- #9\n"
            )],
        )]),
        // #9 is open (absent from closed_issues).
        ..Default::default()
    };

    let report = run_queue(
        &cfg(&repo, "stamp-blocked-comment", false),
        &queue,
        &agent,
        &tracker,
        &ScriptedClock::never(),
    )
    .unwrap();

    let r5 = report
        .worked
        .iter()
        .find(|r| r.number == 5)
        .expect("#5 in worked");
    assert!(r5.outcome.is_none(), "#5 skipped, never planned");
    assert_eq!(
        r5.blocked_by,
        vec![9],
        "the marked comment's blocker gates the queue"
    );
    assert!(agent.planned.borrow().is_empty(), "#5 never planned");

    fs::remove_dir_all(&repo).ok();
}

#[test]
fn limit_no_reset_synthesizes_a_wait_and_auto_resumes() {
    // A limit with no parseable reset (Kimi's 403 account block) no longer stops:
    // it parks a synthesised ~30-min window and re-runs execute() when the wait
    // returns (ADR-0030). Two consecutive no-commit synthetic limits also prove the
    // progress cap is skipped for this path (B2) — a real reset would abandon at two,
    // but an account-wide pause is the human's call, so the loop resumes until Done.
    let repo = init_repo("limit-noreset-resume");
    let queue = vec![issue(11)];
    let agent = ScriptedAgent::scripted(vec![
        (Outcome::Limit(None), false),
        (Outcome::Limit(None), false),
        (Outcome::Done, true),
    ]);
    let tracker = RecordingTracker::default();
    let clock = ScriptedClock::never();

    let report = run_queue(
        &cfg(&repo, "stamp-limit-none", false),
        &queue,
        &agent,
        &tracker,
        &clock,
    )
    .unwrap();

    assert_eq!(
        *agent.executed.borrow(),
        vec![11, 11, 11],
        "execute re-ran after each synthetic wait; the cap did not abandon the issue"
    );
    assert!(
        report.stop.is_none(),
        "a no-reset limit auto-resumes, not a stop"
    );
    // A synthetic, parseable reset target was passed to wait_for_reset each cycle.
    let waited = clock.waited_for.borrow();
    assert_eq!(waited.len(), 2, "waited once per no-reset limit");
    assert!(
        waited.iter().all(|w| !w.is_empty()),
        "each wait carried a synthesised reset target, got {waited:?}"
    );

    fs::remove_dir_all(&repo).ok();
}

#[test]
fn limit_no_reset_stops_when_stop_on_limit() {
    // `--stop-on-limit` is the opt-out (e.g. CI that must not hang): a no-reset limit
    // stops and reports with a None reset instead of parking a synthetic wait.
    let repo = init_repo("limit-noreset-stop");
    let queue = vec![issue(11)];
    let agent = ScriptedAgent::new(vec![Outcome::Limit(None)]);
    let tracker = RecordingTracker::default();

    let report = run_queue(
        &cfg_stop_on_limit(&repo, "stamp-limit-none-stop"),
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

/// A `## Verify` line whose command exits 0 on every platform (argv-only, no
/// shell metacharacters that a non-shell argv split would mangle).
fn verify_ok_line() -> &'static str {
    if cfg!(windows) {
        "cmd /c \"exit 0\""
    } else {
        "sh -c \"exit 0\""
    }
}

/// A `## Verify` line whose command exits non-zero on every platform.
fn verify_fail_line() -> &'static str {
    if cfg!(windows) {
        "cmd /c \"exit 3\""
    } else {
        "sh -c \"exit 3\""
    }
}

#[test]
fn verify_gate_passes_and_issue_closes() {
    // A plan with a `## Verify` section whose command passes: the runner re-runs
    // it over the committed state, sees it pass, posts the honesty artifact, and
    // closes the issue on the existing green path.
    let repo = init_repo("verify-pass");
    let queue = vec![issue(1)];
    let extra = format!("## Verify\n\n{}\n", verify_ok_line());
    let agent = ScriptedAgent::new(vec![Outcome::Done]).with_plan_extra(extra);
    let tracker = RecordingTracker::default();

    let report = run_queue(
        &cfg(&repo, "stamp-verify-pass", false),
        &queue,
        &agent,
        &tracker,
        &ScriptedClock::never(),
    )
    .unwrap();

    assert!(
        report.stop.is_none(),
        "a passing gate does not stop the run"
    );
    let closes: Vec<u64> = tracker.closes.borrow().iter().map(|(n, _)| *n).collect();
    assert_eq!(closes, vec![1], "issue closed on a passing gate");

    // The honesty artifact was posted recording the gate run.
    let comments = tracker.comments.borrow();
    assert!(
        comments
            .iter()
            .any(|(n, b)| *n == 1 && b.contains("## Verify (Ralphy run stamp-verify-pass)")),
        "verify artifact comment posted on pass: {comments:?}"
    );

    fs::remove_dir_all(&repo).ok();
}

#[test]
fn verify_gate_fails_skips_issue_and_continues_queue() {
    // A plan whose `## Verify` command always fails: the runner hands the failure
    // back to the agent up to VERIFY_MAX_REPAIRS times, re-running the SAME gate
    // after each attempt. When the budget is spent the issue is left OPEN (not
    // closed) but the run does NOT stop — it moves on to the next issue. The
    // honesty artifact records the failure for the skipped issue.
    let repo = init_repo("verify-fail");
    let queue = vec![issue(1), issue(2)];
    // #1's gate fails forever; #2 has a passing gate and must still get its turn.
    let agent = ScriptedAgent::new(vec![Outcome::Done, Outcome::Done])
        .with_plan_extra_for(1, format!("## Verify\n\n{}\n", verify_fail_line()))
        .with_plan_extra_for(2, format!("## Verify\n\n{}\n", verify_ok_line()));
    let tracker = RecordingTracker::default();

    let report = run_queue(
        &cfg(&repo, "stamp-verify-fail", false),
        &queue,
        &agent,
        &tracker,
        &ScriptedClock::never(),
    )
    .unwrap();

    // A verify failure no longer stops the run — the whole queue is worked.
    assert!(
        report.stop.is_none(),
        "a failed gate skips the issue but does not stop the run"
    );

    // #1 ran once + VERIFY_MAX_REPAIRS (2) repair attempts; then #2 ran once.
    assert_eq!(
        *agent.executed.borrow(),
        vec![1, 1, 1, 2],
        "#1 repaired twice then skipped; #2 still executed"
    );

    // #1 (gate red) is left open; #2 (gate green) is closed — the queue advanced.
    let closes: Vec<u64> = tracker.closes.borrow().iter().map(|(n, _)| *n).collect();
    assert_eq!(closes, vec![2], "only the passing issue closed");

    // The honesty artifact records #1's failure.
    let comments = tracker.comments.borrow();
    assert!(
        comments
            .iter()
            .any(|(n, b)| *n == 1 && b.contains("Verify gate FAILED")),
        "verify artifact comment posted on the skipped issue: {comments:?}"
    );

    // The worked entry marks #1 a skip (not closed, no outcome); #2 closed.
    let r1 = report.worked.iter().find(|r| r.number == 1).unwrap();
    assert!(!r1.closed, "gate-failed issue is not closed");
    assert!(r1.outcome.is_none(), "a verify skip carries no outcome");
    let r2 = report.worked.iter().find(|r| r.number == 2).unwrap();
    assert!(r2.closed, "the passing issue closed");

    fs::remove_dir_all(&repo).ok();
}

/// A `## Verify` line that fails until the agent's second execute lands, then
/// passes — it keys off the marker file `ScriptedAgent` commits on its 1st repair
/// (`issue-1-1.txt`, where the suffix is the prior execute count). This lets a
/// repair attempt actually flip the gate green.
fn verify_pass_after_repair_line() -> &'static str {
    if cfg!(windows) {
        "cmd /c \"if exist issue-1-1.txt (exit 0) else (exit 3)\""
    } else {
        "sh -c \"test -f issue-1-1.txt\""
    }
}

#[test]
fn verify_gate_repairs_then_closes() {
    // The gate fails on the first run, the runner hands it back, the agent's
    // repair execute lands a commit that flips the gate green, and the issue
    // closes — without the run ever stopping. This is the repair loop's reason to
    // exist: a fixable verify failure no longer hands the branch to a human.
    let repo = init_repo("verify-repair");
    let queue = vec![issue(1)];
    let extra = format!("## Verify\n\n{}\n", verify_pass_after_repair_line());
    // Empty script → every execute defaults to Done + a commit. The 1st execute
    // commits issue-1-0.txt (gate still red), the 1st repair commits issue-1-1.txt
    // (gate goes green).
    let agent = ScriptedAgent::new(vec![]).with_plan_extra(extra);
    let tracker = RecordingTracker::default();

    let report = run_queue(
        &cfg(&repo, "stamp-verify-repair", false),
        &queue,
        &agent,
        &tracker,
        &ScriptedClock::never(),
    )
    .unwrap();

    assert!(
        report.stop.is_none(),
        "a repaired gate does not stop the run"
    );
    assert_eq!(
        *agent.executed.borrow(),
        vec![1, 1],
        "#1 ran once + one repair that fixed the gate"
    );
    let closes: Vec<u64> = tracker.closes.borrow().iter().map(|(n, _)| *n).collect();
    assert_eq!(closes, vec![1], "issue closed after the repair went green");

    // Both the failing and the passing gate runs left honesty artifacts.
    let comments = tracker.comments.borrow();
    assert!(
        comments
            .iter()
            .any(|(n, b)| *n == 1 && b.contains("Verify gate FAILED")),
        "the first (failed) gate run is recorded"
    );
    assert!(
        comments
            .iter()
            .any(|(n, b)| *n == 1 && b.contains("All verify commands passed")),
        "the repaired (passing) gate run is recorded"
    );

    fs::remove_dir_all(&repo).ok();
}

#[test]
fn verify_none_opts_out_and_skips_settings_fallback() {
    // `## Verify: none` is the explicit opt-out: even with a failing settings
    // fallback configured, the gate is skipped and the issue closes.
    let repo = init_repo("verify-none");
    let queue = vec![issue(1)];
    let agent = ScriptedAgent::new(vec![Outcome::Done]).with_plan_extra("## Verify\n\nnone\n");
    let tracker = RecordingTracker::default();

    // A failing fallback that must NOT run because the plan opted out.
    let mut config = cfg(&repo, "stamp-verify-none", false);
    config.verify_fallback = Some(vec![ralphy_core::verify::tokenize(verify_fail_line())]);

    let report = run_queue(&config, &queue, &agent, &tracker, &ScriptedClock::never()).unwrap();

    assert!(report.stop.is_none(), "opt-out closes without a gate");
    let closes: Vec<u64> = tracker.closes.borrow().iter().map(|(n, _)| *n).collect();
    assert_eq!(closes, vec![1], "issue closed under `## Verify: none`");
    // No verify-artifact comment — the gate never ran. (The close still posts
    // the plan's handoff report, which is unrelated to the gate.)
    assert!(
        !tracker
            .comments
            .borrow()
            .iter()
            .any(|(_, b)| b.contains("## Verify (")),
        "opt-out posts no verify artifact"
    );

    fs::remove_dir_all(&repo).ok();
}

#[test]
fn verify_falls_back_to_settings_when_plan_section_absent() {
    // No `## Verify` in the plan → the runner falls back to the per-repo settings
    // command. Here that fallback fails on every attempt, so after the repair
    // budget the issue is skipped (left open) and — as the only issue — the run
    // ends cleanly without stopping.
    let repo = init_repo("verify-fallback");
    let queue = vec![issue(1)];
    // No `## Verify` section in the plan at all.
    let agent = ScriptedAgent::new(vec![Outcome::Done]);
    let tracker = RecordingTracker::default();

    let mut config = cfg(&repo, "stamp-verify-fallback", false);
    config.verify_fallback = Some(vec![ralphy_core::verify::tokenize(verify_fail_line())]);

    let report = run_queue(&config, &queue, &agent, &tracker, &ScriptedClock::never()).unwrap();

    assert!(
        report.stop.is_none(),
        "a failed fallback gate skips the issue, it does not stop the run: {:?}",
        report.stop
    );
    assert!(
        tracker.closes.borrow().is_empty(),
        "fallback gate failure leaves the issue open"
    );
    // The fallback gate actually ran (and failed) — its artifact is posted.
    let comments = tracker.comments.borrow();
    assert!(
        comments
            .iter()
            .any(|(n, b)| *n == 1 && b.contains("Verify gate FAILED")),
        "fallback gate failure posts the verify artifact: {comments:?}"
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

#[test]
fn require_verify_gate_parks_no_gate_issue_and_continues() {
    // ADR-0015: `require_verify_gate` + a plan resolving to NoGate (no
    // `## Verify`, no fallback) → the issue is NOT closed on the self-report; it
    // is labeled ready-for-human with an explanatory comment, and the run
    // continues — the next issue (which has a real gate) still closes.
    let repo = init_repo("require-gate");
    let queue = vec![issue(1), issue(2)];
    let agent = ScriptedAgent::new(vec![Outcome::Done, Outcome::Done])
        .with_plan_extra_for(2, format!("## Verify\n\n{}\n", verify_ok_line()));
    let tracker = RecordingTracker::default();

    let mut config = cfg(&repo, "stamp-require-gate", false);
    config.require_verify_gate = true;

    let report = run_queue(&config, &queue, &agent, &tracker, &ScriptedClock::never()).unwrap();

    // #1 (gateless) stays open and is parked; #2 (gated, green) closes.
    let closes: Vec<u64> = tracker.closes.borrow().iter().map(|(n, _)| *n).collect();
    assert_eq!(closes, vec![2], "the gateless issue must not close");
    assert_eq!(
        tracker.labels.borrow().as_slice(),
        &[(1u64, "ready-for-human".to_string())],
        "the gateless issue is labeled for a human"
    );
    let comments = tracker.comments.borrow();
    assert!(
        comments
            .iter()
            .any(|(n, b)| *n == 1 && b.contains("require_verify_gate")),
        "the parked issue carries the explanatory comment: {comments:?}"
    );

    // The run continued past the parked issue — no stop.
    assert!(report.stop.is_none(), "a parked issue never stops the run");
    let r1 = report.worked.iter().find(|r| r.number == 1).unwrap();
    assert_eq!(r1.outcome, Some(Outcome::Done), "the work itself finished");
    assert!(!r1.closed, "but the issue was not closed");
    let r2 = report.worked.iter().find(|r| r.number == 2).unwrap();
    assert!(r2.closed, "the gated issue closed normally");

    fs::remove_dir_all(&repo).ok();
}

#[test]
fn no_gate_without_require_flag_closes_with_warn_regression() {
    // Regression for ADR-0011: with `require_verify_gate` absent/false, a plan
    // resolving to NoGate closes exactly as before — no label, no parking.
    let repo = init_repo("no-gate-regression");
    let queue = vec![issue(1)];
    let agent = ScriptedAgent::new(vec![Outcome::Done]); // no ## Verify, no fallback
    let tracker = RecordingTracker::default();

    let report = run_queue(
        &cfg(&repo, "stamp-no-gate", false),
        &queue,
        &agent,
        &tracker,
        &ScriptedClock::never(),
    )
    .unwrap();

    let closes = tracker.closes.borrow();
    let numbers: Vec<u64> = closes.iter().map(|(n, _)| *n).collect();
    assert_eq!(
        numbers,
        vec![1],
        "NoGate + flag off still closes (warn only)"
    );
    assert!(
        tracker.labels.borrow().is_empty(),
        "no ready-for-human label without the flag"
    );
    // The close comment carries the protocol-lint result (ADR-0015).
    assert!(
        closes[0].1.contains("## Protocol lint"),
        "lint result published in the close comment: {}",
        closes[0].1
    );
    assert!(report.stop.is_none());

    fs::remove_dir_all(&repo).ok();
}

#[test]
fn protocol_lint_bounces_once_and_repaired_plan_closes_clean() {
    // ADR-0015: a protocol-dirty plan (unticked steps, missing sections) makes
    // the runner write `protocol-failure.md` and re-run the executor ONCE; the
    // well-behaved executor repairs the plan, the re-lint passes, and the issue
    // closes with an all-✓ lint block in the close comment.
    let repo = init_repo("lint-bounce");
    let queue = vec![issue(1)];
    let agent = ScriptedAgent::new(vec![Outcome::Done])
        .lint_dirty()
        .with_protocol_fix();
    let tracker = RecordingTracker::default();

    let report = run_queue(
        &cfg(&repo, "stamp-lint-bounce", false),
        &queue,
        &agent,
        &tracker,
        &ScriptedClock::never(),
    )
    .unwrap();

    assert_eq!(
        *agent.executed.borrow(),
        vec![1, 1],
        "exactly one bounce back to the executor"
    );
    let closes = tracker.closes.borrow();
    assert_eq!(closes.len(), 1, "repaired issue closed");
    let (_, comment) = &closes[0];
    assert!(
        comment.contains("## Protocol lint"),
        "lint result published: {comment}"
    );
    assert!(
        !comment.contains('\u{2717}'),
        "all checks green after the repair: {comment}"
    );
    // The bounce brief never leaks into the next issue.
    assert!(
        !repo.join(".ralphy").join("protocol-failure.md").exists(),
        "protocol-failure.md cleared after the lint settled"
    );
    assert!(report.stop.is_none());

    fs::remove_dir_all(&repo).ok();
}

#[test]
fn protocol_lint_second_violation_closes_with_report() {
    // ADR-0015: an executor that ignores the bounce brief gets no second one —
    // the issue closes anyway (today's behavior) with the ✗ report and a
    // warning in the close comment for the human reviewer.
    let repo = init_repo("lint-unrepaired");
    let queue = vec![issue(1)];
    let agent = ScriptedAgent::new(vec![Outcome::Done]).lint_dirty(); // never repairs
    let tracker = RecordingTracker::default();

    let report = run_queue(
        &cfg(&repo, "stamp-lint-unrepaired", false),
        &queue,
        &agent,
        &tracker,
        &ScriptedClock::never(),
    )
    .unwrap();

    assert_eq!(
        *agent.executed.borrow(),
        vec![1, 1],
        "one bounce only — never a second"
    );
    let closes = tracker.closes.borrow();
    assert_eq!(closes.len(), 1, "second violation still closes");
    let (_, comment) = &closes[0];
    assert!(
        comment.contains('\u{2717}') && comment.contains('\u{26a0}'),
        "close comment carries the failed checks and the warning: {comment}"
    );
    assert!(report.stop.is_none());

    fs::remove_dir_all(&repo).ok();
}
