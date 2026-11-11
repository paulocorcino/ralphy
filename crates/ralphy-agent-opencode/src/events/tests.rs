use super::*;

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
    assert!(is_opencode_api_degraded(line));
}

#[test]
fn degraded_ignores_healthy_and_terminal_lines() {
    // Healthy JSON event lines and plain assistant text are not degraded.
    assert!(!is_opencode_api_degraded(
        r#"{"type":"text","text":"working on it"}"#
    ));
    assert!(!is_opencode_api_degraded("just some assistant prose"));
    // A terminal usage limit (429) is NOT degraded — it is handled as a limit.
    assert!(!is_opencode_api_degraded(
        r#"{"type":"error","name":"APIError","statusCode":429,"message":"rate limited"}"#
    ));
    // A terminal auth error is NOT degraded either.
    assert!(!is_opencode_api_degraded(
        r#"{"type":"error","name":"ProviderAuthError","message":"Not authenticated"}"#
    ));
}

// ── parse_opencode_limit ─────────────────────────────────────────────────

#[test]
fn parse_limit_apierror_429_with_reset_hint() {
    // Representative captured JSON: APIError + statusCode:429 + retryAfter field.
    let stream = concat!(
        r#"{"type":"text","text":"working"}"#,
        "\n",
        r#"{"type":"error","name":"APIError","statusCode":429,"message":"rate limited","#,
        r#""retryAfter":"2026-06-10T18:00:00Z"}"#,
        "\n",
    );
    assert_eq!(
        parse_opencode_limit(stream),
        Some(OpencodeLimit {
            reset: Some("2026-06-10T18:00:00Z".into())
        })
    );
}

#[test]
fn parse_limit_reads_a_numeric_retry_after() {
    let stream = concat!(
        r#"{"type":"error","name":"APIError","statusCode":429,"message":"rate limited","#,
        r#""retryAfter":120}"#,
        "\n",
    );
    assert_eq!(
        parse_opencode_limit(stream),
        Some(OpencodeLimit {
            reset: Some("120".into())
        })
    );
}

#[test]
fn parse_limit_reads_a_retry_after_header_in_the_message() {
    let stream = concat!(
        r#"{"type":"error","name":"APIError","statusCode":429,"message":"Too many requests. "#,
        r#"Retry-After: 45, slow down"}"#,
        "\n",
    );
    assert_eq!(
        parse_opencode_limit(stream),
        Some(OpencodeLimit {
            reset: Some("45".into())
        })
    );
}

#[test]
fn parse_limit_reads_try_again_in_from_the_message() {
    let stream = concat!(
        r#"{"type":"error","name":"APIError","statusCode":429,"message":"Rate limit exceeded. "#,
        r#"Try again in 20 minutes. Thanks"}"#,
        "\n",
    );
    assert_eq!(
        parse_opencode_limit(stream),
        Some(OpencodeLimit {
            reset: Some("20 minutes".into())
        })
    );
}

#[test]
fn parse_limit_apierror_429_without_reset_hint() {
    // APIError + 429 but no reset hint → Some(None).
    let stream = concat!(
        r#"{"type":"error","name":"APIError","statusCode":429,"message":"too many requests"}"#,
        "\n",
    );
    assert_eq!(
        parse_opencode_limit(stream),
        Some(OpencodeLimit { reset: None })
    );
}

#[test]
fn parse_limit_retryable_literal_string() {
    // Documented retryable() literal: "rate limit exceeded".
    let stream = concat!(
        r#"{"type":"error","name":"APIError","statusCode":429,"message":"Rate limit exceeded. "#,
        r#"Try again at 2026-06-10T19:00:00Z"}"#,
        "\n",
    );
    // Should detect as limit and extract a reset hint from the message.
    let result = parse_opencode_limit(stream);
    assert!(result.is_some(), "must detect as limit: {result:?}");
    // The reset hint is extracted from "try again at <value>".
    assert_eq!(
        result,
        Some(OpencodeLimit {
            reset: Some("2026-06-10T19:00:00Z".into())
        })
    );
}

#[test]
fn parse_limit_zen_usage_limit_error() {
    // Zen provider emits a *UsageLimitError name.
    let stream = r#"{"type":"error","name":"KimiUsageLimitError","message":"usage limit reached"}
"#;
    assert_eq!(
        parse_opencode_limit(stream),
        Some(OpencodeLimit { reset: None }),
        "must detect Zen *UsageLimitError, with no reset hint"
    );
}

#[test]
fn parse_limit_ignores_real_unknown_error_envelope() {
    // The exact error event captured live from opencode 1.16.2: a transient
    // backend failure, NOT a usage limit. It must not be misread as a limit.
    let stream = concat!(
        r#"{"type":"error","timestamp":1781088576836,"sessionID":"ses_x","#,
        r#""error":{"name":"UnknownError","data":{"message":"Unexpected server error. Check "#,
        r#"server logs for details.","ref":"err_7391de1e"}}}"#,
    );
    assert_eq!(
        parse_opencode_limit(stream),
        None,
        "an UnknownError backend blip is not a usage limit"
    );
    // But it IS an error event (downgrades a Done claim to Stuck on execute).
    let (_t, saw_error) = parse_opencode_events(stream);
    assert!(saw_error, "the real error envelope must flag saw_error");
}

#[test]
fn parse_limit_detects_429_in_real_error_data_envelope() {
    // A 429 carried in the opencode 1.16.2 envelope: name + statusCode +
    // retryAfter live under `error.data`, not at the top level.
    let stream = concat!(
        r#"{"type":"error","sessionID":"s","error":{"name":"APIError","data":{"statusCode":429,"#,
        r#""message":"rate limit exceeded","retryAfter":"2026-06-10T20:00:00Z"}}}"#,
    );
    assert_eq!(
        parse_opencode_limit(stream),
        Some(OpencodeLimit {
            reset: Some("2026-06-10T20:00:00Z".into())
        }),
        "must read name/statusCode/retryAfter from error.data"
    );
}

#[test]
fn parse_limit_detects_kimi_403_billing_cycle_block() {
    // Kimi's billing-cycle quota block through OpenCode: an `APIError` with
    // statusCode 403 (not 429) and no reset hint — must map to Limit(None), the
    // exact live event shape that previously fell through to Stuck.
    let stream = concat!(
        r#"{"type":"error","error":{"name":"APIError","data":{"message":"You've reached your "#,
        r#"usage limit for this billing cycle. Your quota will be refreshed in the next cycle.","#,
        r#""statusCode":403,"isRetryable":false}}}"#,
    );
    assert_eq!(
        parse_opencode_limit(stream),
        Some(OpencodeLimit { reset: None }),
        "Kimi's 403 billing-cycle block is a usage limit with no reset hint"
    );
}

#[test]
fn parse_limit_plain_403_is_not_a_limit() {
    // A generic 403 (e.g. no access to a model) is a permission error, not a
    // usage limit — it must stay unrecognised so it fails as Stuck, not sleep.
    let stream = concat!(
        r#"{"type":"error","error":{"name":"APIError","data":{"message":"forbidden: model not "#,
        r#"available","statusCode":403}}}"#,
    );
    assert_eq!(parse_opencode_limit(stream), None);
}

#[test]
fn parse_limit_non_limit_status_500() {
    // A 500 error must not be classified as a limit.
    let stream = concat!(
        r#"{"type":"error","name":"APIError","statusCode":500,"message":"internal server error"}"#,
        "\n",
    );
    assert_eq!(parse_opencode_limit(stream), None);
}

#[test]
fn parse_limit_clean_stream_no_limit() {
    // A clean stream with no error events yields None.
    let stream = r#"{"type":"text","text":"working on it"}
{"type":"text","text":"RALPHY_DONE_EXIT"}
{"type":"step_finish","reason":"stop"}
"#;
    assert_eq!(parse_opencode_limit(stream), None);
}

// ── parse_opencode_log_limit ─────────────────────────────────────────────

#[test]
fn log_limit_detects_zai_5h_cap_with_reset() {
    // The exact logfmt line opencode prints to stderr under `--print-logs` when
    // Z.ai's `zai-coding-plan` (GLM) hits its 5-hour cap — captured live
    // 2026-07-11 (FinCal #71). No `{type:"error"}` JSON event accompanies it, so
    // only the log-scan (not `parse_opencode_limit`) can catch it. The reset
    // value carries a space and must survive intact.
    let log = concat!(
        "{\"type\":\"step_finish\",\"reason\":\"stop\"}\n",
        "timestamp=2026-07-11T09:48:22.735Z level=ERROR run=d9ec1918 ",
        "message=\"stream error\" providerID=zai-coding-plan modelID=glm-5.2 ",
        "session.id=ses_x error.error=\"AI_APICallError: Usage limit reached for ",
        "5 hour. Your limit will reset at 2026-07-11 22:14:08\"",
    );
    assert_eq!(
        parse_opencode_log_limit(log),
        Some(OpencodeLimit {
            reset: Some("2026-07-11 22:14:08".into())
        }),
    );
}

#[test]
fn log_limit_detects_zai_weekly_limit_exhausted_wording() {
    // Z.ai's weekly/monthly block reads `Limit Exhausted`, not the 5h cap's
    // `Usage limit reached` — captured live 2026-07-13 (FinCal #77). The verb
    // `exhausted` sits directly after `Limit` with no `quota` prefix, so the
    // matcher must carry `limit exhausted` or this slips through and the child
    // burns the full wall budget in silent backoff instead of early-killing.
    let log = concat!(
        "timestamp=2026-07-13T17:06:04.998Z level=ERROR run=d9a3c1f7 ",
        "message=\"stream error\" providerID=zai-coding-plan modelID=glm-5.2 ",
        "session.id=ses_x error.error=\"AI_APICallError: Weekly/Monthly Limit ",
        "Exhausted. Your limit will reset at 2026-07-18 11:26:07\"",
    );
    assert_eq!(
        parse_opencode_log_limit(log),
        Some(OpencodeLimit {
            reset: Some("2026-07-18 11:26:07".into())
        }),
    );
}

#[test]
fn log_limit_detects_kimi_billing_cycle_without_reset() {
    // Kimi's billing-cycle block as it reads in the logfmt log: a usage limit
    // with no reset timestamp → `Some(None)`.
    let log = concat!(
        "timestamp=2026-07-09T23:19:01.732Z level=ERROR message=\"stream error\" ",
        "providerID=kimi-for-coding error.error=\"AI_APICallError: You've reached ",
        "your usage limit for this billing cycle. Your quota will be refreshed in ",
        "the next cycle.\"",
    );
    assert_eq!(
        parse_opencode_log_limit(log),
        Some(OpencodeLimit { reset: None })
    );
}

#[test]
fn log_limit_is_model_agnostic_and_extracts_iso_reset() {
    // A provider whose wording differs from Z.ai/Kimi ("rate limit" + "try again
    // at" + an ISO-Z reset): the model-agnostic class matcher catches it with no
    // per-model phrase, and the reset that follows the phrase is captured — not
    // the logfmt line's own leading `timestamp=` instant.
    let log = concat!(
        "timestamp=2026-08-01T09:00:00.000Z level=ERROR message=\"stream error\" ",
        "error.error=\"Rate limit exceeded. Please try again at 2026-08-01T10:30:00Z\"",
    );
    assert_eq!(
        parse_opencode_log_limit(log),
        Some(OpencodeLimit {
            reset: Some("2026-08-01T10:30:00Z".into())
        }),
    );
}

#[test]
fn log_limit_seen_without_usable_datetime_is_some_none() {
    // A usage limit whose only hint is relative ("in 30 minutes", no datetime):
    // the limit is detected but there is no schedulable reset, so it maps to
    // `Some(None)` — the caller then retries on the synthetic cadence rather than
    // parking on an unparseable string.
    let log = "level=ERROR error.error=\"Usage limit reached. Try again in 30 minutes.\"";
    assert_eq!(
        parse_opencode_log_limit(log),
        Some(OpencodeLimit { reset: None })
    );
}

#[test]
fn log_limit_ignores_ordinary_and_non_limit_error_lines() {
    // An INFO runtime line and a non-limit ERROR (transient backend blip) must
    // not be misread as a usage limit.
    let log = concat!(
        "{\"type\":\"text\",\"text\":\"working\"}\n",
        "timestamp=2026-07-11T09:47:41.590Z level=INFO message=\"llm runtime ",
        "selected\" llm.provider=zai-coding-plan llm.model=glm-5.2\n",
        "timestamp=2026-07-11T09:48:22.735Z level=ERROR message=\"stream error\" ",
        "error.error=\"AI_APICallError: Unexpected server error\"",
    );
    assert_eq!(parse_opencode_log_limit(log), None);
}

// ── parse_opencode_events ────────────────────────────────────────────────

#[test]
fn parse_extracts_text_parts() {
    let stream = "{\"type\":\"step_start\",\"snapshot\":\"abc\"}\n\
                  {\"type\":\"text\",\"text\":\"working on it\"}\n\
                  {\"type\":\"text\",\"text\":\"RALPHY_DONE_EXIT\"}\n\
                  {\"type\":\"step_finish\",\"reason\":\"stop\"}\n";
    let (text, saw_error) = parse_opencode_events(stream);
    assert!(text.contains("working on it"), "text: {text:?}");
    assert!(text.contains("RALPHY_DONE_EXIT"), "text: {text:?}");
    assert!(!saw_error);
}

#[test]
fn parse_flags_error_event() {
    let stream = "{\"type\":\"text\",\"text\":\"trying\"}\n\
                  {\"type\":\"error\",\"name\":\"APIError\",\"statusCode\":500}\n";
    let (text, saw_error) = parse_opencode_events(stream);
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
    let (text, saw_error) = parse_opencode_events(stream);
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
    let (_text, saw_error) = parse_opencode_events(stream);
    assert!(saw_error, "a nested error part must flag saw_error");
}

#[test]
fn parse_limit_detects_429_in_nested_part() {
    // The limit scan must read name/statusCode/retryAfter from `part` too.
    let stream = concat!(
        r#"{"type":"error","sessionID":"s","part":{"type":"error","name":"APIError","#,
        r#""statusCode":429,"message":"rate limited","retryAfter":"2026-06-10T18:00:00Z"}}"#,
        "\n",
    );
    assert_eq!(
        parse_opencode_limit(stream),
        Some(OpencodeLimit {
            reset: Some("2026-06-10T18:00:00Z".into())
        }),
        "must detect a 429 nested under part and extract the reset"
    );
}

#[test]
fn parse_tolerates_unparseable_lines() {
    let stream = "not json at all\n\
                  {\"type\":\"text\",\"text\":\"kept\"}\n\
                  \n";
    let (text, saw_error) = parse_opencode_events(stream);
    assert_eq!(text.trim(), "kept");
    assert!(!saw_error);
}
