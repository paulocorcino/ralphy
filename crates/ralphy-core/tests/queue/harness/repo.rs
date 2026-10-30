use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use ralphy_core::{BranchMode, Issue, QueueConfig};

/// Remove a scratch directory at the end of a test, or before a test starts. The
/// directory may be gone already, and a failed removal must not fail the test.
pub(crate) fn remove_scratch(path: &Path) {
    #[expect(
        clippy::unused_result_ok,
        reason = "a scratch directory may be gone already, and a failed cleanup must not \
                 fail the test"
    )]
    fs::remove_dir_all(path).ok();
}

pub(crate) fn git(repo: &Path, args: &[&str]) {
    let status = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .status()
        .expect("spawn git");
    assert!(status.success(), "git {args:?} failed");
}

pub(crate) fn current_branch(repo: &Path) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .output()
        .expect("spawn git");
    String::from_utf8(out.stdout)
        .expect("git output was not UTF-8")
        .trim()
        .to_string()
}

pub(crate) fn branch_exists(repo: &Path, branch: &str) -> bool {
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
pub(crate) fn rev_parse(repo: &Path, refname: &str) -> Option<String> {
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
    out.status.success().then(|| {
        String::from_utf8(out.stdout)
            .expect("git output was not UTF-8")
            .trim()
            .to_string()
    })
}

/// The tracked file paths at `refname` (recursive), one per entry.
pub(crate) fn git_ls(repo: &Path, refname: &str) -> Vec<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["ls-tree", "-r", "--name-only", refname])
        .output()
        .expect("spawn git");
    String::from_utf8(out.stdout)
        .expect("git output was not UTF-8")
        .lines()
        .map(str::to_string)
        .collect()
}

/// The contents of `path` as committed at `refname` (empty if absent).
pub(crate) fn git_show(repo: &Path, refname: &str, path: &str) -> String {
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
pub(crate) fn ensure_usage_dir() -> PathBuf {
    use std::sync::OnceLock;
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        sweep_stale("ralphy-usage-test-");
        sweep_stale("ralphy-queue-");
        let dir = std::env::temp_dir().join(format!("ralphy-usage-test-{}", std::process::id()));
        remove_scratch(&dir);
        std::env::set_var("RALPHY_USAGE_DIR", &dir);
        dir
    })
    .clone()
}

/// Scratch directories under `prefix` that are more than an hour old. A test
/// that passes removes its own repo, but a failed one keeps it for a look, and
/// the per-process usage directory cannot go while that process may still run
/// tests (`cargo test` runs them all in one). An hour is far longer than any
/// test process, so what is that old belongs to no process still running.
fn sweep_stale(prefix: &str) {
    let Ok(entries) = fs::read_dir(std::env::temp_dir()) else {
        return;
    };
    for entry in entries.flatten() {
        if !entry.file_name().to_string_lossy().starts_with(prefix) {
            continue;
        }
        let old = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age > std::time::Duration::from_secs(3600));
        if old && entry.path().is_dir() {
            remove_scratch(&entry.path());
        }
    }
}

pub(crate) fn init_repo(name: &str) -> PathBuf {
    ensure_usage_dir();
    static N: AtomicU32 = AtomicU32::new(0);
    let dir = std::env::temp_dir().join(format!(
        "ralphy-queue-{}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed),
        name
    ));
    // The name is not unique across runs: nextest gives every test its own
    // process, so `N` is 0 each time, and Windows reuses process ids within
    // minutes. A leftover repo under the same name made `git init` a re-init
    // and the commit below fail with "nothing to commit" (measured 6 in 25
    // runs of tests/stop.rs, which has the same helper, 2026-09-26).
    remove_scratch(&dir);
    fs::create_dir_all(&dir).expect("create the scratch repo dir");
    git(&dir, &["init", "-q", "-b", "main"]);
    git(&dir, &["config", "user.email", "t@example.com"]);
    git(&dir, &["config", "user.name", "Test"]);
    fs::write(dir.join(".gitignore"), ".ralphy/\n").expect("write .gitignore");
    fs::write(dir.join("README.md"), "hello\n").expect("write README.md");
    git(&dir, &["add", "."]);
    git(&dir, &["commit", "-q", "-m", "init"]);
    dir
}

pub(crate) fn issue(number: u64) -> Issue {
    Issue {
        number,
        title: format!("issue {number}"),
        body: String::new(),
        labels: vec![],
        comments: vec![],
    }
}

pub(crate) fn issue_labeled(number: u64, labels: &[&str]) -> Issue {
    Issue {
        number,
        title: format!("issue {number}"),
        body: String::new(),
        labels: labels.iter().map(|s| s.to_string()).collect(),
        comments: vec![],
    }
}

pub(crate) fn issue_with_body(number: u64, body: impl Into<String>) -> Issue {
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
pub(crate) fn default_human_return() -> Vec<String> {
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

pub(crate) fn cfg(repo: &Path, stamp: &str, dry_run: bool) -> QueueConfig {
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

pub(crate) fn cfg_only(repo: &Path, stamp: &str, only: u64) -> QueueConfig {
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

pub(crate) fn cfg_forced(repo: &Path, stamp: &str, forced: Vec<u64>) -> QueueConfig {
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

pub(crate) fn cfg_current(repo: &Path, stamp: &str) -> QueueConfig {
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
pub(crate) fn cfg_stop_on_limit(repo: &Path, stamp: &str) -> QueueConfig {
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
pub(crate) fn cfg_split_limit(repo: &Path, stamp: &str) -> QueueConfig {
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

/// A `## Verify` line whose command exits 0 on every platform (argv-only, no
/// shell metacharacters that a non-shell argv split would mangle).
pub(crate) fn verify_ok_line() -> &'static str {
    if cfg!(windows) {
        "cmd /c \"exit 0\""
    } else {
        "sh -c \"exit 0\""
    }
}

/// A `## Verify` line whose command exits non-zero on every platform.
pub(crate) fn verify_fail_line() -> &'static str {
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
pub(crate) fn verify_pass_after_repair_line() -> &'static str {
    if cfg!(windows) {
        "cmd /c \"if exist issue-1-1.txt (exit 0) else (exit 3)\""
    } else {
        "sh -c \"test -f issue-1-1.txt\""
    }
}
