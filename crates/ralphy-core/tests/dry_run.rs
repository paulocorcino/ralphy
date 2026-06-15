//! End-to-end lifecycle tests over a throwaway git repo, with a `FakeAgent`
//! standing in for the Claude adapter. These cover the acceptance criteria the
//! core owns: a clean dry run restores the original branch and drops the empty
//! run branch, and the run aborts cleanly on a dirty tree or a missing base.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};

use ralphy_core::{
    run, Agent, BranchMode, Execution, Issue, Outcome, Plan, RunConfig, RunOutcome, Usage,
    Workspace,
};

/// Writes a plan with `steps` open items; never touches git, so a dry run stays
/// empty and the branch is dropped on restore.
struct FakeAgent {
    steps: usize,
}

impl Agent for FakeAgent {
    fn name(&self) -> &'static str {
        "fake"
    }

    fn plan(&self, _issue: &Issue, ws: &Workspace) -> anyhow::Result<Plan> {
        fs::create_dir_all(ws.ralphy_dir())?;
        let path = ws.plan_path();
        let body = format!(
            "# Plan for #1\n\n## Execution model: sonnet\n\n## Steps\n{}",
            "- [ ] do a thing\n".repeat(self.steps)
        );
        fs::write(&path, body)?;
        Ok(Plan {
            open_steps: self.steps,
            recommended_model: Some("sonnet".into()),
            path,
            usage: Usage::default(),
        })
    }

    fn execute(&self, _plan: &Plan, _ws: &Workspace) -> anyhow::Result<Execution> {
        Ok(Execution {
            outcome: Outcome::Done,
            usage: Usage::default(),
        })
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
        "ralphy-it-{}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed),
        name
    ));
    fs::create_dir_all(&dir).unwrap();
    git(&dir, &["init", "-q", "-b", "main"]);
    git(&dir, &["config", "user.email", "t@example.com"]);
    git(&dir, &["config", "user.name", "Test"]);
    // .gitignore already ignores .ralphy/ so the auto-edit is a no-op and the
    // tree stays clean for the happy-path test.
    fs::write(dir.join(".gitignore"), ".ralphy/\n").unwrap();
    fs::write(dir.join("README.md"), "hello\n").unwrap();
    git(&dir, &["add", "."]);
    git(&dir, &["commit", "-q", "-m", "init"]);
    dir
}

fn issue() -> Issue {
    Issue {
        number: 1,
        title: "walking skeleton".into(),
        body: "body".into(),
        labels: vec![],
    }
}

fn cfg(repo: &Path, base: &str, stamp: &str) -> RunConfig {
    RunConfig {
        repo_root: repo.to_path_buf(),
        base_branch: base.into(),
        dry_run: true,
        stamp: stamp.into(),
        branch_mode: BranchMode::New,
    }
}

#[test]
fn dry_run_restores_branch_and_drops_empty() {
    let repo = init_repo("happy");
    let report = run(
        &cfg(&repo, "main", "stamp1"),
        &issue(),
        &FakeAgent { steps: 2 },
    )
    .unwrap();

    assert!(repo.join(".ralphy/plan.md").exists(), "plan written");
    match report.outcome {
        RunOutcome::DryRun { open_steps } => assert_eq!(open_steps, 2),
        other => panic!("expected DryRun, got {other:?}"),
    }
    assert_eq!(current_branch(&repo), "main", "returned to original branch");
    assert!(
        !branch_exists(&repo, "afk/run-stamp1"),
        "empty run branch should be deleted"
    );

    fs::remove_dir_all(&repo).ok();
}

#[test]
fn aborts_when_tree_is_dirty() {
    let repo = init_repo("dirty");
    fs::write(repo.join("uncommitted.txt"), "x").unwrap();

    let err = run(
        &cfg(&repo, "main", "stamp2"),
        &issue(),
        &FakeAgent { steps: 1 },
    )
    .unwrap_err();
    assert!(err.to_string().contains("not clean"), "got: {err}");
    assert_eq!(current_branch(&repo), "main", "left where it started");

    fs::remove_dir_all(&repo).ok();
}

#[test]
fn aborts_when_base_is_missing() {
    let repo = init_repo("nobase");

    let err = run(
        &cfg(&repo, "origin/does-not-exist", "stamp3"),
        &issue(),
        &FakeAgent { steps: 1 },
    )
    .unwrap_err();
    assert!(err.to_string().contains("not found"), "got: {err}");
    assert_eq!(current_branch(&repo), "main", "left where it started");

    fs::remove_dir_all(&repo).ok();
}
