//! The scratch repository and the queue config of the stop tests.

use super::*;

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
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

/// Scratch directories under `prefix` that are more than an hour old. A test
/// that passes removes its own repo, but a failed one keeps it for a look, and
/// the per-process usage directory cannot go while that process may still run
/// tests (`cargo test` runs them all in one). An hour is far longer than any
/// test process, so what is that old belongs to no process still running.
pub(crate) fn sweep_stale(prefix: &str) {
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
            .is_some_and(|age| age > Duration::from_secs(3600));
        if old && entry.path().is_dir() {
            remove_scratch(&entry.path());
        }
    }
}

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

pub(crate) fn init_repo(name: &str) -> PathBuf {
    // The ledger writes under `RALPHY_USAGE_DIR`; point it at a throwaway so the
    // tests never touch the developer's real usage store.
    static USAGE: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    let usage = USAGE.get_or_init(|| {
        sweep_stale("ralphy-stop-");
        let dir = std::env::temp_dir().join(format!("ralphy-stop-usage-{}", std::process::id()));
        fs::create_dir_all(&dir).expect("create the usage dir");
        std::env::set_var("RALPHY_USAGE_DIR", &dir);
        dir
    });
    let _: &PathBuf = usage;

    static N: AtomicU32 = AtomicU32::new(0);
    let dir = std::env::temp_dir().join(format!(
        "ralphy-stop-{}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed),
        name
    ));
    // The name is not unique across runs: nextest gives every test its own
    // process, so `N` is 0 each time, and Windows reuses process ids within
    // minutes. A leftover repo under the same name made `git init` a re-init
    // and the commit below fail with "nothing to commit" (measured 6 in 25
    // runs of this binary, 2026-09-26).
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

pub(crate) fn cfg(repo: &Path, stamp: &str) -> QueueConfig {
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

pub(crate) fn issue(number: u64) -> Issue {
    Issue {
        number,
        title: format!("issue {number}"),
        body: String::new(),
        labels: vec![],
        comments: vec![],
    }
}
