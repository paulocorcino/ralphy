//! Characterization pins over the core-emitted event vocabulary (#219), and
//! the `exec_usage` model fold (#225).
//!
//! Each pin asserts the FULL `(level, target, message, field-key-set)` triple of a
//! consumed message, plus the observed encoding of the interesting values (`%order`
//! arrives rendered, `?blockers` as a Debug list). An added, dropped, renamed, or
//! re-sigiled field reds the pin — that is the drift class ADR-0039 §2 names. The
//! CLI-side decoder that consumes these lives in
//! `crates/ralphy-cli/src/runstate/event.rs`; the remaining 14 messages are pinned
//! there (`runstate::capture`).

use super::*;

#[test]
fn runner_emits_plan_written_steps_and_plan_opened_closed_snapshots() {
    // A single green issue exercises the plan-write point (plan written + plan
    // opened) and the close read (plan closed). Capture the run's tracing stream and
    // assert the three plan-lifecycle emissions carry their #96 fields.
    let repo = init_repo("plan-events");
    let queue = vec![issue(7)];
    let agent = ScriptedAgent::new(vec![Outcome::Done]);
    let tracker = RecordingTracker::default();

    install_global_capture();
    let captured = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    CAPTURE_TARGET.with(|t| *t.borrow_mut() = Some(captured.clone()));
    let report = run_queue(
        &cfg(&repo, "stamp-plan-events", false),
        &queue,
        &agent,
        &tracker,
        &ScriptedClock::never(),
    );
    CAPTURE_TARGET.with(|t| *t.borrow_mut() = None);
    let report = report.unwrap();
    assert!(report.stop.is_none(), "a single green issue completes");

    let events = captured.lock().unwrap();
    let find = |msg: &str| events.iter().find(|f| f.message == msg);

    // `plan written` now carries the serialized steps ([{text,status}]).
    let written = find("plan written").expect("a plan written event");
    let steps = written.steps_json.as_deref().expect("steps_json present");
    assert!(
        steps.contains("do a thing") && steps.contains("checked"),
        "steps_json must carry the checked step: {steps}"
    );

    // `plan opened` carries the raw plan markdown at the write point.
    let opened = find("plan opened").expect("a plan opened event");
    assert!(
        opened
            .plan_md
            .as_deref()
            .is_some_and(|m| m.contains("## Steps")),
        "plan opened must carry the raw plan_md: {:?}",
        opened.plan_md
    );

    // `plan closed` carries the raw plan markdown at the close read.
    let closed = find("plan closed").expect("a plan closed event");
    assert!(
        closed
            .plan_md
            .as_deref()
            .is_some_and(|m| m.contains("## Steps")),
        "plan closed must carry the raw plan_md: {:?}",
        closed.plan_md
    );

    fs::remove_dir_all(&repo).ok();
}

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

    let started = pin(
        &events,
        "issue started",
        T_EMIT,
        &["message", "number", "title"],
    );
    assert_eq!(started.get("number"), "7");
    assert_eq!(started.get("title"), "issue 7");

    let written = pin(
        &events,
        "plan written",
        T_EMIT,
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
    // `%steps_json` (Display) arrives as the raw JSON array, NOT a quoted Debug form.
    assert!(
        written.get("steps_json").starts_with('['),
        "steps_json must arrive Display-rendered: {}",
        written.get("steps_json")
    );

    let opened = pin(
        &events,
        "plan opened",
        T_EMIT,
        &["message", "number", "plan_md"],
    );
    assert_eq!(opened.get("number"), "7");
    assert!(opened.get("plan_md").contains("## Steps"));

    let closed = pin(
        &events,
        "plan closed",
        T_EMIT,
        &["message", "number", "plan_md"],
    );
    assert_eq!(closed.get("number"), "7");
    assert!(closed.get("plan_md").contains("## Steps"));

    let green = pin(
        &events,
        "green — issue closed",
        T_EMIT,
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
fn exec_usage_single_attempt_keeps_model() {
    let repo = init_repo("exec-usage-single");
    let queue = vec![issue(7)];
    let agent = ScriptedAgent::new(vec![Outcome::Done]).with_exec_usages(vec![Usage {
        input: 100,
        output: 400,
        cache_read: 200,
        cache_creation: 300,
        model: Some("claude-opus-4-8".into()),
    }]);
    let tracker = RecordingTracker::default();

    let (report, events) = capture_run(|| {
        run_queue(
            &cfg(&repo, "stamp-exec-usage-single", false),
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
        T_EMIT,
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
    assert_eq!(green.get("up"), "100");
    assert_eq!(green.get("out"), "400");

    fs::remove_dir_all(&repo).ok();
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
        T_EMIT,
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
fn exec_usage_without_model_stays_unattributed() {
    let repo = init_repo("exec-usage-unattributed");
    let queue = vec![issue(7)];
    let agent = ScriptedAgent::new(vec![Outcome::Done]).with_exec_usages(vec![Usage {
        input: 10,
        output: 0,
        cache_read: 0,
        cache_creation: 0,
        model: None,
    }]);
    let tracker = RecordingTracker::default();

    let (report, events) = capture_run(|| {
        run_queue(
            &cfg(&repo, "stamp-exec-usage-unattributed", false),
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
        T_EMIT,
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
    assert_eq!(green.get("model"), "");
    assert_eq!(green.get("up"), "10");

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
        T_EMIT,
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
            T_EMIT,
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
            T_EMIT,
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
        T_EMIT,
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
        T_EMIT,
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
        T_EMIT,
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
        T_EMIT,
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
        pin(
            &events,
            "bundle plan — needs split",
            T_EMIT,
            &["message", "number"],
        )
        .get("number"),
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
        T_EMIT,
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

    pin(&events, "reset reached — resuming", T_EMIT, &["message"]);
}
