//! The `exec_usage` model fold on the green event (#225).

use super::*;

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

        remove_scratch(&repo);
    }
}

#[test]
fn exec_usage_resume_loop_folds_heaviest_model() {
    let repo = init_repo("exec-usage-resume");
    let queue = vec![issue(7)];
    let agent = ScriptedAgent::scripted(vec![
        (
            Outcome::Limit(Some(ralphy_core::ResetAt {
                text: "15:00".into(),
                at: None,
            })),
            false,
        ),
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

    remove_scratch(&repo);
}
