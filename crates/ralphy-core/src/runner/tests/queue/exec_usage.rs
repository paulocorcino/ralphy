//! The `exec_usage` model fold on the green event (#225).

use super::capture::{capture_run, pin};
use super::*;

const GREEN_KEYS: [&str; 9] = [
    "cr",
    "cw",
    "invocations",
    "message",
    "model",
    "number",
    "out",
    "tokens",
    "up",
];

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
        let root = test_dir(&format!("q-exec-usage-single-{i}"));
        let queue = vec![test_issue(7)];
        let agent = ScriptedAgent::new(vec![Outcome::Done]).with_exec_usages(vec![usage]);
        let tracker = RecordingTracker::default();

        let ((report, _), events) = capture_run(|| {
            run_on_fake(
                &test_cfg(&root, &format!("stamp-exec-usage-single-{i}")),
                &queue,
                &agent,
                &tracker,
            )
        });
        assert!(report.stop.is_none(), "{case}");

        let green = pin(&events, "green — issue closed", &GREEN_KEYS);
        assert_eq!(green.get("model"), model, "{case}: model");
        assert_eq!(green.get("up"), up, "{case}: up");
        assert_eq!(green.get("out"), out, "{case}: out");

        remove_scratch(&root);
    }
}

#[test]
fn exec_usage_resume_loop_folds_heaviest_model() {
    let root = test_dir("q-exec-usage-resume");
    let queue = vec![test_issue(7)];
    let agent = ScriptedAgent::scripted(vec![
        (Outcome::Limit(Some(reset_text("15:00"))), false),
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

    let ((report, _), events) = capture_run(|| {
        run_on_fake(
            &test_cfg(&root, "stamp-exec-usage-resume"),
            &queue,
            &agent,
            &tracker,
        )
    });
    assert!(report.stop.is_none());

    let green = pin(&events, "green — issue closed", &GREEN_KEYS);
    assert_eq!(green.get("model"), "claude-opus-4-8");
    assert_eq!(green.get("up"), "1100");
    assert_eq!(green.get("cr"), "2200");
    assert_eq!(green.get("cw"), "3300");
    assert_eq!(green.get("out"), "4400");

    remove_scratch(&root);
}
