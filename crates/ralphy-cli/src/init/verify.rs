use std::path::Path;

use anyhow::{bail, Context, Result};
use console::Style;
use ralphy_core::{git, github};

use super::gate::Agent;
use super::render::{ask_yes_no, forced, print_note, print_ok, print_section, qa_color};
use super::skills::skills_target;
use super::wizard::InitConfig;

/// A gathered snapshot of what `ralphy init` produced: which artifacts exist,
/// how many labels and skills were installed, what issues are queued, and who is
/// logged in.
pub struct VerifyReport {
    pub ralphy_present: bool,
    pub docs: Vec<(&'static str, bool)>,
    pub ralphy_label_count: usize,
    pub skill_count: usize,
    pub queue: Vec<u64>,
    pub branch: String,
    pub logged_in: Vec<String>,
}

/// The lowest-numbered queue issue is the suggested next run target (the queue
/// from `github::build_queue` is ascending by number). Returns `None` when the
/// queue is empty.
pub fn suggested_issue(queue: &[u64]) -> Option<u64> {
    queue.first().copied()
}

/// The exact `ralphy run` command the dev should run next for issue `n`.
fn next_step_command(n: u64) -> String {
    format!("ralphy run --only-issue {n} --dry-run")
}

/// Returns `true` only for an explicit `y`/`yes` (case-insensitive, trimmed).
/// Any other answer — including silence — declines, making the smoke test
/// opt-in rather than automatic.
pub fn smoke_test_decision(answer: &str) -> bool {
    matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes")
}

/// Returns the relative paths of required artifacts that are missing from the
/// report. The order is stable: `.ralphy/` first, then the doc files in order.
pub fn required_artifacts_missing(r: &VerifyReport) -> Vec<String> {
    let mut missing = Vec::new();
    if !r.ralphy_present {
        missing.push(".ralphy/".to_string());
    }

    for (path, present) in &r.docs {
        if !present {
            missing.push(path.to_string());
        }
    }

    missing
}

/// Render a human-readable final report from a gathered [`VerifyReport`]. When
/// the queue is empty the literal `warning: no queue-labeled issue` is emitted
/// and no next-step line is included; otherwise the `next step:` line names the
/// lowest queue number.
pub fn format_final_report(r: &VerifyReport) -> String {
    let mut out = String::new();
    out.push_str(&format!("agents logged in: {}\n", r.logged_in.join(", ")));
    out.push_str(&format!("branch: {}\n", r.branch));

    let ralphy_status = if r.ralphy_present {
        "present"
    } else {
        "MISSING"
    };
    out.push_str(&format!(".ralphy/:              {ralphy_status}\n"));
    for (path, present) in &r.docs {
        let status = if *present { "present" } else { "MISSING" };
        out.push_str(&format!("{path}:  {status}\n"));
    }

    out.push_str(&format!(
        "labels: {}  skills: {}  queue issues: {}\n",
        r.ralphy_label_count,
        r.skill_count,
        r.queue.len()
    ));

    if r.queue.is_empty() {
        out.push_str("warning: no queue-labeled issue\n");
    } else {
        let n = suggested_issue(&r.queue).unwrap();
        out.push_str(&format!("next step: {}\n", next_step_command(n)));
    }

    out
}

/// Print the final verify report as a styled checklist (green ✓ / red ✗ for the
/// required artifacts, dim summary lines, a highlighted next step) on a colour
/// TTY, or the plain [`format_final_report`] text otherwise.
fn print_final_report(r: &VerifyReport) {
    if !qa_color() {
        print!("{}", format_final_report(r));
        return;
    }

    let mark = |ok: bool| {
        if ok {
            forced(Style::new().green().bold())
                .apply_to("✓")
                .to_string()
        } else {
            forced(Style::new().red().bold()).apply_to("✗").to_string()
        }
    };
    print_section("Setup complete", None);
    println!("  {} .ralphy/", mark(r.ralphy_present));
    for (path, present) in &r.docs {
        println!("  {} {path}", mark(*present));
    }

    print_note(&format!("branch: {}", r.branch));
    print_note(&format!("agents: {}", r.logged_in.join(", ")));
    print_note(&format!(
        "labels: {} · skills: {} · queued issues: {}",
        r.ralphy_label_count,
        r.skill_count,
        r.queue.len()
    ));
    if r.queue.is_empty() {
        print_note("note: no issue is queued for the agent yet");
    } else {
        let n = suggested_issue(&r.queue).unwrap();
        print_ok(&format!("Next: {}", next_step_command(n)));
    }
}

/// Spawn the current binary as `ralphy run --repo <repo> --only-issue <n>
/// --dry-run`, inheriting stdio. A non-zero exit is surfaced as a warning line
/// but does NOT fail `finalize` — the smoke test is diagnostic.
fn run_smoke_test(repo: &Path, n: u64) -> Result<()> {
    let exe = std::env::current_exe().context("resolving current exe for smoke test")?;
    let repo_str = repo.display().to_string();
    let status = std::process::Command::new(&exe)
        .args([
            "run",
            "--repo",
            &repo_str,
            "--only-issue",
            &n.to_string(),
            "--dry-run",
        ])
        .status()
        .with_context(|| format!("spawning smoke test: {}", exe.display()))?;
    if !status.success() {
        println!("warning: smoke test exited with status {status} — inspect the output above");
    }

    Ok(())
}

/// Stage 10: gather a [`VerifyReport`], print the final report, bail when any
/// required artifact is missing, and — when the queue is non-empty — offer an
/// optional `--dry-run` smoke test. Called from every completion point in
/// [`run`] so the report always appears regardless of which path the dev took.
pub(crate) fn finalize(repo: &Path, cfg: &InitConfig, logged_in: &[Agent]) -> Result<()> {
    let triage_doc = std::fs::read_to_string(repo.join("docs/agents/triage-labels.md")).ok();

    let ralphy_present = repo.join(".ralphy").is_dir();
    let docs: Vec<(&'static str, bool)> = vec![
        (
            "docs/agents/issue-tracker.md",
            repo.join("docs/agents/issue-tracker.md").exists(),
        ),
        (
            "docs/agents/triage-labels.md",
            repo.join("docs/agents/triage-labels.md").exists(),
        ),
        (
            "docs/agents/domain.md",
            repo.join("docs/agents/domain.md").exists(),
        ),
    ];

    let desired_labels = github::ralphy_label_specs(triage_doc.as_deref());
    let existing_labels = github::list_repo_labels(repo)?;
    let existing_names: std::collections::HashSet<&str> =
        existing_labels.iter().map(|(n, _)| n.as_str()).collect();
    let ralphy_label_count = desired_labels
        .iter()
        .filter(|s| existing_names.contains(s.name.as_str()))
        .count();

    let skills_path = repo.join(skills_target(cfg.skills_dir.as_deref()));
    let skill_count = std::fs::read_dir(&skills_path)
        .ok()
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
                .count()
        })
        .unwrap_or(0);

    let queue_labels = github::resolve_queue_labels(&[], repo);
    // Whole-repo housekeeping — never assignee-scoped (ADR-0021).
    let queue_issues = github::list_queue(&queue_labels, None, repo)?;
    let mut queue: Vec<u64> = queue_issues.iter().map(|i| i.number).collect();
    queue.sort_unstable();

    let branch = git::current_branch(repo)?;
    let logged_in_names: Vec<String> = logged_in.iter().map(|a| a.cli_name().to_string()).collect();

    let r = VerifyReport {
        ralphy_present,
        docs,
        ralphy_label_count,
        skill_count,
        queue,
        branch,
        logged_in: logged_in_names,
    };

    print_final_report(&r);

    let missing = required_artifacts_missing(&r);
    if !missing.is_empty() {
        bail!(
            "ralphy init: repo is not ready — missing {}",
            missing.join(", ")
        );
    }

    if !r.queue.is_empty() {
        let n = suggested_issue(&r.queue).unwrap();
        let answer = ask_yes_no("Try a safe practice run now (no changes made)?", false)?;
        if smoke_test_decision(&answer) {
            run_smoke_test(repo, n)?;
        } else {
            print_note(&format!(
                "Skipped. Run it yourself anytime: {}",
                next_step_command(n)
            ));
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report_all_present(queue: Vec<u64>) -> VerifyReport {
        VerifyReport {
            ralphy_present: true,
            docs: vec![
                ("docs/agents/issue-tracker.md", true),
                ("docs/agents/triage-labels.md", true),
                ("docs/agents/domain.md", true),
            ],
            ralphy_label_count: 9,
            skill_count: 5,
            queue,
            branch: "main".into(),
            logged_in: vec!["claude".into()],
        }
    }

    #[test]
    fn format_final_report_nonempty_queue_has_next_step() {
        let r = report_all_present(vec![7, 12]);
        let output = format_final_report(&r);
        assert!(
            output.contains("ralphy run --only-issue 7 --dry-run"),
            "expected next-step command in:\n{output}"
        );
        assert!(
            !output.contains("warning:"),
            "expected no warning in:\n{output}"
        );
    }

    #[test]
    fn format_final_report_empty_queue_warns() {
        let r = report_all_present(vec![]);
        let output = format_final_report(&r);
        assert!(
            output.contains("warning: no queue-labeled issue"),
            "expected warning in:\n{output}"
        );
        assert!(
            !output.contains("ralphy run --only-issue"),
            "expected no next-step line in:\n{output}"
        );
    }

    #[test]
    fn format_final_report_marks_missing_artifact() {
        let r = VerifyReport {
            ralphy_present: true,
            docs: vec![
                ("docs/agents/issue-tracker.md", true),
                ("docs/agents/triage-labels.md", true),
                ("docs/agents/domain.md", false),
            ],
            ralphy_label_count: 0,
            skill_count: 0,
            queue: vec![],
            branch: "main".into(),
            logged_in: vec![],
        };
        let output = format_final_report(&r);
        // Find the line containing docs/agents/domain.md and assert it has MISSING.
        let domain_line = output
            .lines()
            .find(|l| l.contains("docs/agents/domain.md"))
            .expect("expected a line for docs/agents/domain.md");
        assert!(
            domain_line.contains("MISSING"),
            "expected MISSING on domain.md line:\n{domain_line}"
        );

        assert_eq!(
            required_artifacts_missing(&r),
            vec!["docs/agents/domain.md".to_string()]
        );
    }

    #[test]
    fn smoke_test_decision_default_declines() {
        assert!(!smoke_test_decision(""), "empty should decline");
        assert!(!smoke_test_decision("n"), "n should decline");
        assert!(smoke_test_decision("y"), "y should accept");
        assert!(smoke_test_decision("yes"), "yes should accept");
    }

    #[test]
    fn suggested_issue_picks_lowest() {
        assert_eq!(suggested_issue(&[7, 12]), Some(7));
        assert_eq!(suggested_issue(&[]), None);
    }
}
