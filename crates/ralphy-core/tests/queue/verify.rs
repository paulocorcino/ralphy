//! The verify gate, the ledger lines a green issue writes and the protocol
//! lint.

use super::*;

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
