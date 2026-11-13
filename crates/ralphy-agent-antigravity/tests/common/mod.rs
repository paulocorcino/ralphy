//! The temp repository, owned root and run dir that the tests share.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use ralphy_agent_antigravity::AntigravityAgent;
use ralphy_core::{Issue, Workspace};
use tempfile::TempDir;

/// A temp repository, the owned root and the run dir of one test.
pub(crate) struct Setup {
    _dir: TempDir,
    pub(crate) repo: PathBuf,
    pub(crate) root: PathBuf,
    pub(crate) run_dir: PathBuf,
}

impl Setup {
    pub(crate) fn new(scenario: &str) -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let repo = dir.path().join("repo");
        let root = dir.path().join("root");
        let run_dir = dir.path().join("run");
        fs::create_dir_all(&repo).expect("repo dir");
        fs::create_dir_all(&root).expect("root dir");
        let status = Command::new("git")
            .args(["init", "-q"])
            .current_dir(&repo)
            .status()
            .expect("git init");
        assert!(status.success(), "git init failed");
        fs::write(root.join("scenario"), scenario).expect("scenario");
        Self {
            _dir: dir,
            repo,
            root,
            run_dir,
        }
    }

    pub(crate) fn agent(&self, model: Option<&str>) -> AntigravityAgent {
        AntigravityAgent::new(model.map(str::to_owned), self.run_dir.clone())
            .with_program(PathBuf::from(env!("CARGO_BIN_EXE_agy_test_child")))
            .with_root(self.root.clone())
    }

    pub(crate) fn ws(&self) -> Workspace {
        Workspace::new(&self.repo)
    }

    /// The lines of the fake's order log; none before the fake wrote one.
    pub(crate) fn order(&self) -> Vec<String> {
        let log = self.root.join("order.log");
        if !log.exists() {
            return Vec::new();
        }
        read(&log).lines().map(str::to_owned).collect()
    }
}

pub(crate) fn read(path: &Path) -> String {
    fs::read_to_string(path).expect("the test reads a file the run wrote")
}

pub(crate) fn issue() -> Issue {
    serde_json::from_value(serde_json::json!({ "number": 7, "title": "a fake issue" }))
        .expect("an issue")
}
