use super::*;
use ralphy_proc_util::opencode::OpenCodeMajor::V1;

// ── is_opencode_auth_error ──────────────────────────────────────────────

#[test]
fn is_opencode_auth_error_matches_captured_provider_auth_error() {
    // Representative captured log from a signed-out `opencode run`: the SDK
    // emits a `ProviderAuthError` event name in the JSON error event and may
    // also print it to stderr. Either occurrence triggers the detector.
    let json_event = r#"{"type":"error","name":"ProviderAuthError","message":"Not authenticated"}"#;
    assert!(
        is_opencode_auth_error(json_event),
        "must match a ProviderAuthError JSON event"
    );

    // Mixed log with stderr text (case-insensitive check).
    let mixed_log = "some init output\nError: ProviderAuthError: not signed in\n";
    assert!(
        is_opencode_auth_error(mixed_log),
        "must match ProviderAuthError in stderr text"
    );

    // Upper-cased variant — to_ascii_lowercase makes it case-insensitive.
    assert!(
        is_opencode_auth_error("PROVIDERAUTHERROR"),
        "must be case-insensitive"
    );
}

#[test]
fn is_opencode_auth_error_ignores_unrelated_text() {
    assert!(
        !is_opencode_auth_error("all steps green\nRALPHY_DONE_EXIT\n"),
        "must not match a clean DONE sentinel"
    );
    assert!(
        !is_opencode_auth_error("timeout waiting for response"),
        "must not match an unrelated error"
    );
    assert!(!is_opencode_auth_error(""), "must not match empty text");
}

// ── is_opencode_api_degraded ─────────────────────────────────────────────

#[test]
fn degraded_matches_retryable_error_event() {
    // A 500 backend blip: a retryable error event, not a limit or auth — the
    // degraded clock should track it.
    let line =
        r#"{"type":"error","name":"APIError","statusCode":500,"message":"internal server error"}"#;
    assert!(is_opencode_api_degraded(V1, line));
}

#[test]
fn degraded_ignores_healthy_and_terminal_lines() {
    // Healthy JSON event lines and plain assistant text are not degraded.
    assert!(!is_opencode_api_degraded(
        V1,
        r#"{"type":"text","text":"working on it"}"#
    ));
    assert!(!is_opencode_api_degraded(V1, "just some assistant prose"));
    // A terminal usage limit (429) is NOT degraded — it is handled as a limit.
    assert!(!is_opencode_api_degraded(
        V1,
        r#"{"type":"error","name":"APIError","statusCode":429,"message":"rate limited"}"#
    ));
    // A terminal auth error is NOT degraded either.
    assert!(!is_opencode_api_degraded(
        V1,
        r#"{"type":"error","name":"ProviderAuthError","message":"Not authenticated"}"#
    ));
}

// ── parse_opencode_limit ─────────────────────────────────────────────────

/// A detected limit whose reset hint is `reset`.
fn hit(reset: &str) -> OpencodeLimit {
    OpencodeLimit {
        reset: Some(reset.into()),
    }
}

/// A detected limit with no reset hint.
fn hit_without_reset() -> OpencodeLimit {
    OpencodeLimit { reset: None }
}

struct LimitRow {
    name: &'static str,
    input: &'static str,
    want: Option<OpencodeLimit>,
}

fn assert_json_limits(rows: Vec<LimitRow>) {
    for row in rows {
        assert_eq!(
            parse_opencode_limit(V1, row.input),
            row.want,
            "{}",
            row.name
        );
    }
}

#[test]
fn parse_limit_reads_the_reset_hint_of_a_429() {
    assert_json_limits(vec![
        LimitRow {
            name: "429 with a retryAfter timestamp",
            input: concat!(
                r#"{"type":"text","text":"working"}"#,
                "\n",
                r#"{"type":"error","name":"APIError","statusCode":429,"message":"rate limited","#,
                r#""retryAfter":"2026-06-10T18:00:00Z"}"#,
                "\n",
            ),
            want: Some(hit("2026-06-10T18:00:00Z")),
        },
        LimitRow {
            name: "a numeric retryAfter",
            input: concat!(
                r#"{"type":"error","name":"APIError","statusCode":429,"message":"rate limited","#,
                r#""retryAfter":120}"#,
                "\n",
            ),
            want: Some(hit("120")),
        },
        LimitRow {
            name: "a Retry-After header in the message",
            input: concat!(
                r#"{"type":"error","name":"APIError","statusCode":429,"message":"Too many "#,
                r#"requests. Retry-After: 45, slow down"}"#,
                "\n",
            ),
            want: Some(hit("45")),
        },
        LimitRow {
            name: "try again in, from the message",
            input: concat!(
                r#"{"type":"error","name":"APIError","statusCode":429,"message":"Rate limit "#,
                r#"exceeded. Try again in 20 minutes. Thanks"}"#,
                "\n",
            ),
            want: Some(hit("20 minutes")),
        },
        LimitRow {
            name: "try again at, from the retryable literal",
            input: concat!(
                r#"{"type":"error","name":"APIError","statusCode":429,"message":"Rate limit "#,
                r#"exceeded. Try again at 2026-06-10T19:00:00Z"}"#,
                "\n",
            ),
            want: Some(hit("2026-06-10T19:00:00Z")),
        },
        LimitRow {
            name: "429 without a reset hint",
            input: concat!(
                r#"{"type":"error","name":"APIError","statusCode":429,"#,
                r#""message":"too many requests"}"#,
                "\n",
            ),
            want: Some(hit_without_reset()),
        },
        LimitRow {
            name: "a Zen usage limit error name, with no reset hint",
            input: concat!(
                r#"{"type":"error","name":"KimiUsageLimitError","#,
                r#""message":"usage limit reached"}"#,
                "\n",
            ),
            want: Some(hit_without_reset()),
        },
    ]);
}

#[test]
fn parse_limit_reads_the_other_error_shapes() {
    assert_json_limits(vec![
        // opencode 1.16.2 envelope: name, statusCode and retryAfter live under `error.data`.
        LimitRow {
            name: "a 429 in the error.data envelope",
            input: concat!(
                r#"{"type":"error","sessionID":"s","error":{"name":"APIError","data":{"#,
                r#""statusCode":429,"message":"rate limit exceeded","#,
                r#""retryAfter":"2026-06-10T20:00:00Z"}}}"#,
            ),
            want: Some(hit("2026-06-10T20:00:00Z")),
        },
        LimitRow {
            name: "a 429 nested under part",
            input: concat!(
                r#"{"type":"error","sessionID":"s","part":{"type":"error","name":"APIError","#,
                r#""statusCode":429,"message":"rate limited","#,
                r#""retryAfter":"2026-06-10T18:00:00Z"}}"#,
                "\n",
            ),
            want: Some(hit("2026-06-10T18:00:00Z")),
        },
        // Kimi's billing-cycle block: a 403, not a 429, with no reset hint.
        LimitRow {
            name: "a Kimi 403 billing-cycle block",
            input: concat!(
                r#"{"type":"error","error":{"name":"APIError","data":{"message":"You've "#,
                r#"reached your usage limit for this billing cycle. Your quota will be "#,
                r#"refreshed in the next cycle.","statusCode":403,"isRetryable":false}}}"#,
            ),
            want: Some(hit_without_reset()),
        },
    ]);
}

#[test]
fn parse_limit_ignores_errors_that_are_not_a_limit() {
    assert_json_limits(vec![
        // A transient backend failure captured live from opencode 1.16.2, not a limit.
        LimitRow {
            name: "an UnknownError backend blip is not a limit",
            input: concat!(
                r#"{"type":"error","timestamp":1781088576836,"sessionID":"ses_x","#,
                r#""error":{"name":"UnknownError","data":{"message":"Unexpected server "#,
                r#"error. Check server logs for details.","ref":"err_7391de1e"}}}"#,
            ),
            want: None,
        },
        // A permission error must fail as Stuck, not sleep.
        LimitRow {
            name: "a plain 403 is not a limit",
            input: concat!(
                r#"{"type":"error","error":{"name":"APIError","data":{"message":"forbidden: "#,
                r#"model not available","statusCode":403}}}"#,
            ),
            want: None,
        },
        LimitRow {
            name: "a 500 is not a limit",
            input: concat!(
                r#"{"type":"error","name":"APIError","statusCode":500,"#,
                r#""message":"internal server error"}"#,
                "\n",
            ),
            want: None,
        },
        LimitRow {
            name: "a clean stream has no limit",
            input: concat!(
                r#"{"type":"text","text":"working on it"}"#,
                "\n",
                r#"{"type":"text","text":"RALPHY_DONE_EXIT"}"#,
                "\n",
                r#"{"type":"step_finish","reason":"stop"}"#,
                "\n",
            ),
            want: None,
        },
    ]);
}

#[test]
fn an_unknown_error_envelope_is_an_error_event_but_not_a_limit() {
    let stream = concat!(
        r#"{"type":"error","timestamp":1781088576836,"sessionID":"ses_x","#,
        r#""error":{"name":"UnknownError","data":{"message":"Unexpected server error. Check "#,
        r#"server logs for details.","ref":"err_7391de1e"}}}"#,
    );
    // It downgrades a Done claim to Stuck on execute.
    let (_t, saw_error) = parse_opencode_events(V1, stream);
    assert!(saw_error, "the real error envelope must flag saw_error");
}

// ── parse_opencode_log_limit ─────────────────────────────────────────────

#[test]
fn log_limit_reads_the_logfmt_error_line() {
    let rows = [
        // Captured live 2026-07-11 (FinCal #71): no JSON error event accompanies it,
        // so only the log scan catches it. The reset value has a space and stays whole.
        LimitRow {
            name: "the Z.ai 5 hour cap names its reset",
            input: concat!(
                "{\"type\":\"step_finish\",\"reason\":\"stop\"}\n",
                "timestamp=2026-07-11T09:48:22.735Z level=ERROR run=d9ec1918 ",
                "message=\"stream error\" providerID=zai-coding-plan modelID=glm-5.2 ",
                "session.id=ses_x error.error=\"AI_APICallError: Usage limit reached for ",
                "5 hour. Your limit will reset at 2026-07-11 22:14:08\"",
            ),
            want: Some(hit("2026-07-11 22:14:08")),
        },
        // Captured live 2026-07-13 (FinCal #77): `Limit Exhausted` follows no `quota`.
        LimitRow {
            name: "the Z.ai weekly limit exhausted wording",
            input: concat!(
                "timestamp=2026-07-13T17:06:04.998Z level=ERROR run=d9a3c1f7 ",
                "message=\"stream error\" providerID=zai-coding-plan modelID=glm-5.2 ",
                "session.id=ses_x error.error=\"AI_APICallError: Weekly/Monthly Limit ",
                "Exhausted. Your limit will reset at 2026-07-18 11:26:07\"",
            ),
            want: Some(hit("2026-07-18 11:26:07")),
        },
        LimitRow {
            name: "the Kimi billing cycle has no reset",
            input: concat!(
                "timestamp=2026-07-09T23:19:01.732Z level=ERROR message=\"stream error\" ",
                "providerID=kimi-for-coding error.error=\"AI_APICallError: You've reached ",
                "your usage limit for this billing cycle. Your quota will be refreshed in ",
                "the next cycle.\"",
            ),
            want: Some(hit_without_reset()),
        },
        // The reset after the phrase is read, not the line's own `timestamp=` instant.
        LimitRow {
            name: "any provider, with an ISO reset",
            input: concat!(
                "timestamp=2026-08-01T09:00:00.000Z level=ERROR message=\"stream error\" ",
                "error.error=\"Rate limit exceeded. Please try again at 2026-08-01T10:30:00Z\"",
            ),
            want: Some(hit("2026-08-01T10:30:00Z")),
        },
        // Seen, but no schedulable reset: the caller retries on the synthetic cadence.
        LimitRow {
            name: "a relative hint is a limit without a reset",
            input: "level=ERROR error.error=\"Usage limit reached. Try again in 30 minutes.\"",
            want: Some(hit_without_reset()),
        },
        LimitRow {
            name: "an INFO line and a non-limit ERROR are not a limit",
            input: concat!(
                "{\"type\":\"text\",\"text\":\"working\"}\n",
                "timestamp=2026-07-11T09:47:41.590Z level=INFO message=\"llm runtime ",
                "selected\" llm.provider=zai-coding-plan llm.model=glm-5.2\n",
                "timestamp=2026-07-11T09:48:22.735Z level=ERROR message=\"stream error\" ",
                "error.error=\"AI_APICallError: Unexpected server error\"",
            ),
            want: None,
        },
    ];
    for row in rows {
        assert_eq!(
            parse_opencode_log_limit(row.input),
            row.want,
            "{}",
            row.name
        );
    }
}

// ── parse_opencode_events ────────────────────────────────────────────────

#[test]
fn parse_extracts_text_parts() {
    let stream = "{\"type\":\"step_start\",\"snapshot\":\"abc\"}\n\
                  {\"type\":\"text\",\"text\":\"working on it\"}\n\
                  {\"type\":\"text\",\"text\":\"RALPHY_DONE_EXIT\"}\n\
                  {\"type\":\"step_finish\",\"reason\":\"stop\"}\n";
    let (text, saw_error) = parse_opencode_events(V1, stream);
    assert!(text.contains("working on it"), "text: {text:?}");
    assert!(text.contains("RALPHY_DONE_EXIT"), "text: {text:?}");
    assert!(!saw_error);
}

#[test]
fn parse_flags_error_event() {
    let stream = "{\"type\":\"text\",\"text\":\"trying\"}\n\
                  {\"type\":\"error\",\"name\":\"APIError\",\"statusCode\":500}\n";
    let (text, saw_error) = parse_opencode_events(V1, stream);
    assert!(text.contains("trying"));
    assert!(saw_error, "an error event must set the flag");
}

#[test]
fn parse_extracts_text_from_nested_part_envelope() {
    // The real opencode 1.16.2 `--format json` shape, captured live: every
    // event is wrapped `{type, timestamp, sessionID, part:{…}}` and the text
    // lives under `part.text`. The sentinel scan must see it through the
    // envelope, or every execute run misclassifies as Stuck.
    let stream = concat!(
        r#"{"type":"step_start","sessionID":"s","part":{"type":"step-start","snapshot":"abc"}}"#,
        "\n",
        r#"{"type":"tool_use","sessionID":"s","part":{"type":"tool","tool":"read","callID":"c1"}}"#,
        "\n",
        concat!(
            r#"{"type":"text","sessionID":"s","part":{"type":"text","text":"did the "#,
            r#"work\nRALPHY_DONE_EXIT"}}"#,
        ),
        "\n",
        r#"{"type":"step_finish","sessionID":"s","part":{"type":"step-finish","reason":"stop"}}"#,
        "\n",
    );
    let (text, saw_error) = parse_opencode_events(V1, stream);
    assert!(
        text.contains("RALPHY_DONE_EXIT"),
        "must extract the sentinel from part.text: {text:?}"
    );
    assert!(
        ralphy_adapter_support::done_sentinel(&text),
        "done_sentinel must fire on the extracted text"
    );
    // A `tool` part must not be mistaken for text or an error.
    assert!(!saw_error, "a tool_use envelope is not an error");
}

#[test]
fn parse_flags_error_event_in_nested_part() {
    // An error carried inside the `part` envelope must still set saw_error.
    let stream = concat!(
        r#"{"type":"error","sessionID":"s","part":{"type":"error","name":"APIError","#,
        r#""statusCode":500}}"#,
    );
    let (_text, saw_error) = parse_opencode_events(V1, stream);
    assert!(saw_error, "a nested error part must flag saw_error");
}

#[test]
fn parse_tolerates_unparseable_lines() {
    let stream = "not json at all\n\
                  {\"type\":\"text\",\"text\":\"kept\"}\n\
                  \n";
    let (text, saw_error) = parse_opencode_events(V1, stream);
    assert_eq!(text.trim(), "kept");
    assert!(!saw_error);
}

#[test]
fn v1_fixtures_classify_as_before() {
    use ralphy_core::Outcome::{Done, Stuck};
    use v2::tests::{fixture, verdict, Verdict::Ends};
    for (name, exited, want) in [
        ("v1/execute-win.jsonl", true, Ends(Done)),
        ("v1/execute-wsl.jsonl", true, Ends(Done)),
        ("v1/plan-win.jsonl", true, Ends(Done)),
        ("v1/auth-bad-key-win.jsonl", false, Ends(Stuck)),
        ("v1/no-login-win.jsonl", false, Ends(Stuck)),
    ] {
        let stdout = fixture(name);
        assert_eq!(verdict(V1, &stdout, &stdout, exited), want, "{name}");
    }
}
