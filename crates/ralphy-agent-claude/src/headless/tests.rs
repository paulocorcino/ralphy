use super::*;

/// A limit's reset carries the parsed hint and names an instant. The instant
/// is read from the clock apart from the code under test, so only its
/// presence is asserted, never its value.
fn assert_reset(reset: Option<ResetAt>, text: &str) {
    let reset = reset.expect("the limit text names a reset");
    assert_eq!(reset.text, text);
    assert!(reset.at.is_some(), "the reset names an instant");
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

#[test]
fn classify_done_from_flag() {
    assert_eq!(classify_outcome(Some("DONE\n"), false, None), Outcome::Done);
}

#[test]
fn classify_blocked_from_flag() {
    assert_eq!(
        classify_outcome(Some("BLOCKED missing key"), false, None),
        Outcome::Blocked("missing key".into())
    );
}

#[test]
fn classify_limit_beats_timeout() {
    // A timed-out session whose transcript shows a real rate-limit error line
    // classifies as Limit (oracle parity, ralphy.ps1:395-397) so the run
    // resumes after reset.
    let t = limit_jsonl("You've hit your usage limit");
    assert_eq!(classify_outcome(None, true, Some(&t)), Outcome::Limit(None));
}

#[test]
fn classify_timeout_when_no_limit_in_transcript() {
    assert_eq!(
        classify_outcome(None, true, Some("just a normal log")),
        Outcome::Timeout
    );
}

#[test]
fn classify_limit_from_transcript() {
    let t = limit_jsonl("You've reached your usage limit; resets 3:00pm");
    let got = classify_outcome(None, false, Some(&t));
    let Outcome::Limit(reset) = got else {
        panic!("expected a limit, got {got:?}");
    };
    assert_reset(reset, "15:00");
}

#[test]
fn classify_limit_from_subagent_session_limit_transcript() {
    // The exact line Claude Code records when the session cap is hit while the
    // interactive PTY remains alive (captured from a real 429).
    let t = limit_jsonl("You've hit your session limit · resets 8:10am (America/Bahia)");
    let got = classify_outcome(None, false, Some(&t));
    let Outcome::Limit(reset) = got else {
        panic!("expected a limit, got {got:?}");
    };
    assert_reset(reset, "08:10");
}

#[test]
fn classify_does_not_trip_on_source_that_mentions_limits() {
    // THE REGRESSION GUARD: running ralphy on a repo about rate limiting (its
    // own included) fills the transcript with tool results and assistant text
    // that say "usage limit" / "session limit" / "resets 3:00pm" — none of
    // which is a real limit. A structural detector must ignore all of it.
    let transcript = concat!(
        r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","#,
        r#""content":"fn is_limit_text(text) { /* rate limit, usage limit, session limit */ "#,
        r#"}\nassert_eq!(parse_reset_hhmm(\"resets 3:00pm\"), Some(\"15:00\"));"}]}}"#,
        "\n",
        r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","#,
        r#""text":"I'll wire the usage limit handling so a session limit auto-resumes after "#,
        r#"reset."}]}}"#,
    );
    assert_eq!(transcript_limit(transcript), None);
    // ...and a timed-out session over that transcript is a Timeout, not a Limit.
    assert_eq!(
        classify_outcome(None, true, Some(transcript)),
        Outcome::Timeout
    );
}

#[test]
fn classify_stuck_when_quiet_exit() {
    assert_eq!(
        classify_outcome(None, false, Some("just a normal log")),
        Outcome::Stuck
    );
    assert_eq!(classify_outcome(None, false, None), Outcome::Stuck);
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

#[test]
fn classify_exec_not_exited_with_limit_text_is_limit() {
    let out = "You've reached your usage limit; resets 3:00pm";
    let got = classify_exec_call(out, false, 5);
    let Some(HeadlessReason::Limit(reset)) = got else {
        panic!("expected a limit, got {got:?}");
    };
    assert_reset(reset, "15:00");
}

#[test]
fn classify_exec_not_exited_without_limit_is_timeout() {
    assert_eq!(
        classify_exec_call("partial output", false, 5),
        Some(HeadlessReason::Timeout)
    );
}

#[test]
fn classify_exec_blocked_sentinel() {
    let out = "some work\nRALPHY_BLOCKED_EXIT missing key\nmore text";
    assert_eq!(
        classify_exec_call(out, true, 5),
        Some(HeadlessReason::Blocked("missing key".into()))
    );
}

#[test]
fn classify_exec_done_via_done_sentinel() {
    let out = "all done\nRALPHY_DONE_EXIT\n";
    assert_eq!(classify_exec_call(out, true, 3), Some(HeadlessReason::Done));
}

#[test]
fn classify_exec_done_via_zero_open_steps() {
    assert_eq!(
        classify_exec_call("no sentinel", true, 0),
        Some(HeadlessReason::Done)
    );
}

#[test]
fn classify_exec_limit_on_natural_exit() {
    let out = "You've reached your usage limit; resets 3:00pm";
    let got = classify_exec_call(out, true, 2);
    let Some(HeadlessReason::Limit(reset)) = got else {
        panic!("expected a limit, got {got:?}");
    };
    assert_reset(reset, "15:00");
}

#[test]
fn classify_exec_limit_beats_done_sentinel() {
    // The oracle checks Test-LimitText first (ralphy.ps1:283): a usage limit
    // wins even when the same exited call emitted RALPHY_DONE_EXIT, so the
    // run resumes after reset instead of closing the issue.
    let out = "RALPHY_DONE_EXIT\nYou've reached your usage limit; resets 3:00pm";
    let got = classify_exec_call(out, true, 0);
    let Some(HeadlessReason::Limit(reset)) = got else {
        panic!("expected a limit, got {got:?}");
    };
    assert_reset(reset, "15:00");
}

#[test]
fn classify_exec_continue_when_no_terminal_condition() {
    assert_eq!(
        classify_exec_call("partial progress, no sentinel", true, 3),
        None
    );
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
    for (classified, committed) in calls.iter().take(max_exec_calls as usize) {
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
