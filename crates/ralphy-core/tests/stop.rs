//! The cooperative stop (docs/adr/0054), end to end through `run_queue`.
//!
//! **This file is its own test binary on purpose, and nothing else may join it.**
//! `ralphy_core::stop` is a PROCESS-GLOBAL flag, and `cargo test` shares one
//! process per test binary (nextest does not — both gate this repo, so the
//! weaker runner sets the rule). A stop test living in `queue.rs` would stop
//! every queue test that ran after it, and it would read as flake rather than as
//! a leak. Every test here takes [`StopFlagGuard`], which serializes them and
//! restores the flag even through a panicking assertion.

use std::cell::RefCell;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use ralphy_core::{
    run_queue, Agent, BranchMode, Execution, Issue, IssueTracker, Outcome, Plan, QueueConfig,
    RunClock, StopReason, Usage, WaitOutcome, Workspace,
};

// ---- the flag guard ---------------------------------------------------------

static STOP_FLAG_LOCK: Mutex<()> = Mutex::new(());

struct StopFlagGuard(#[allow(dead_code)] MutexGuard<'static, ()>);

impl StopFlagGuard {
    fn acquire() -> Self {
        // A panicking test poisons the lock; recover rather than cascade — the
        // flag is reset right below, so there is no state left to protect.
        let lock = STOP_FLAG_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        ralphy_core::stop::clear();
        Self(lock)
    }
}

impl Drop for StopFlagGuard {
    fn drop(&mut self) {
        ralphy_core::stop::clear();
    }
}

// ---- the fakes --------------------------------------------------------------

/// Plans a feasible, lint-clean step and executes it green. `stop_at` names the
/// issue whose `execute` raises the stop flag from INSIDE the call — which is
/// what a real stop looks like: the setter is another thread, and the run
/// discovers it partway through its work.
struct StoppingAgent {
    planned: RefCell<Vec<u64>>,
    executed: RefCell<Vec<u64>>,
    stop_at: Option<u64>,
    /// A file the executor writes into the working tree and never commits — the
    /// witness for "a stop does not destroy uncommitted work".
    litter: Option<String>,
}

impl StoppingAgent {
    fn new(stop_at: Option<u64>) -> Self {
        Self {
            planned: RefCell::new(Vec::new()),
            executed: RefCell::new(Vec::new()),
            stop_at,
            litter: None,
        }
    }

    fn littering(mut self, name: &str) -> Self {
        self.litter = Some(name.to_string());
        self
    }
}

impl Agent for StoppingAgent {
    fn name(&self) -> &'static str {
        "stopping"
    }

    fn plan(&self, issue: &Issue, ws: &Workspace) -> anyhow::Result<Plan> {
        self.planned.borrow_mut().push(issue.number);
        fs::create_dir_all(ws.ralphy_dir())?;
        let path = ws.plan_path();
        fs::write(
            &path,
            format!(
                "# Plan for #{}\n\n## Steps\n- [x] do a thing\n\
                 \n## Acceptance ledger\n\n- [verified] scripted AC \u{2014} evidence: scripted run\n\
                 \n## Handoff\n\n- **Delivered**: scripted work\n\
                 \n## Plan friction\n\n- none\n",
                issue.number
            ),
        )?;
        Ok(Plan {
            open_steps: 1,
            recommended_model: None,
            path,
            usage: Usage::default(),
            session_id: None,
        })
    }

    fn execute(&self, _plan: &Plan, ws: &Workspace) -> anyhow::Result<Execution> {
        let number = *self.planned.borrow().last().unwrap();
        self.executed.borrow_mut().push(number);
        // One real commit per issue, so the run branch is non-empty and the
        // teardown has something to hand back.
        let file = ws.repo_root().join(format!("work-{number}.txt"));
        fs::write(&file, "work\n")?;
        git(ws.repo_root(), &["add", "."]);
        git(
            ws.repo_root(),
            &["commit", "-q", "-m", &format!("work #{number}")],
        );
        // …and, when asked, one file left DIRTY in the tree. Written after the
        // commit so nothing sweeps it up.
        if let Some(name) = &self.litter {
            fs::write(ws.repo_root().join(name), "uncommitted\n")?;
        }
        // The stop lands mid-issue, exactly as the delivery worker would raise it.
        if self.stop_at == Some(number) {
            ralphy_core::stop::request();
        }
        Ok(Execution {
            outcome: Outcome::Done,
            usage: Usage::default(),
            session_id: None,
        })
    }
}

struct SilentTracker;

impl IssueTracker for SilentTracker {
    fn close(&self, _number: u64, _comment: &str) -> anyhow::Result<()> {
        Ok(())
    }
}

/// Never expires, never sleeps — so nothing in these tests can stop a run except
/// the flag under test.
struct FreeClock;

impl RunClock for FreeClock {
    fn deadline_passed(&self) -> bool {
        false
    }
    fn wait_for_reset(&self, _reset: &str) -> WaitOutcome {
        WaitOutcome::Resumed
    }
}

// ---- repo scaffolding -------------------------------------------------------

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
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn init_repo(name: &str) -> PathBuf {
    // The ledger writes under `RALPHY_USAGE_DIR`; point it at a throwaway so the
    // tests never touch the developer's real usage store.
    static USAGE: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    let usage = USAGE.get_or_init(|| {
        let dir = std::env::temp_dir().join(format!("ralphy-stop-usage-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        std::env::set_var("RALPHY_USAGE_DIR", &dir);
        dir
    });
    let _ = usage;

    static N: AtomicU32 = AtomicU32::new(0);
    let dir = std::env::temp_dir().join(format!(
        "ralphy-stop-{}-{}-{}",
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

fn cfg(repo: &Path, stamp: &str) -> QueueConfig {
    QueueConfig {
        repo_root: repo.to_path_buf(),
        base_branch: "main".into(),
        dry_run: false,
        stamp: stamp.into(),
        branch_mode: BranchMode::New,
        forced_issues: Vec::new(),
        stop_on_limit_plan: false,
        stop_on_limit_exec: false,
        verify_fallback: None,
        verify_timeout: Duration::from_secs(60),
        require_verify_gate: false,
        done_signal: "DONE_TOKEN".into(),
        human_return_labels: Vec::new(),
    }
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

// ---- the tests --------------------------------------------------------------

/// A stop raised while #1 is executing is caught the moment the execute returns
/// — BEFORE the protocol lint and the verify gate, which is where the minutes
/// are. #2 is never planned and never executed, and the stop names the issue
/// that was in flight so the panel can say what happened to it.
#[test]
fn a_stop_during_an_issue_halts_before_the_gates_and_names_that_issue() {
    let _flag = StopFlagGuard::acquire();
    let repo = init_repo("halts");
    let agent = StoppingAgent::new(Some(1));

    let report = run_queue(
        &cfg(&repo, "20260728-000000"),
        &[issue(1), issue(2), issue(3)],
        &agent,
        &SilentTracker,
        &FreeClock,
    )
    .expect("the run completes its unwind");

    assert_eq!(
        *agent.executed.borrow(),
        vec![1],
        "issues past the stop must never be executed"
    );
    assert_eq!(*agent.planned.borrow(), vec![1], "…nor even planned");
    assert!(
        matches!(report.stop, Some(StopReason::Stopped { number: Some(1) })),
        "expected a stop naming #1, got {:?}",
        report.stop
    );
    // Worked but NOT closed: its commits are on the branch, but no gate ever
    // verified them, so the run must not vouch for the issue by closing it.
    assert_eq!(report.worked.len(), 1);
    assert!(!report.worked[0].closed, "a stopped issue stays open");
}

/// The other gate: a stop already standing when the loop begins is seen at the
/// TOP of the iteration, so nothing is planned at all and no issue is named.
#[test]
fn a_stop_standing_before_the_first_issue_names_no_issue() {
    let _flag = StopFlagGuard::acquire();
    let repo = init_repo("between");
    let agent = StoppingAgent::new(None);
    ralphy_core::stop::request();

    let report = run_queue(
        &cfg(&repo, "20260728-000003"),
        &[issue(1), issue(2)],
        &agent,
        &SilentTracker,
        &FreeClock,
    )
    .expect("the run completes its unwind");

    assert!(
        agent.planned.borrow().is_empty(),
        "the top-of-iteration gate runs before `plan`"
    );
    assert!(
        matches!(report.stop, Some(StopReason::Stopped { number: None })),
        "expected a between-issues stop, got {:?}",
        report.stop
    );
    assert!(report.worked.is_empty());
}

/// The negative control. Without it, a gate accidentally inverted into
/// `if !stop::requested()` would stop every run at its first issue and the test
/// above would still be green.
#[test]
fn no_stop_request_works_the_whole_queue() {
    let _flag = StopFlagGuard::acquire();
    let repo = init_repo("nostop");
    let agent = StoppingAgent::new(None);

    let report = run_queue(
        &cfg(&repo, "20260728-000001"),
        &[issue(1), issue(2)],
        &agent,
        &SilentTracker,
        &FreeClock,
    )
    .expect("the run completes");

    assert_eq!(*agent.executed.borrow(), vec![1, 2]);
    assert!(
        report.stop.is_none(),
        "an unstopped queue must report no stop reason, got {:?}",
        report.stop
    );
}

/// **The load-bearing test of docs/adr/0054.**
///
/// The runner's teardown hands the branch back for every `Some(_)` stop and
/// force-checks-out the ORIGINAL branch when `stop` is `None` — and
/// `git checkout -f` destroys uncommitted tracked changes silently. So a stop
/// modelled as anything other than a `StopReason` (an early return, a bool, a
/// "queue exhausted") would delete the work the run had in flight.
///
/// This asserts the consequence rather than the implementation: the repo is
/// still on the run branch, and an uncommitted file written during the stopped
/// issue is still there. Rewrite the stop as a non-`StopReason` and this reds.
#[test]
fn a_stop_leaves_the_run_branch_checked_out_with_uncommitted_work_intact() {
    let _flag = StopFlagGuard::acquire();
    let repo = init_repo("keeps");
    let agent = StoppingAgent::new(Some(1)).littering("scratch.txt");

    let report = run_queue(
        &cfg(&repo, "20260728-000002"),
        &[issue(1), issue(2)],
        &agent,
        &SilentTracker,
        &FreeClock,
    )
    .expect("the run completes its unwind");

    assert_eq!(
        current_branch(&repo),
        report.branch,
        "a stopped run stays on its own branch — it does not check `main` back out"
    );
    assert_ne!(
        report.branch, report.orig_branch,
        "sanity: they differ here"
    );
    let scratch = repo.join("scratch.txt");
    assert!(
        scratch.exists(),
        "the uncommitted file written during the stopped issue was destroyed"
    );
    assert_eq!(fs::read_to_string(&scratch).unwrap(), "uncommitted\n");
}

/// The verify gate is the run's OTHER long child — a real suite here is minutes.
/// Without a stop check in its 50 ms poll loop the operator's stop would sit
/// unheard until `verify_timeout` elapsed, which is exactly the wait a Stop
/// button exists to remove.
#[test]
fn a_stop_cuts_a_running_verify_gate() {
    let _flag = StopFlagGuard::acquire();
    let repo = init_repo("verify");
    let cmd = vec![
        env!("CARGO_BIN_EXE_verify_test_child").to_string(),
        "sleep".to_string(),
    ];

    // Pre-set, so the assertion is about the check existing rather than about
    // racing a 60s child with a timer.
    ralphy_core::stop::request();
    let started = std::time::Instant::now();
    let report = ralphy_core::verify::run(&[cmd], &repo, Duration::from_secs(120));
    let elapsed = started.elapsed();

    assert!(
        elapsed < Duration::from_secs(30),
        "the gate must reap on the stop, not wait out its 120s budget (took {elapsed:?})"
    );
    assert!(!report.passed, "a gate that never finished did not pass");
}

/// `wait_for_reset` is the one wait with no ceiling — a reset days out is
/// honoured on purpose (ADR-0030). Before docs/adr/0054 its own doc said it was
/// "bounded only by the run deadline and Ctrl-C", which made it the single place
/// a run could ignore a stop indefinitely.
///
/// A far-future reset with the flag already up must return AT ONCE: a sleeping
/// loop would take a second per tick, so the elapsed-time assertion is what
/// proves the check sits at the TOP of the loop rather than after the sleep.
#[test]
fn wait_for_reset_returns_at_once_on_a_stop_request() {
    let _flag = StopFlagGuard::acquire();
    let clock = ralphy_core::WallClock { deadline: None };
    let far_future = (chrono::Local::now() + chrono::Duration::days(3)).to_rfc3339();

    ralphy_core::stop::request();
    let started = std::time::Instant::now();
    let outcome = clock.wait_for_reset(&far_future);
    let elapsed = started.elapsed();

    assert_eq!(
        outcome,
        WaitOutcome::DeadlinePassed,
        "a stopped wait ends the run rather than resuming it"
    );
    assert!(
        elapsed < Duration::from_millis(900),
        "the check must precede the 1s sleep, not follow it (took {elapsed:?})"
    );
}

/// The inert half. An inverted check would cut every verify gate in the
/// workspace short and still leave the test above green.
#[test]
fn without_a_stop_the_verify_gate_runs_its_command_to_completion() {
    let _flag = StopFlagGuard::acquire();
    let repo = init_repo("verify-clean");
    let cmd = vec![
        env!("CARGO_BIN_EXE_verify_test_child").to_string(),
        "exit-leaking-grandchild".to_string(),
    ];

    let report = ralphy_core::verify::run(&[cmd], &repo, Duration::from_secs(120));
    assert!(
        report.passed,
        "an unstopped gate runs its command normally: {:?}",
        report.commands
    );
}
