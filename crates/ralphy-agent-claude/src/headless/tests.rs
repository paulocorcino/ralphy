use super::*;

/// A limit's reset carries the parsed hint and names an instant. The instant
/// is read from the clock apart from the code under test, so only its
/// presence is asserted, never its value.
fn assert_reset(reset: Option<ResetAt>, text: &str, case: &str) {
    let reset = reset.unwrap_or_else(|| panic!("{case}: the limit text names a reset"));
    assert_eq!(reset.text, text, "{case}");
    assert!(reset.at.is_some(), "{case}: the reset names an instant");
}

#[test]
fn headless_command_maps_high_and_omits_unset() {
    let set = headless_args(
        Path::new("settings.json"),
        Path::new("plugin"),
        "sonnet",
        Some("high"),
    );
    assert_eq!(
        set.windows(2)
            .filter(|pair| pair == &["--effort", "high"])
            .count(),
        1
    );
    let unset = headless_args(
        Path::new("settings.json"),
        Path::new("plugin"),
        "sonnet",
        None,
    );
    assert!(!unset.iter().any(|arg| arg == "--effort"));
}

/// One real transcript api-error line carrying the limit banner `text`, in the
/// exact shape Claude Code writes (`isApiErrorMessage`+`error`+`apiErrorStatus`).
fn limit_jsonl(text: &str) -> String {
    serde_json::json!({
        "type": "assistant",
        "isApiErrorMessage": true,
        "error": "rate_limit",
        "apiErrorStatus": 429,
        "message": { "role": "assistant", "content": [ { "type": "text", "text": text } ] }
    })
    .to_string()
}

#[test]
fn a_clean_exec_with_auth_text_is_not_an_auth_error() {
    let text = "Not logged in \u{00b7} Please run /login\n";
    let ended = |exited_cleanly| RunFacts {
        exited_cleanly,
        timed_out: false,
        stopped: false,
    };
    assert!(!is_exec_auth_failure(ended(true), text));
    assert!(is_exec_auth_failure(ended(false), text));
}

/// A transcript that only talks about limits: tool output and assistant prose, no api
/// error. The regression guard: a repo about rate limiting must not read as a real limit.
const MENTIONS_LIMITS: &str = concat!(
    r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","#,
    r#""content":"fn is_limit_text(text) { /* rate limit, usage limit, session limit */ "#,
    r#"}\nassert_eq!(parse_reset_hhmm(\"resets 3:00pm\"), Some(\"15:00\"));"}]}}"#,
    "\n",
    r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","#,
    r#""text":"I'll wire the usage limit handling so a session limit auto-resumes after "#,
    r#"reset."}]}}"#,
);

/// A table row's expected value: an exact one, or a limit that names its reset text.
enum Want<T> {
    Is(T),
    LimitAt(&'static str),
}

struct OutcomeRow<'a> {
    name: &'static str,
    flag: Option<&'static str>,
    timed_out: bool,
    transcript: Option<&'a str>,
    want: Want<Outcome>,
}

#[test]
fn claude_reads_the_flag_and_the_transcript_before_the_ladder() {
    let usage_limit = limit_jsonl("You've hit your usage limit");
    let usage_limit_reset = limit_jsonl("You've reached your usage limit; resets 3:00pm");
    // The exact line Claude Code records when the session cap is hit while the PTY is alive.
    let session_limit =
        limit_jsonl("You've hit your session limit \u{b7} resets 8:10am (America/Bahia)");
    let rows = [
        OutcomeRow {
            name: "the DONE flag is done",
            flag: Some("DONE\n"),
            timed_out: false,
            transcript: None,
            want: Want::Is(Outcome::Done),
        },
        OutcomeRow {
            name: "the BLOCKED flag carries its reason",
            flag: Some("BLOCKED missing key"),
            timed_out: false,
            transcript: None,
            want: Want::Is(Outcome::Blocked("missing key".into())),
        },
        OutcomeRow {
            name: "a limit in the transcript beats a timeout",
            flag: None,
            timed_out: true,
            transcript: Some(&usage_limit),
            want: Want::Is(Outcome::Limit(None)),
        },
        OutcomeRow {
            name: "a timeout without a limit in the transcript",
            flag: None,
            timed_out: true,
            transcript: Some("just a normal log"),
            want: Want::Is(Outcome::Timeout),
        },
        OutcomeRow {
            name: "a usage limit names its reset",
            flag: None,
            timed_out: false,
            transcript: Some(&usage_limit_reset),
            want: Want::LimitAt("15:00"),
        },
        OutcomeRow {
            name: "a session limit names its reset",
            flag: None,
            timed_out: false,
            transcript: Some(&session_limit),
            want: Want::LimitAt("08:10"),
        },
        OutcomeRow {
            name: "source text that mentions limits is a timeout, not a limit",
            flag: None,
            timed_out: true,
            transcript: Some(MENTIONS_LIMITS),
            want: Want::Is(Outcome::Timeout),
        },
        OutcomeRow {
            name: "a quiet exit over a normal log is stuck",
            flag: None,
            timed_out: false,
            transcript: Some("just a normal log"),
            want: Want::Is(Outcome::Stuck),
        },
        OutcomeRow {
            name: "a quiet exit with no transcript is stuck",
            flag: None,
            timed_out: false,
            transcript: None,
            want: Want::Is(Outcome::Stuck),
        },
    ];
    for row in rows {
        let got = classify_outcome(row.flag, row.timed_out, row.transcript);
        match (got, row.want) {
            (Outcome::Limit(reset), Want::LimitAt(text)) => assert_reset(reset, text, row.name),
            (got, Want::Is(want)) => assert_eq!(got, want, "{}", row.name),
            (got, Want::LimitAt(_)) => panic!("{}: expected a limit, got {got:?}", row.name),
        }
    }
    assert_eq!(transcript_limit(MENTIONS_LIMITS), None);
}

// ── is_headless_api_degraded ────────────────────────────────────────────

#[test]
fn headless_degraded_matcher_matches_the_retry_banners() {
    assert!(is_headless_api_degraded("Waiting for API response…"));
    // Case-insensitive, and the second distinctive substring.
    assert!(is_headless_api_degraded(
        "API Error: Server error mid-response"
    ));
}

#[test]
fn headless_degraded_matcher_ignores_healthy_output() {
    // A healthy `-p` line must not match, or a busy child would starve its own
    // idle beacon and be wrongly reaped.
    assert!(!is_headless_api_degraded("Running cargo test..."));
    assert!(!is_headless_api_degraded(
        "the api call returned 200 · continuing"
    ));
}

// ── classify_exec_call ──────────────────────────────────────────────────

struct ExecRow {
    name: &'static str,
    out: &'static str,
    exited: bool,
    open_steps: usize,
    want: Want<Option<HeadlessReason>>,
}

#[test]
fn claude_reads_the_call_output_before_the_ladder() {
    const LIMIT_TEXT: &str = "You've reached your usage limit; resets 3:00pm";
    let rows = [
        ExecRow {
            name: "not exited with limit text is a limit",
            out: LIMIT_TEXT,
            exited: false,
            open_steps: 5,
            want: Want::LimitAt("15:00"),
        },
        ExecRow {
            name: "not exited without a limit is a timeout",
            out: "partial output",
            exited: false,
            open_steps: 5,
            want: Want::Is(Some(HeadlessReason::Timeout)),
        },
        ExecRow {
            name: "the blocked sentinel carries its reason",
            out: "some work\nRALPHY_BLOCKED_EXIT missing key\nmore text",
            exited: true,
            open_steps: 5,
            want: Want::Is(Some(HeadlessReason::Blocked("missing key".into()))),
        },
        ExecRow {
            name: "the done sentinel is done",
            out: "all done\nRALPHY_DONE_EXIT\n",
            exited: true,
            open_steps: 3,
            want: Want::Is(Some(HeadlessReason::Done)),
        },
        ExecRow {
            name: "zero open steps is done",
            out: "no sentinel",
            exited: true,
            open_steps: 0,
            want: Want::Is(Some(HeadlessReason::Done)),
        },
        ExecRow {
            name: "limit text on a natural exit is a limit",
            out: LIMIT_TEXT,
            exited: true,
            open_steps: 2,
            want: Want::LimitAt("15:00"),
        },
        // The oracle checks the limit text first: a limit wins even when the same
        // exited call emitted the done sentinel, so the run resumes after the reset.
        ExecRow {
            name: "a limit beats the done sentinel",
            out: "RALPHY_DONE_EXIT\nYou've reached your usage limit; resets 3:00pm",
            exited: true,
            open_steps: 0,
            want: Want::LimitAt("15:00"),
        },
        ExecRow {
            name: "no terminal condition continues the loop",
            out: "partial progress, no sentinel",
            exited: true,
            open_steps: 3,
            want: Want::Is(None),
        },
    ];
    for row in rows {
        let got = classify_exec_call(row.out, row.exited, row.open_steps);
        match (got, row.want) {
            (Some(HeadlessReason::Limit(reset)), Want::LimitAt(text)) => {
                assert_reset(reset, text, row.name);
            }
            (got, Want::Is(want)) => assert_eq!(got, want, "{}", row.name),
            (got, Want::LimitAt(_)) => panic!("{}: expected a limit, got {got:?}", row.name),
        }
    }
}

// ── headless_reason_to_outcome ──────────────────────────────────────────

#[test]
fn headless_reason_maps_onto_a_core_outcome() {
    // (headless reason, expected outcome)
    let rows = [
        (HeadlessReason::Done, Outcome::Done),
        (
            HeadlessReason::Blocked("reason".into()),
            Outcome::Blocked("reason".into()),
        ),
        (HeadlessReason::Timeout, Outcome::Timeout),
        (HeadlessReason::Stuck, Outcome::Stuck),
        (HeadlessReason::MaxCalls, Outcome::Stuck),
    ];
    for (reason, want) in rows {
        let case = format!("{reason:?}");
        assert_eq!(headless_reason_to_outcome(reason), want, "{case}");
    }
}

// ── loop-driver: stuck counter ──────────────────────────────────────────

/// Drive the *production* `headless_step` over a scripted sequence, mirroring
/// only the trivial `for i in 1..=max` bound in `execute_headless`. The
/// decision logic under test is the real `headless_step`, not a copy — so a
/// change to the loop's branching can't pass green here while diverging.
fn run_headless_steps(
    calls: &[(Option<HeadlessReason>, bool)], // (classify result, committed) per call
    max_exec_calls: u32,
) -> HeadlessReason {
    let mut streak = 0u32;
    for (classified, committed) in calls
        .iter()
        .take(usize::try_from(max_exec_calls).expect("a u32 fits usize"))
    {
        match headless_step(streak, classified.clone(), *committed) {
            LoopStep::Terminal(r) => return r,
            LoopStep::Continue(s) => streak = s,
        }
    }
    HeadlessReason::MaxCalls
}

#[test]
fn headless_loop_decides_each_call_sequence() {
    // (classify result, committed) for one call.
    type Call = (Option<HeadlessReason>, bool);
    // (case, calls, expected reason)
    let rows: [(&str, Vec<Call>, HeadlessReason); 3] = [
        (
            "a terminal reason passes through",
            vec![(Some(HeadlessReason::Done), false)],
            HeadlessReason::Done,
        ),
        (
            "two no-commit calls in a row are stuck",
            vec![(None, false), (None, false)],
            HeadlessReason::Stuck,
        ),
        (
            // Without the reset, call 3 would be the second no-commit call.
            "a commit resets the no-commit streak",
            vec![
                (None, false),
                (None, true),
                (None, false),
                (Some(HeadlessReason::Done), false),
            ],
            HeadlessReason::Done,
        ),
    ];
    for (case, calls, want) in rows {
        assert_eq!(run_headless_steps(&calls, 6), want, "{case}");
    }
}
