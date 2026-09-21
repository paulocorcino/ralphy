//! What a close leaves behind: evidence, review-only labels, handoff/friction
//! and knowledge comments, infeasible-plan reasoning, planner inputs.

use super::*;

#[test]
fn green_close_calls_write_evidence_with_parsed_verdicts() {
    let repo = init_repo("evidence-write");
    // The ledger contains one verified criterion: the runner must call
    // write_evidence with it after the green close.
    let ledger = "- [verified] Some AC — evidence: unit test proves it\n";
    let queue = vec![issue(1)];
    let agent = ScriptedAgent::new(vec![Outcome::Done]).with_ledger(ledger);
    let tracker = RecordingTracker::default();

    run_queue(
        &cfg(&repo, "stamp-evidence", false),
        &queue,
        &agent,
        &tracker,
        &ScriptedClock::never(),
    )
    .unwrap();

    let writes = tracker.evidence_writes.borrow();
    assert_eq!(
        writes.len(),
        1,
        "write_evidence called once for the green issue"
    );
    let (number, verdicts) = &writes[0];
    assert_eq!(*number, 1);
    assert_eq!(verdicts.len(), 1);
    assert_eq!(verdicts[0].criterion, "Some AC");
    assert_eq!(verdicts[0].kind, ralphy_core::VerdictKind::Verified);
    assert_eq!(verdicts[0].evidence, "unit test proves it");

    fs::remove_dir_all(&repo).ok();
}

#[test]
fn green_close_records_review_only_count_and_labels_the_issue() {
    let repo = init_repo("review-only-label");
    let ledger = "- [verified] Some AC — evidence: unit test proves it\n\
                  - [review-only] CONTEXT.md reads correctly — evidence: human reads the prose\n\
                  - [review-only] ADR-0037 amended — evidence: human reads the prose\n";
    let queue = vec![issue(1)];
    let agent = ScriptedAgent::new(vec![Outcome::Done]).with_ledger(ledger);
    let tracker = RecordingTracker::default();

    let report = run_queue(
        &cfg(&repo, "stamp-review-only", false),
        &queue,
        &agent,
        &tracker,
        &ScriptedClock::never(),
    )
    .unwrap();

    assert_eq!(
        report.worked[0].review_only, 2,
        "both review-only ledger lines counted on the closed issue"
    );
    // The EXACT set, not a `contains`: `ready-for-human`/`HITL` are excluded by
    // the same assertion that pins the one label the close is allowed to apply
    // (ADR-0014 — review debt must never re-enter the human-blocker path).
    assert_eq!(
        tracker.labels.borrow().as_slice(),
        [(1, "needs-human-review".to_string())]
    );

    fs::remove_dir_all(&repo).ok();
}

#[test]
fn green_close_with_no_review_only_lines_adds_no_label() {
    let repo = init_repo("review-only-none");
    let ledger = "- [verified] Some AC — evidence: unit test proves it\n";
    let queue = vec![issue(1)];
    let agent = ScriptedAgent::new(vec![Outcome::Done]).with_ledger(ledger);
    let tracker = RecordingTracker::default();

    let report = run_queue(
        &cfg(&repo, "stamp-review-none", false),
        &queue,
        &agent,
        &tracker,
        &ScriptedClock::never(),
    )
    .unwrap();

    // The close must actually have happened, or `review_only == 0` is vacuous:
    // ten non-close paths hardcode the field to `0`.
    assert_eq!(tracker.closes.borrow().len(), 1, "the issue closed");
    assert_eq!(report.worked[0].review_only, 0);
    assert!(
        tracker.labels.borrow().is_empty(),
        "a fully verified ledger applies no label: {:?}",
        tracker.labels.borrow()
    );

    fs::remove_dir_all(&repo).ok();
}

#[test]
fn ledgerless_plan_bounces_and_writes_no_evidence() {
    let repo = init_repo("evidence-noop");
    // Otherwise lint-clean plan, but no `## Acceptance ledger` section — the
    // ADR-0015 lint's ledger-presence check ALONE bounces it once (ADR-0015),
    // and write_evidence is never called on the eventual close.
    let queue = vec![issue(2)];
    let agent = ScriptedAgent::new(vec![Outcome::Done, Outcome::Done]).without_ledger();
    let tracker = RecordingTracker::default();

    run_queue(
        &cfg(&repo, "stamp-noop", false),
        &queue,
        &agent,
        &tracker,
        &ScriptedClock::never(),
    )
    .unwrap();

    // One protocol bounce: plan + two executes (the lint fails after the
    // first execute, the runner hands the session back once).
    assert_eq!(*agent.executed.borrow(), vec![2, 2]);
    assert_eq!(tracker.closes.borrow().len(), 1, "issue still closed");
    let (_, comment) = &tracker.closes.borrow()[0];
    assert!(
        comment.contains("\u{2717} ## Acceptance ledger present"),
        "close comment must report the failed ledger check: {comment}"
    );
    assert!(
        tracker.evidence_writes.borrow().is_empty(),
        "no evidence-write when ledger is absent"
    );

    fs::remove_dir_all(&repo).ok();
}

#[test]
fn green_close_posts_handoff_and_friction_comment() {
    let repo = init_repo("handoff-close");
    let extra = "## Handoff\n\n- **Delivered**: lab fixtures (abc1234)\n- **Residue**: Setup-Lab.ps1 never ran clean-slate\n\n## Plan friction\n\n- the plan treated the lab as a given precondition";
    let queue = vec![issue(1)];
    let agent = ScriptedAgent::new(vec![Outcome::Done]).with_plan_extra(extra);
    let tracker = RecordingTracker::default();

    run_queue(
        &cfg(&repo, "stamp-handoff", false),
        &queue,
        &agent,
        &tracker,
        &ScriptedClock::never(),
    )
    .unwrap();

    let comments = tracker.comments.borrow();
    assert_eq!(comments.len(), 1, "one handoff comment for the green issue");
    let (number, body) = &comments[0];
    assert_eq!(*number, 1);
    assert!(body.contains("## Handoff"), "comment carries the handoff");
    assert!(
        body.contains("never ran clean-slate"),
        "residue reaches the issue"
    );
    assert!(
        body.contains("## Plan friction") && body.contains("given precondition"),
        "friction reaches the issue"
    );
    // A handoff without a `Knowledge used` block warns but records nothing.
    assert!(
        !Workspace::new(&repo).citations_path().exists(),
        "no citations.jsonl entry when the field is absent"
    );

    fs::remove_dir_all(&repo).ok();
}

#[test]
fn green_close_appends_knowledge_used_citations() {
    let repo = init_repo("citations-close");
    let cited = "## Handoff\n\n- **Delivered**: fix (abc1234)\n- **Knowledge used**:\n  - \"Toolchain & platform\" — cargo test needs docker up first\n  - handoffs.md #5: schema rejects empty DEVICEID\n\n## Plan friction\n\n- none";
    let none = "## Handoff\n\n- **Delivered**: docs (def5678)\n- **Knowledge used**: none\n\n## Plan friction\n\n- none";
    let queue = vec![issue(1), issue(2)];
    let agent = ScriptedAgent::new(vec![Outcome::Done, Outcome::Done])
        .with_plan_extra_for(1, cited)
        .with_plan_extra_for(2, none);
    let tracker = RecordingTracker::default();

    run_queue(
        &cfg(&repo, "stamp-citations", false),
        &queue,
        &agent,
        &tracker,
        &ScriptedClock::never(),
    )
    .unwrap();

    // One JSON line per green close, in queue order — the hit-rate log the
    // consolidation curator prunes against.
    let content = fs::read_to_string(Workspace::new(&repo).citations_path())
        .expect("citations.jsonl written");
    let entries: Vec<ralphy_core::knowledge::CitationEntry> = content
        .lines()
        .map(|l| serde_json::from_str(l).expect("each line is one CitationEntry"))
        .collect();
    assert_eq!(entries.len(), 2, "one entry per green close: {content}");
    assert_eq!(entries[0].issue, 1);
    assert_eq!(entries[0].stamp, "stamp-citations");
    assert_eq!(
        entries[0].citations,
        vec![
            "\"Toolchain & platform\" — cargo test needs docker up first".to_string(),
            "handoffs.md #5: schema rejects empty DEVICEID".to_string(),
        ]
    );
    assert_eq!(entries[1].issue, 2);
    assert_eq!(
        entries[1].citations,
        Vec::<String>::new(),
        "an honest `none` is recorded as an empty list"
    );

    fs::remove_dir_all(&repo).ok();
}

#[test]
fn infeasible_plan_posts_planner_reasoning_comment() {
    let repo = init_repo("infeasible-comment");
    let extra =
        "## Feasible: no\nThe issue bundles six PRD breakdown tasks; split into W1-T01..T06.";
    let queue = vec![issue(3)];
    let agent = ScriptedAgent::new(vec![])
        .infeasible()
        .with_plan_extra(extra);
    let tracker = RecordingTracker::default();

    let report = run_queue(
        &cfg(&repo, "stamp-infeasible", false),
        &queue,
        &agent,
        &tracker,
        &ScriptedClock::never(),
    )
    .unwrap();

    // The skip stays a skip: not closed, not executed, run continues.
    assert!(
        tracker.closes.borrow().is_empty(),
        "infeasible never closes"
    );
    assert!(
        agent.executed.borrow().is_empty(),
        "infeasible never executes"
    );
    assert!(report.stop.is_none(), "infeasible does not stop the run");

    // But the verdict is no longer silent: the reasoning lands on the issue.
    // This reason carries the word "bundle", so it routes to the bundle path:
    // the needs-split label is applied and the comment names the human step.
    let comments = tracker.comments.borrow();
    assert_eq!(comments.len(), 1, "one skip comment");
    let (number, body) = &comments[0];
    assert_eq!(*number, 3);
    assert!(
        body.contains("bundles six PRD breakdown tasks"),
        "planner reasoning reaches the issue: {body}"
    );
    assert!(
        body.contains("/to-issues"),
        "bundle comment names the human split step: {body}"
    );
    let labels = tracker.labels.borrow();
    assert_eq!(
        labels.as_slice(),
        &[(3u64, "needs-split".to_string())],
        "bundle verdict applies the needs-split label"
    );

    fs::remove_dir_all(&repo).ok();
}

#[test]
fn infeasible_plan_without_bundle_verdict_stays_generic() {
    // A reason without the word "bundle" takes the generic infeasible path:
    // no label, and the comment is the respecify-oriented one.
    let repo = init_repo("infeasible-generic");
    let extra = "## Feasible: no\nNo acceptance criteria and no verifiable done condition.";
    let queue = vec![issue(4)];
    let agent = ScriptedAgent::new(vec![])
        .infeasible()
        .with_plan_extra(extra);
    let tracker = RecordingTracker::default();

    let report = run_queue(
        &cfg(&repo, "stamp-generic", false),
        &queue,
        &agent,
        &tracker,
        &ScriptedClock::never(),
    )
    .unwrap();

    assert!(report.stop.is_none(), "infeasible does not stop the run");
    assert!(
        tracker.labels.borrow().is_empty(),
        "no needs-split label without a bundle verdict"
    );
    let comments = tracker.comments.borrow();
    assert_eq!(comments.len(), 1, "one skip comment");
    let (_, body) = &comments[0];
    assert!(
        body.contains("stays open"),
        "generic comment tells the human the issue was not closed: {body}"
    );
    assert!(
        !body.contains("/to-issues"),
        "generic comment does not prescribe a split: {body}"
    );

    fs::remove_dir_all(&repo).ok();
}

#[test]
fn closed_blockers_handoffs_feed_the_planner_and_stale_file_is_removed() {
    // Pass 1: #5 depends on closed #2, which left a handoff comment. The runner
    // must write `.ralphy/handoffs.md` before planning #5.
    {
        let repo = init_repo("handoff-feed");
        let queue = vec![issue_with_body(5, "## Blocked by\n- #2\n")];
        let agent = ScriptedAgent::new(vec![Outcome::Done]);
        let tracker = RecordingTracker {
            closed_issues: HashSet::from([2]),
            handoffs: HashMap::from([(
                2u64,
                "## Handoff\n\n- **Delivered**: lab fixtures\n- **Commands that work**: docker compose up -d".to_string(),
            )]),
            ..Default::default()
        };

        run_queue(
            &cfg(&repo, "stamp-feed", false),
            &queue,
            &agent,
            &tracker,
            &ScriptedClock::never(),
        )
        .unwrap();

        let handoffs_md = fs::read_to_string(repo.join(".ralphy").join("handoffs.md"))
            .expect("handoffs.md written");
        assert!(handoffs_md.contains("## From #2"), "names the source issue");
        assert!(
            handoffs_md.contains("lab fixtures") && handoffs_md.contains("docker compose up -d"),
            "carries the predecessor's handoff content"
        );
        assert!(
            handoffs_md.contains("leads, not truths"),
            "carries the staleness caveat"
        );

        fs::remove_dir_all(&repo).ok();
    }

    // Pass 2: an issue with no blockers must not inherit a stale handoffs.md
    // from a previous issue — the runner removes it.
    {
        let repo = init_repo("handoff-stale");
        let ralphy = repo.join(".ralphy");
        fs::create_dir_all(&ralphy).unwrap();
        fs::write(
            ralphy.join("handoffs.md"),
            "# stale from a previous issue\n",
        )
        .unwrap();

        let queue = vec![issue(7)];
        let agent = ScriptedAgent::new(vec![Outcome::Done]);
        let tracker = RecordingTracker::default();

        run_queue(
            &cfg(&repo, "stamp-stale", false),
            &queue,
            &agent,
            &tracker,
            &ScriptedClock::never(),
        )
        .unwrap();

        assert!(
            !ralphy.join("handoffs.md").exists(),
            "stale handoffs.md removed for an issue with no closed blockers"
        );

        fs::remove_dir_all(&repo).ok();
    }
}

#[test]
fn issue_comments_are_attached_to_the_planner_issue_json() {
    // The runner fetches the selected issue's comment thread and folds it into
    // `.ralphy/issue.json` (the `comments` array), so the planner reads the
    // discussion, not just the body.
    let repo = init_repo("comments-attach");
    let queue = vec![issue_with_body(5, "original body")];
    let agent = ScriptedAgent::new(vec![Outcome::Done]);
    let tracker = RecordingTracker {
        comment_threads: HashMap::from([(
            5u64,
            vec![
                "first clarification from a human".to_string(),
                "second: use the staging endpoint".to_string(),
            ],
        )]),
        ..Default::default()
    };

    run_queue(
        &cfg(&repo, "stamp-comments", false),
        &queue,
        &agent,
        &tracker,
        &ScriptedClock::never(),
    )
    .unwrap();

    let issue_json =
        fs::read_to_string(repo.join(".ralphy").join("issue.json")).expect("issue.json written");
    let parsed: serde_json::Value = serde_json::from_str(&issue_json).expect("valid JSON");
    let comments = parsed["comments"].as_array().expect("comments array");
    assert_eq!(comments.len(), 2, "both comments carried into issue.json");
    assert_eq!(comments[0], "first clarification from a human");
    assert_eq!(comments[1], "second: use the staging endpoint");

    fs::remove_dir_all(&repo).ok();
}
