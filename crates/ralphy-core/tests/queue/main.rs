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

// One test binary (`queue`), one module per queue behaviour: the helpers above
// this line are what every module shares through `use super::*`.
mod blockers;
mod branches;
mod close_artifacts;
mod events;
mod lifecycle;
mod limits;
mod selection;
mod verify;

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
    /// When true, `plan` omits the default `## Acceptance ledger` section
    /// (ADR-0015) that a lint-clean plan otherwise carries — used to test the
    /// lint's ledger-presence check in isolation.
    omit_ledger: bool,
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
            omit_ledger: false,
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

    /// Omit the default `## Acceptance ledger` section from an otherwise
    /// lint-clean plan, so the ledger-presence check alone bounces it.
    fn without_ledger(mut self) -> Self {
        self.omit_ledger = true;
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
            if !self.omit_ledger && !body.contains("## Acceptance ledger") {
                body.push_str(
                    "\n## Acceptance ledger\n\n- [verified] scripted AC \u{2014} evidence: scripted run\n",
                );
            }
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
                + "\n## Acceptance ledger\n\n- [verified] scripted AC \u{2014} evidence: scripted run\n\n\
                   ## Handoff\n\n- **Delivered**: repaired\n\n## Plan friction\n\n- none\n";
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
    /// Every `add_label` call (`needs-split` on a bundle verdict,
    /// `needs-human-review` on a close carrying review-only ledger lines).
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
