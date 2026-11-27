//! The verify gate, the ledger lines a green issue writes and the protocol
//! lint.

use super::*;
use crate::verify::tokenize;

fn verify_section(line: &str) -> String {
    format!("## Verify\n\n{line}\n")
}

#[test]
fn a_resumed_plan_with_every_step_done_runs_the_gates_and_closes() {
    // A finalized plan with every step checked is finished work: the runner
    // skips the executor, runs the gates, and closes the issue. It never parks
    // it as a bundle, even though the prose says "Not a bundle".
    let root = test_dir("q-resumed-executed");
    let extra = format!(
        "## Feasible: yes\nNot a bundle: the work sits in one crate.\n\n{}",
        verify_section(verify_ok_line())
    );
    let agent = ScriptedAgent::new(vec![])
        .already_executed()
        .with_plan_extra(extra);
    let tracker = RecordingTracker::default();

    let (report, _) = run_on_fake(
        &test_cfg(&root, "stamp-resumed-executed"),
        &[test_issue(1)],
        &agent,
        &tracker,
    );

    assert!(
        report.stop.is_none(),
        "a finished plan does not stop the run"
    );
    assert!(
        agent.executed.borrow().is_empty(),
        "the executor does not run again"
    );
    assert_eq!(closed(&tracker), vec![1], "issue closed on a passing gate");
    assert!(
        tracker.labels.borrow().is_empty(),
        "no needs-split label: {:?}",
        tracker.labels.borrow()
    );

    remove_scratch(&root);
}

#[test]
fn verify_gate_fails_skips_issue_and_continues_queue() {
    // #1's gate always fails: the runner hands it back VERIFY_MAX_REPAIRS times,
    // then leaves the issue open and moves on; #2 has a passing gate.
    let root = test_dir("q-verify-fail");
    let agent = ScriptedAgent::new(vec![Outcome::Done, Outcome::Done])
        .with_plan_extra_for(1, verify_section(verify_fail_line()))
        .with_plan_extra_for(2, verify_section(verify_ok_line()));
    let tracker = RecordingTracker::default();

    let (report, _) = run_on_fake(
        &test_cfg(&root, "stamp-verify-fail"),
        &[test_issue(1), test_issue(2)],
        &agent,
        &tracker,
    );

    assert!(
        report.stop.is_none(),
        "a failed gate skips the issue but does not stop the run"
    );
    assert_eq!(
        *agent.executed.borrow(),
        vec![1, 1, 1, 2],
        "#1 repaired twice then skipped; #2 still executed"
    );
    assert_eq!(closed(&tracker), vec![2], "only the passing issue closed");
    let comments = tracker.comments.borrow();
    assert!(
        comments
            .iter()
            .any(|(n, b)| *n == 1 && b.contains("Verify gate FAILED")),
        "verify artifact comment posted on the skipped issue: {comments:?}"
    );
    let r1 = report.worked.iter().find(|r| r.number == 1).unwrap();
    assert!(!r1.closed, "gate-failed issue is not closed");
    assert!(r1.outcome.is_none(), "a verify skip carries no outcome");
    assert_eq!(r1.skip, Some(SkipReason::VerifyFailed));
    let r2 = report.worked.iter().find(|r| r.number == 2).unwrap();
    assert!(r2.closed, "the passing issue closed");

    remove_scratch(&root);
}

#[test]
fn verify_gate_repairs_then_closes() {
    // The gate fails on the first run; the repair execute writes the marker that
    // flips it green, and the issue closes without the run stopping.
    let root = test_dir("q-verify-repair");
    let agent =
        ScriptedAgent::new(vec![]).with_plan_extra(verify_section(verify_pass_after_repair_line()));
    let tracker = RecordingTracker::default();

    let (report, _) = run_on_fake(
        &test_cfg(&root, "stamp-verify-repair"),
        &[test_issue(1)],
        &agent,
        &tracker,
    );

    assert!(
        report.stop.is_none(),
        "a repaired gate does not stop the run"
    );
    assert_eq!(
        *agent.executed.borrow(),
        vec![1, 1],
        "#1 ran once + one repair that fixed the gate"
    );
    assert_eq!(
        closed(&tracker),
        vec![1],
        "issue closed after the repair went green"
    );
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

    remove_scratch(&root);
}

struct VerifySectionRow {
    name: &'static str,
    plan_extra: Option<&'static str>,
    closed: Vec<u64>,
    artifact: Option<&'static str>,
}

#[test]
fn the_verify_section_decides_the_gate() {
    // Both rows carry a failing settings fallback. `none` opts out of every
    // gate; an absent section runs the fallback, which fails and skips the issue.
    let rows = [
        VerifySectionRow {
            name: "none opts out and skips the settings fallback",
            plan_extra: Some(
                "## Verify

none
",
            ),
            closed: vec![1],
            artifact: None,
        },
        VerifySectionRow {
            name: "an absent section falls back to the settings",
            plan_extra: None,
            closed: vec![],
            artifact: Some("Verify gate FAILED"),
        },
    ];
    for row in rows {
        let root = test_dir("q-verify-section");
        let mut agent = ScriptedAgent::new(vec![Outcome::Done]);
        if let Some(extra) = row.plan_extra {
            agent = agent.with_plan_extra(extra);
        }
        let tracker = RecordingTracker::default();
        let mut config = test_cfg(&root, "stamp-verify-section");
        config.verify_fallback = Some(vec![tokenize(verify_fail_line())]);

        let (report, _) = run_on_fake(&config, &[test_issue(1)], &agent, &tracker);

        let name = row.name;
        assert!(report.stop.is_none(), "{name}: the run does not stop");
        assert_eq!(closed(&tracker), row.closed, "{name}: closed issues");
        let comments = tracker.comments.borrow();
        match row.artifact {
            Some(text) => assert!(
                comments.iter().any(|(n, b)| *n == 1 && b.contains(text)),
                "{name}: the artifact says {text:?}: {comments:?}"
            ),
            None => assert!(
                !comments.iter().any(|(_, b)| b.contains("## Verify (")),
                "{name}: no verify artifact is posted"
            ),
        }

        remove_scratch(&root);
    }
}

#[test]
fn green_issue_writes_one_plan_and_one_execute_ledger_line() {
    // One green issue appends exactly one plan line and one execute line, each
    // with the agent label, the issue number, and an outcome (ADR-0008 D6).
    let root = test_dir("q-ledger");
    let agent = ScriptedAgent::new(vec![Outcome::Done]);

    let (_, sink) = run_on_fake(
        &test_cfg(&root, "stamp-ledger"),
        &[test_issue(77)],
        &agent,
        &RecordingTracker::default(),
    );

    let lines = ledger_lines(&sink);
    let content = lines.join("\n");
    let plan_lines: Vec<&String> = lines
        .iter()
        .filter(|l| l.contains("\"phase\":\"plan\""))
        .collect();
    let exec_lines: Vec<&String> = lines
        .iter()
        .filter(|l| l.contains("\"phase\":\"execute\""))
        .collect();
    assert_eq!(plan_lines.len(), 1, "exactly one plan line: {content}");
    assert_eq!(exec_lines.len(), 1, "exactly one execute line: {content}");
    for line in [plan_lines[0], exec_lines[0]] {
        assert!(line.contains("\"agent\":\"scripted\""), "agent: {line}");
        assert!(line.contains("\"issue\":77"), "issue: {line}");
        assert!(line.contains("\"outcome\":"), "outcome: {line}");
    }
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

    remove_scratch(&root);
}

#[test]
fn require_verify_gate_parks_no_gate_issue_and_continues() {
    // ADR-0015: `require_verify_gate` + a plan with no gate: the issue is not
    // closed but labeled ready-for-human with a comment, and the run continues.
    let root = test_dir("q-require-gate");
    let agent = ScriptedAgent::new(vec![Outcome::Done, Outcome::Done])
        .with_plan_extra_for(2, verify_section(verify_ok_line()));
    let tracker = RecordingTracker::default();
    let mut config = test_cfg(&root, "stamp-require-gate");
    config.require_verify_gate = true;

    let (report, _) = run_on_fake(&config, &[test_issue(1), test_issue(2)], &agent, &tracker);

    assert_eq!(
        closed(&tracker),
        vec![2],
        "the gateless issue must not close"
    );
    assert_eq!(
        tracker.labels.borrow().as_slice(),
        &[(1u64, "ready-for-human".to_owned())],
        "the gateless issue is labeled for a human"
    );
    let comments = tracker.comments.borrow();
    assert!(
        comments
            .iter()
            .any(|(n, b)| *n == 1 && b.contains("require_verify_gate")),
        "the parked issue carries the explanatory comment: {comments:?}"
    );
    assert!(report.stop.is_none(), "a parked issue never stops the run");
    let r1 = report.worked.iter().find(|r| r.number == 1).unwrap();
    assert_eq!(r1.outcome, Some(Outcome::Done), "the work itself finished");
    assert!(!r1.closed, "but the issue was not closed");
    let r2 = report.worked.iter().find(|r| r.number == 2).unwrap();
    assert!(r2.closed, "the gated issue closed normally");

    remove_scratch(&root);
}

#[test]
fn no_gate_without_require_flag_closes_with_warn_regression() {
    // ADR-0011: without `require_verify_gate`, a plan with no gate closes as
    // before: no label, no parking.
    let root = test_dir("q-no-gate-regression");
    let agent = ScriptedAgent::new(vec![Outcome::Done]);
    let tracker = RecordingTracker::default();

    let (report, _) = run_on_fake(
        &test_cfg(&root, "stamp-no-gate"),
        &[test_issue(1)],
        &agent,
        &tracker,
    );

    assert_eq!(
        closed(&tracker),
        vec![1],
        "NoGate + flag off still closes (warn only)"
    );
    assert!(
        tracker.labels.borrow().is_empty(),
        "no ready-for-human label without the flag"
    );
    let closes = tracker.closes.borrow();
    assert!(
        closes[0].1.contains("## Protocol lint"),
        "lint result published in the close comment: {}",
        closes[0].1
    );
    assert!(report.stop.is_none());

    remove_scratch(&root);
}

#[test]
fn a_malformed_verify_section_leaves_the_issue_open_without_a_repair() {
    // A `## Verify` written as a markdown list cannot run. The settings fallback
    // passes, so a runner that read the section as absent would close the issue.
    let root = test_dir("q-verify-invalid");
    let agent = ScriptedAgent::new(vec![Outcome::Done, Outcome::Done])
        .with_plan_extra_for(1, format!("## Verify\n\n- {}\n", verify_ok_line()))
        .with_plan_extra_for(2, verify_section(verify_ok_line()));
    let tracker = RecordingTracker::default();
    let mut config = test_cfg(&root, "stamp-verify-invalid");
    config.verify_fallback = Some(vec![tokenize(verify_ok_line())]);
    config.require_verify_gate = true;

    let (report, _) = run_on_fake(&config, &[test_issue(1), test_issue(2)], &agent, &tracker);

    assert!(
        report.stop.is_none(),
        "a malformed gate does not stop the run"
    );
    assert_eq!(*agent.executed.borrow(), vec![1, 2], "no repair for #1");
    assert_eq!(closed(&tracker), vec![2], "#1 stays open");
    assert!(tracker.labels.borrow().is_empty(), "not parked for a human");
    let comments = tracker.comments.borrow();
    assert!(
        comments
            .iter()
            .any(|(n, b)| *n == 1 && b.contains("could NOT run")),
        "the malformed-section artifact is posted: {comments:?}"
    );
    let r1 = report.worked.iter().find(|r| r.number == 1).unwrap();
    assert!(!r1.closed);
    assert_eq!(r1.skip, Some(SkipReason::VerifyFailed));

    remove_scratch(&root);
}

#[test]
fn protocol_lint_bounces_once_and_repaired_plan_closes_clean() {
    // ADR-0015: a protocol-dirty plan makes the runner write `protocol-failure.md`
    // and re-run the executor once; the executor repairs the plan and the issue
    // closes with an all-pass lint block.
    let root = test_dir("q-lint-bounce");
    let agent = ScriptedAgent::new(vec![Outcome::Done])
        .lint_dirty()
        .with_protocol_fix();
    let tracker = RecordingTracker::default();

    let (report, _) = run_on_fake(
        &test_cfg(&root, "stamp-lint-bounce"),
        &[test_issue(1)],
        &agent,
        &tracker,
    );

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
    assert!(
        !root.join(".ralphy").join("protocol-failure.md").exists(),
        "protocol-failure.md cleared after the lint settled"
    );
    assert!(report.stop.is_none());

    remove_scratch(&root);
}

#[test]
fn protocol_lint_second_violation_closes_with_report() {
    // ADR-0015: an executor that ignores the bounce brief gets no second one;
    // the issue closes with the failed checks and a warning.
    let root = test_dir("q-lint-unrepaired");
    let agent = ScriptedAgent::new(vec![Outcome::Done]).lint_dirty();
    let tracker = RecordingTracker::default();

    let (report, _) = run_on_fake(
        &test_cfg(&root, "stamp-lint-unrepaired"),
        &[test_issue(1)],
        &agent,
        &tracker,
    );

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

    remove_scratch(&root);
}
