//! Characterization pins over the core-emitted event vocabulary (#219), and
//! the `exec_usage` model fold (#225).
//!
//! Each pin asserts the FULL `(level, message, field-key-set)` triple of a
//! consumed message, plus the observed encoding of the interesting values (`%order`
//! arrives rendered, `?blockers` as a Debug list). An added, dropped, renamed, or
//! re-sigiled field reds the pin — that is the drift class ADR-0039 §2 names. The
//! CLI-side decoder that consumes these lives in
//! `crates/ralphy-cli/src/runstate/event.rs`; the remaining 14 messages are pinned
//! there (`runstate::capture`).

use super::*;

#[test]
fn pins_green_run_vocabulary() {
    let repo = init_repo("pins-green");
    let queue = vec![issue(7)];
    let agent = ScriptedAgent::new(vec![Outcome::Done]);
    let tracker = RecordingTracker::default();

    let (report, events) = capture_run(|| {
        run_queue(
            &cfg(&repo, "stamp-pins-green", false),
            &queue,
            &agent,
            &tracker,
            &ScriptedClock::never(),
        )
    });
    assert!(
        report.unwrap().stop.is_none(),
        "a single green issue completes"
    );

    let started = pin(&events, "issue started", &["message", "number", "title"]);
    assert_eq!(started.get("number"), "7");
    assert_eq!(started.get("title"), "issue 7");

    let written = pin(
        &events,
        "plan written",
        &[
            "cr",
            "cw",
            "message",
            "model",
            "number",
            "open_steps",
            "out",
            "steps_json",
            "up",
        ],
    );
    assert_eq!(written.get("number"), "7");
    assert_eq!(written.get("open_steps"), "1");
    // `%steps_json` (Display) arrives as the raw JSON array, NOT a quoted Debug
    // form, and it carries the plan's checked step (#96).
    let steps = written.get("steps_json");
    assert!(
        steps.starts_with('[') && steps.contains("do a thing") && steps.contains("checked"),
        "steps_json must arrive Display-rendered with the checked step: {steps}"
    );

    let opened = pin(&events, "plan opened", &["message", "number", "plan_md"]);
    assert_eq!(opened.get("number"), "7");
    assert!(opened.get("plan_md").contains("## Steps"));

    let closed = pin(&events, "plan closed", &["message", "number", "plan_md"]);
    assert_eq!(closed.get("number"), "7");
    assert!(closed.get("plan_md").contains("## Steps"));

    let green = pin(
        &events,
        "green — issue closed",
        &[
            "cr",
            "cw",
            "invocations",
            "message",
            "model",
            "number",
            "out",
            "tokens",
            "up",
        ],
    );
    assert_eq!(green.get("number"), "7");
    // A clean green issue is two vendor spawns — plan + execute, no repair/protocol
    // bounce — so the invocation count is 2.
    assert_eq!(green.get("invocations"), "2");

    fs::remove_dir_all(&repo).ok();
}

#[test]
fn exec_usage_single_attempt_keeps_its_model_or_none() {
    // (case, execute usage, expected model, expected up, expected out)
    let rows = [
        (
            "single attempt keeps its model",
            Usage {
                input: 100,
                output: 400,
                cache_read: 200,
                cache_creation: 300,
                model: Some("claude-opus-4-8".into()),
            },
            "claude-opus-4-8",
            "100",
            "400",
        ),
        (
            "no model stays unattributed",
            Usage {
                input: 10,
                output: 0,
                cache_read: 0,
                cache_creation: 0,
                model: None,
            },
            "",
            "10",
            "0",
        ),
    ];
    for (i, (case, usage, model, up, out)) in rows.into_iter().enumerate() {
        let repo = init_repo(&format!("exec-usage-single-{i}"));
        let queue = vec![issue(7)];
        let agent = ScriptedAgent::new(vec![Outcome::Done]).with_exec_usages(vec![usage]);
        let tracker = RecordingTracker::default();

        let (report, events) = capture_run(|| {
            run_queue(
                &cfg(&repo, &format!("stamp-exec-usage-single-{i}"), false),
                &queue,
                &agent,
                &tracker,
                &ScriptedClock::never(),
            )
        });
        assert!(report.unwrap().stop.is_none(), "{case}");

        let green = pin(
            &events,
            "green — issue closed",
            &[
                "cr",
                "cw",
                "invocations",
                "message",
                "model",
                "number",
                "out",
                "tokens",
                "up",
            ],
        );
        assert_eq!(green.get("model"), model, "{case}: model");
        assert_eq!(green.get("up"), up, "{case}: up");
        assert_eq!(green.get("out"), out, "{case}: out");

        fs::remove_dir_all(&repo).ok();
    }
}

#[test]
fn exec_usage_resume_loop_folds_heaviest_model() {
    let repo = init_repo("exec-usage-resume");
    let queue = vec![issue(7)];
    let agent = ScriptedAgent::scripted(vec![
        (Outcome::Limit(Some("15:00".into())), false),
        (Outcome::Done, true),
    ])
    .with_exec_usages(vec![
        Usage {
            input: 100,
            output: 400,
            cache_read: 200,
            cache_creation: 300,
            model: Some("claude-haiku-4-5".into()),
        },
        Usage {
            input: 1000,
            output: 4000,
            cache_read: 2000,
            cache_creation: 3000,
            model: Some("claude-opus-4-8".into()),
        },
    ]);
    let tracker = RecordingTracker::default();

    let (report, events) = capture_run(|| {
        run_queue(
            &cfg(&repo, "stamp-exec-usage-resume", false),
            &queue,
            &agent,
            &tracker,
            &ScriptedClock::never(),
        )
    });
    assert!(report.unwrap().stop.is_none());

    let green = pin(
        &events,
        "green — issue closed",
        &[
            "cr",
            "cw",
            "invocations",
            "message",
            "model",
            "number",
            "out",
            "tokens",
            "up",
        ],
    );
    assert_eq!(green.get("model"), "claude-opus-4-8");
    assert_eq!(green.get("up"), "1100");
    assert_eq!(green.get("cr"), "2200");
    assert_eq!(green.get("cw"), "3300");
    assert_eq!(green.get("out"), "4400");

    fs::remove_dir_all(&repo).ok();
}

#[test]
fn pins_skip_and_stop_vocabulary() {
    // Non-green stop: #1 green, #2 blocked.
    let repo = init_repo("pins-nongreen");
    let queue = vec![issue(1), issue(2)];
    let agent = ScriptedAgent::new(vec![Outcome::Done, Outcome::Blocked("nope".into())]);
    let (_r, events) = capture_run(|| {
        run_queue(
            &cfg(&repo, "stamp-pins-nongreen", false),
            &queue,
            &agent,
            &RecordingTracker::default(),
            &ScriptedClock::never(),
        )
    });
    let non_green = pin(
        &events,
        "non-green — stopping run",
        &["message", "number", "outcome"],
    );
    assert_eq!(non_green.get("number"), "2");
    // `?outcome` (Debug on the shorthand) — the decoder reads this rendered form.
    assert_eq!(non_green.get("outcome"), "Blocked(\"nope\")");
    fs::remove_dir_all(&repo).ok();

    // Deadline: the clock reports passed before #2.
    let repo = init_repo("pins-deadline");
    let queue = vec![issue(1), issue(2)];
    let agent = ScriptedAgent::new(vec![Outcome::Done, Outcome::Done]);
    let (_r, events) = capture_run(|| {
        run_queue(
            &cfg(&repo, "stamp-pins-deadline", false),
            &queue,
            &agent,
            &RecordingTracker::default(),
            &ScriptedClock::passes_after(1),
        )
    });
    assert_eq!(
        pin(
            &events,
            "deadline passed — not starting issue",
            &["message", "number"],
        )
        .get("number"),
        "2"
    );
    fs::remove_dir_all(&repo).ok();

    // Stop-before label on #2.
    let repo = init_repo("pins-stopbefore");
    let queue = vec![issue(1), issue_labeled(2, &["stop-before"]), issue(3)];
    let agent = ScriptedAgent::new(vec![Outcome::Done]);
    let (_r, events) = capture_run(|| {
        run_queue(
            &cfg(&repo, "stamp-pins-stopbefore", false),
            &queue,
            &agent,
            &RecordingTracker::default(),
            &ScriptedClock::never(),
        )
    });
    assert_eq!(
        pin(
            &events,
            "stop-before label — halting run before this issue",
            &["message", "number"],
        )
        .get("number"),
        "2"
    );
    fs::remove_dir_all(&repo).ok();

    // Human-return label on #1; the queue continues to #2.
    let repo = init_repo("pins-humanreturn");
    let queue = vec![issue_labeled(1, &["AFK", "needs-info"]), issue(2)];
    let agent = ScriptedAgent::new(vec![Outcome::Done]);
    let (_r, events) = capture_run(|| {
        run_queue(
            &cfg(&repo, "stamp-pins-hr", false),
            &queue,
            &agent,
            &RecordingTracker::default(),
            &ScriptedClock::never(),
        )
    });
    let hr = pin(
        &events,
        "human-return label — skipping issue",
        &["label", "message", "number"],
    );
    assert_eq!(hr.get("number"), "1");
    // `%label` (Display) — the decoder's `clean_opt` strips no quotes here.
    assert_eq!(hr.get("label"), "needs-info");
    fs::remove_dir_all(&repo).ok();

    // Verify gate red past the repair budget on #1; the queue continues to #2.
    let repo = init_repo("pins-verifyfail");
    let queue = vec![issue(1), issue(2)];
    let agent = ScriptedAgent::new(vec![Outcome::Done, Outcome::Done])
        .with_plan_extra_for(1, format!("## Verify\n\n{}\n", verify_fail_line()))
        .with_plan_extra_for(2, format!("## Verify\n\n{}\n", verify_ok_line()));
    let (_r, events) = capture_run(|| {
        run_queue(
            &cfg(&repo, "stamp-pins-verifyfail", false),
            &queue,
            &agent,
            &RecordingTracker::default(),
            &ScriptedClock::never(),
        )
    });
    let vg = pin(
        &events,
        "verify gate failed — skipping issue",
        &["message", "number", "summary"],
    );
    assert_eq!(vg.get("number"), "1");
    assert!(
        !vg.get("summary").is_empty(),
        "the `%summary` field must carry the failure text"
    );
    fs::remove_dir_all(&repo).ok();
}

#[test]
fn pins_blocked_and_split_vocabulary() {
    // #5 blocked by open, agent-owned #2.
    let repo = init_repo("pins-blocked");
    let queue = vec![issue_with_body(5, "## Blocked by\n- #2\n"), issue(2)];
    let agent = ScriptedAgent::new(vec![Outcome::Done]);
    let (_r, events) = capture_run(|| {
        run_queue(
            &cfg(&repo, "stamp-pins-blocked", false),
            &queue,
            &agent,
            &RecordingTracker::default(),
            &ScriptedClock::never(),
        )
    });
    let blocked = pin(
        &events,
        "blocked by open issue(s) — skipping",
        &["blockers", "message", "number"],
    );
    assert_eq!(blocked.get("number"), "5");
    // `?blockers` — a Debug-rendered `Vec<u64>` the decoder parses the numbers out of.
    assert_eq!(blocked.get("blockers"), "[2]");
    fs::remove_dir_all(&repo).ok();

    // #5 blocked by open #2, which carries a human gate.
    let repo = init_repo("pins-humangate");
    let queue = vec![issue_with_body(5, "## Blocked by\n- #2\n"), issue(7)];
    let agent = ScriptedAgent::new(vec![Outcome::Done]);
    let tracker = RecordingTracker {
        issue_labels: HashMap::from([(2u64, vec!["ready-for-human".to_string()])]),
        ..Default::default()
    };
    let (_r, events) = capture_run(|| {
        run_queue(
            &cfg(&repo, "stamp-pins-humangate", false),
            &queue,
            &agent,
            &tracker,
            &ScriptedClock::never(),
        )
    });
    let hb = pin(
        &events,
        "blocked — waiting on human",
        &["blockers", "human_blockers", "message", "number"],
    );
    assert_eq!(hb.get("number"), "5");
    assert_eq!(hb.get("blockers"), "[2]");
    assert_eq!(hb.get("human_blockers"), "[2]");
    fs::remove_dir_all(&repo).ok();

    // An infeasible plan whose reason names a bundle → the needs-split verdict.
    let repo = init_repo("pins-bundle");
    let queue = vec![issue(3)];
    let agent = ScriptedAgent::new(vec![]).infeasible().with_plan_extra(
        "## Feasible: no\nThe issue bundles six PRD breakdown tasks; split into W1-T01..T06.",
    );
    let (_r, events) = capture_run(|| {
        run_queue(
            &cfg(&repo, "stamp-pins-bundle", false),
            &queue,
            &agent,
            &RecordingTracker::default(),
            &ScriptedClock::never(),
        )
    });
    assert_eq!(
        pin(&events, "bundle plan — needs split", &["message", "number"],).get("number"),
        "3"
    );
    fs::remove_dir_all(&repo).ok();
}

#[test]
fn pins_usage_limit_vocabulary() {
    // The two sleep-boundary emissions live on the REAL `WallClock`, not the
    // `ScriptedClock` the runner tests inject — so drive it directly. A reset
    // instant more than the 5-minute wake buffer in the past makes the wait
    // resolve on its first loop turn, with no sleep.
    let past = (chrono::Local::now() - chrono::Duration::minutes(30)).to_rfc3339();
    let (outcome, events) =
        capture_run(|| ralphy_core::WallClock { deadline: None }.wait_for_reset(&past));
    assert_eq!(outcome, ralphy_core::WaitOutcome::Resumed);

    let sleep = pin(
        &events,
        "usage limit — waiting for reset",
        &["hint", "message", "reset", "target_epoch"],
    );
    // `%reset` is the WAKE time-of-day (`HH:MM`), not the raw hint — the decoder
    // carries it straight into the card's countdown label.
    assert_eq!(
        sleep.get("reset").len(),
        5,
        "reset is HH:MM: {}",
        sleep.get("reset")
    );
    assert_eq!(sleep.get("hint"), past, "the raw hint stays on `hint`");
    assert!(
        sleep.get("target_epoch").parse::<i64>().is_ok(),
        "target_epoch is an i64 timestamp: {}",
        sleep.get("target_epoch")
    );

    pin(&events, "reset reached — resuming", &["message"]);
}
