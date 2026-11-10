use super::*;
use crate::scratch::remove_scratch;
use crate::Outcome;

mod queue_run;

fn labeled(number: u64, labels: &[&str]) -> Issue {
    Issue {
        number,
        title: format!("issue {number}"),
        body: String::new(),
        labels: labels.iter().map(ToString::to_string).collect(),
        comments: Vec::new(),
    }
}

#[test]
fn first_stop_before_finds_first_and_respects_forced() {
    let queue = vec![
        labeled(1, &[]),
        labeled(2, &[STOP_BEFORE_LABEL]),
        labeled(3, &[STOP_BEFORE_LABEL]),
    ];
    // The first stop-before in order.
    assert_eq!(first_stop_before(&queue, &[]), Some(2));
    // Forcing the first stop-before skips to the next one.
    assert_eq!(first_stop_before(&queue, &[2]), Some(3));
    // Forcing every stop-before yields none.
    assert_eq!(first_stop_before(&queue, &[2, 3]), None);
    // A queue with no stop-before yields none.
    assert_eq!(first_stop_before(&[labeled(1, &[])], &[]), None);
}

#[test]
fn human_return_label_matches_first_configured_label() {
    let labels = vec!["needs-info".to_string(), "wontfix".to_string()];
    let parked = labeled(5, &["queue", "wontfix"]);
    assert_eq!(
        human_return_label(&parked, &labels).map(String::as_str),
        Some("wontfix")
    );
    let plain = labeled(6, &["queue"]);
    assert_eq!(human_return_label(&plain, &labels), None);
}

// ------------------------------------------------------------------
// Queue-loop tests through the injectable seams: `run_queue_with` driven
// by a FakeRepo and FakeLedger — no on-disk git repository, no usage
// file, no RALPHY_USAGE_DIR juggling. The workspace is a plain temp dir
// (the `.ralphy/` scratch and the verify commands only need a
// filesystem). Complements tests/queue/, which proves the same loop
// over a real repo.
// ------------------------------------------------------------------

use std::cell::RefCell;
use std::collections::VecDeque;
use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use crate::ledger::LedgerRecord;
use crate::{Execution, Plan, ResetAt};

/// Constant answers for the reads, a recorder for the checkouts. The
/// constant `head_sha` is fine here: the no-commit streak only matters on
/// limit-resumes, which these tests do not script.
struct FakeRepo {
    checkouts: RefCell<Vec<String>>,
}

impl FakeRepo {
    fn new() -> Self {
        Self {
            checkouts: RefCell::new(Vec::new()),
        }
    }
}

impl Repo for FakeRepo {
    fn current_branch(&self) -> Result<String> {
        Ok("main".into())
    }

    fn head_sha(&self) -> Result<String> {
        Ok("abc123".into())
    }

    fn project_slug(&self) -> String {
        "owner/repo".into()
    }

    fn checkout_new_branch(&self, branch: &str, base: &str) -> Result<()> {
        self.checkouts
            .borrow_mut()
            .push(format!("new:{branch}:{base}"));
        Ok(())
    }

    fn checkout_force(&self, refname: &str) -> Result<()> {
        self.checkouts.borrow_mut().push(format!("force:{refname}"));
        Ok(())
    }

    fn delete_branch(&self, branch: &str) -> Result<()> {
        self.checkouts.borrow_mut().push(format!("delete:{branch}"));
        Ok(())
    }

    fn rev_list_count(&self, _range: &str) -> Result<usize> {
        Ok(1)
    }

    fn log_oneline(&self, _range: &str) -> Result<Vec<String>> {
        Ok(vec!["abc123 work".into()])
    }

    fn user_email(&self) -> Option<String> {
        Some("t@example.com".into())
    }

    fn user_name(&self) -> Option<String> {
        Some("Test".into())
    }
}

/// Captures every ledger line in memory.
#[derive(Default)]
struct FakeLedger {
    records: RefCell<Vec<LedgerRecord>>,
}

impl LedgerSink for FakeLedger {
    fn append(&self, rec: &LedgerRecord) -> Result<()> {
        self.records.borrow_mut().push(rec.clone());
        Ok(())
    }
}

/// Plans a one-step, lint-clean plan (optionally carrying a `## Verify`
/// section, or a protocol-dirty shape) and pops a scripted outcome per
/// `execute` — never touching git.
struct MiniAgent {
    outcomes: RefCell<VecDeque<Outcome>>,
    planned: RefCell<Vec<u64>>,
    /// Appended verbatim to the plan (e.g. a `## Verify` section).
    extra: Option<String>,
    /// Write a protocol-dirty plan (unticked step, no closing sections).
    lint_dirty: bool,
    /// On `execute`, repair the plan when the ADR-0015 bounce brief is on
    /// disk: tick every step and append the closing sections.
    fix_protocol: bool,
}

impl MiniAgent {
    fn new(outcomes: Vec<Outcome>) -> Self {
        Self {
            outcomes: RefCell::new(outcomes.into()),
            planned: RefCell::new(Vec::new()),
            extra: None,
            lint_dirty: false,
            fix_protocol: false,
        }
    }

    fn with_extra(mut self, extra: impl Into<String>) -> Self {
        self.extra = Some(extra.into());
        self
    }

    fn lint_dirty_with_fix(mut self) -> Self {
        self.lint_dirty = true;
        self.fix_protocol = true;
        self
    }
}

impl Agent for MiniAgent {
    fn name(&self) -> &'static str {
        "mini"
    }

    fn plan(&self, issue: &Issue, ws: &Workspace) -> Result<Plan> {
        self.planned.borrow_mut().push(issue.number);
        fs::create_dir_all(ws.ralphy_dir())?;
        let step = if self.lint_dirty {
            "- [ ] do a thing\n"
        } else {
            "- [x] do a thing\n"
        };
        let extra = self
            .extra
            .as_deref()
            .map(|e| format!("\n{e}\n"))
            .unwrap_or_default();
        let closing = if self.lint_dirty {
            ""
        } else {
            "\n## Acceptance ledger\n\n- [verified] scripted AC \u{2014} evidence: scripted run\n\n\
                 ## Handoff\n\n- **Delivered**: scripted work\n\n## Plan friction\n\n- none\n"
        };
        let body = format!(
            "# Plan for #{}\n\n## Steps\n{step}{extra}{closing}",
            issue.number
        );
        let path = ws.plan_path();
        fs::write(&path, body)?;
        Ok(Plan {
            path,
            open_steps: 1,
            recommended_model: None,
            usage: Usage {
                output: 3,
                model: Some("fake-model".into()),
                ..Usage::default()
            },
            session_id: None,
        })
    }

    fn execute(&self, _plan: &Plan, ws: &Workspace) -> Result<Execution> {
        if self.fix_protocol && ws.ralphy_dir().join("protocol-failure.md").exists() {
            let plan_md = fs::read_to_string(ws.plan_path())?;
            let fixed = plan_md.replace("- [ ]", "- [x]")
                + "\n## Acceptance ledger\n\n- [verified] scripted AC \u{2014} \
                       evidence: scripted run\n\n\
                       ## Handoff\n\n- **Delivered**: repaired\n\n## Plan friction\n\n- none\n";
            fs::write(ws.plan_path(), fixed)?;
        }
        let outcome = self
            .outcomes
            .borrow_mut()
            .pop_front()
            .expect("more execute calls than scripted outcomes");
        Ok(Execution {
            outcome,
            usage: Usage {
                output: 5,
                model: Some("fake-model".into()),
                ..Usage::default()
            },
            session_id: None,
        })
    }
}

/// Records closes/comments/labels. Every issue reads as closed and
/// `create_issue` creates nothing; the trait's defaults cover the rest.
#[derive(Default)]
struct FakeTracker {
    closes: RefCell<Vec<u64>>,
    comments: RefCell<Vec<(u64, String)>>,
    labels: RefCell<Vec<(u64, String)>>,
}

impl IssueTracker for FakeTracker {
    fn close(&self, number: u64, _comment: &str) -> Result<()> {
        self.closes.borrow_mut().push(number);
        Ok(())
    }

    fn comment(&self, number: u64, body: &str) -> Result<()> {
        self.comments.borrow_mut().push((number, body.to_string()));
        Ok(())
    }

    fn add_label(&self, number: u64, label: &str) -> Result<()> {
        self.labels.borrow_mut().push((number, label.to_string()));
        Ok(())
    }

    fn is_closed(&self, _number: u64) -> Result<bool> {
        Ok(true)
    }

    fn create_issue(&self, _title: &str, _body: &str, _labels: &[String]) -> Result<u64> {
        Ok(0)
    }
}

/// Never expires, never sleeps.
struct FakeClock;

impl RunClock for FakeClock {
    fn deadline_passed(&self) -> bool {
        false
    }

    fn wait_for_reset(&self, _reset: &ResetAt) -> WaitOutcome {
        WaitOutcome::Resumed
    }
}

/// A fresh plain directory (no git) the workspace and verify commands run
/// in; unique per test so parallel tests never collide.
fn test_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ralphy-runner-ut-{}-{name}", std::process::id()));
    remove_scratch(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn test_cfg(root: &std::path::Path, stamp: &str) -> QueueConfig {
    QueueConfig {
        repo_root: root.to_path_buf(),
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
        human_return_labels: vec![
            "ready-for-human".into(),
            "HITL".into(),
            "needs-info".into(),
            "needs-triage".into(),
            "wontfix".into(),
            "triage-agent".into(),
        ],
    }
}

fn test_issue(number: u64) -> Issue {
    Issue {
        number,
        title: format!("issue {number}"),
        body: String::new(),
        labels: vec![],
        comments: vec![],
    }
}

/// A `## Verify` line whose command exits 0 on every platform.
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
