use super::*;

/// The live charter round-trip (step 18 of the plan for #253): the assembled
/// planning charter piped on stdin with markers planted on its first and last
/// line, plus an argv prompt marker.
const CHARTER_ROUNDTRIP: &str = include_str!("../../fixtures/charter-roundtrip-2026-07-21.jsonl");

fn msg(role: &str, content: &str) -> String {
    serde_json::json!({"type": "message", "role": role, "content": content}).to_string()
}

/// The defect this exists to catch: the sentinel arrives SPLIT across two
/// delta records, so a per-record match reports a finished session as stuck.
#[test]
fn fold_joins_a_sentinel_split_across_delta_records() {
    let stdout = format!(
        "{}\n{}\n{}\n",
        msg("assistant", "all green\nRAL"),
        msg("assistant", "PHY_DONE_EXIT"),
        serde_json::json!({"type": "result", "status": "success"})
    );
    let fold = fold_gemini_stream(&stdout);
    assert!(
        ralphy_adapter_support::done_sentinel(&fold.final_text),
        "the joined text must carry the sentinel: {:?}",
        fold.final_text
    );
    // The discriminating control: matching per record finds nothing.
    assert!(!ralphy_adapter_support::done_sentinel("all green\nRAL"));
    assert!(!ralphy_adapter_support::done_sentinel("PHY_DONE_EXIT"));
    assert!(fold.saw_result);
    assert_eq!(fold.status.as_deref(), Some("success"));
}

/// A pre-flight failure ends the stream with no terminal record at all; the
/// fold must still classify rather than panic or claim success.
#[test]
fn a_missing_result_record_is_still_classified() {
    let stdout = format!("{}\n", msg("assistant", "partial work"));
    let fold = fold_gemini_stream(&stdout);
    assert!(!fold.saw_result);
    assert_eq!(fold.status, None);
    // Zero records at all — a rejection before the model was ever reached.
    let empty = fold_gemini_stream("Error: something went wrong\n");
    assert!(!empty.saw_result);
    assert!(empty.final_text.is_empty());
    // Neither is reported as a green run.
    for f in [&fold, &empty] {
        assert_ne!(
            classify_gemini_outcome(f, "", false, false, false, Some(1), None),
            Outcome::Done
        );
    }
}

/// D9: absence must never be rewritten as zero. A `result` record with no
/// `stats` key, and a stream with no `result` record at all, both leave
/// `usage` at `None` — distinct from `Some(vec![])`, which is what a run
/// that truly saw zero usage would carry.
#[test]
fn an_envelope_without_stats_carries_no_usage() {
    let stats_less = fold_gemini_stream(r#"{"type":"result","status":"success"}"#);
    assert!(stats_less.usage.is_none());

    let no_result = fold_gemini_stream(&msg("assistant", "partial work"));
    assert!(no_result.usage.is_none());
}

/// A non-ASCII charter — including an astral-plane character — must survive
/// the fold byte-exact. A fold that sliced on `char` boundaries or re-encoded
/// would corrupt exactly this payload.
#[test]
fn a_non_ascii_charter_survives_the_fold() {
    const PAYLOAD: &str = "𝄞 café 日本語 — ✅";
    let stdout = format!(
        "{}\n{}\n",
        msg("assistant", PAYLOAD),
        serde_json::json!({"type": "result", "status": "success"})
    );
    let fold = fold_gemini_stream(&stdout);
    assert_eq!(fold.final_text, PAYLOAD);
    assert_eq!(fold.final_text.as_bytes(), PAYLOAD.as_bytes());
    // Split across deltas mid-payload, the join must still be byte-exact.
    let (a, b) = PAYLOAD.split_at("𝄞 café ".len());
    let split = format!("{}\n{}\n", msg("assistant", a), msg("assistant", b));
    assert_eq!(fold_gemini_stream(&split).final_text, PAYLOAD);
}

/// D3's table, plus the two codes that prove it is not a closed set: `429`
/// (reachable because `extractErrorCode()` forwards any numeric `.code`) and
/// an unassigned number.
#[test]
fn classify_exit_maps_the_taxonomy_and_an_unknown_code() {
    for (code, want) in [
        (Some(0), ExitClass::Success),
        (Some(1), ExitClass::Generic),
        (Some(41), ExitClass::Auth),
        (Some(42), ExitClass::BadArgv),
        (Some(44), ExitClass::Sandbox),
        (Some(52), ExitClass::Config),
        (Some(53), ExitClass::TurnLimit),
        (Some(54), ExitClass::ToolFailure),
        (Some(55), ExitClass::Untrusted),
        (Some(130), ExitClass::Cancelled),
        (Some(199), ExitClass::Relaunch),
        (Some(429), ExitClass::Limit),
        (Some(999), ExitClass::Other),
        (None, ExitClass::Other),
    ] {
        assert_eq!(classify_exit(code), want, "exit {code:?}");
    }
}

/// The exit code outranks the envelope: a stream that reported success while
/// the process exited on a tool failure is not a green run.
#[test]
fn the_exit_code_outranks_the_envelope() {
    let stdout = format!(
        "{}\n{}\n",
        msg("assistant", "done\nRALPHY_DONE_EXIT"),
        serde_json::json!({"type": "result", "status": "success"})
    );
    let fold = fold_gemini_stream(&stdout);
    assert_eq!(
        classify_gemini_outcome(&fold, "", true, false, true, Some(0), None),
        Outcome::Done
    );
    assert_ne!(
        classify_gemini_outcome(&fold, "", false, false, true, Some(54), None),
        Outcome::Done,
        "exit 54 (tool failure) must not be reported as a completed run"
    );
    // A cancellation is Ralphy stopping the child, not a crash.
    assert_eq!(
        classify_gemini_outcome(&fold, "", false, false, true, Some(130), None),
        Outcome::Timeout
    );
}

/// D2.2: the vendor gives stdin a 500 ms grace timer after spawn, so the whole
/// prompt must exist BEFORE the child is created. Pinned on the source rather
/// than assumed from the API shape — `HeadlessCall::new` takes the payload by
/// value, and `crates/ralphy-adapter-support/src/headless.rs` writes it
/// immediately after spawning the reader threads (read 2026-07-21).
#[test]
fn the_prompt_is_computed_before_the_child_is_spawned() {
    let outcome_src = include_str!("../outcome.rs")
        .split("#[cfg(test)]")
        .next()
        .unwrap();
    assert_eq!(
        outcome_src
            .matches(concat!("HeadlessCall::", "new("))
            .count(),
        1,
        "one spawn site in the crate (ADR-0040 Tier 1)"
    );
    // …and it is handed a prompt the caller already owns, never a closure or a
    // reader the child could race.
    assert!(
        outcome_src.contains(concat!("HeadlessCall::", "new(cmd, prompt, timeout,")),
        "the payload must be complete at construction (D2.2's 500 ms grace timer)"
    );
    let lib_src = include_str!("../lib.rs");
    assert!(
        !lib_src.contains(concat!("HeadlessCall::", "new(")),
        "lib.rs must go through run_gemini, not spawn its own child"
    );
}

/// The vendor writes `error` as an OBJECT, in two shapes — one under a
/// `result` record, one with NO `type` field at all. A fold that read it as a
/// string, or that keyed on `type`, dropped both and the stop went mute.
#[test]
fn the_vendor_error_object_is_read_in_both_observed_shapes() {
    let under_result = r#"{"type":"result","status":"error","error":{"type":"unknown","message":"[API Error: quota]"}}"#;
    let fold = fold_gemini_stream(under_result);
    assert_eq!(
        fold.vendor_error.as_deref(),
        Some("unknown: [API Error: quota]"),
        "the `error` object under a `result` record must be read"
    );
    assert!(fold.saw_result);

    // The auth record: no `type` key whatsoever.
    let typeless = r#"{"session_id":"s1","error":{"type":"Error","message":"Please set an Auth method in your settings.json","code":41}}"#;
    let fold = fold_gemini_stream(typeless);
    assert_eq!(
        fold.vendor_error.as_deref(),
        Some("Error: Please set an Auth method in your settings.json"),
        "a typeless record carrying `error` must not be dropped"
    );
    // A bare string `error` still works, and a record with none is silent.
    assert_eq!(
        fold_gemini_stream(r#"{"type":"error","error":"boom"}"#)
            .vendor_error
            .as_deref(),
        Some("boom")
    );
    assert_eq!(
        fold_gemini_stream(r#"{"type":"result"}"#).vendor_error,
        None
    );
}

/// The role gate must be EXACT. `PROMPT_EXECUTE` itself contains the sentinel
/// and the vendor echoes the whole prompt back as the `role:"user"` record —
/// folding anything but the assistant's own words reports every execute call
/// as instantly done.
#[test]
fn the_echoed_user_prompt_never_counts_as_the_agents_answer() {
    const SENTINEL_PROMPT: &str = "…do the work then print RALPHY_DONE_EXIT";
    let terminal = serde_json::json!({"type": "result", "status": "success"});
    // Both discriminating shapes: the echoed `role:"user"` record, and a
    // ROLE-LESS `message` record — a fold that widened to `role.is_empty()`
    // would report the second as the agent's own answer.
    let roleless = serde_json::json!({"type": "message", "content": SENTINEL_PROMPT}).to_string();
    for stdout in [
        format!("{}\n{terminal}\n", msg("user", SENTINEL_PROMPT)),
        format!("{roleless}\n{terminal}\n"),
    ] {
        let fold = fold_gemini_stream(&stdout);
        assert!(
            fold.final_text.is_empty(),
            "only the assistant's own words are the answer: {:?}",
            fold.final_text
        );
        assert_ne!(
            classify_gemini_outcome(&fold, "", true, false, false, Some(0), None),
            Outcome::Done
        );
    }
}

/// D11: this vendor reserves no exit code for quota, so the TEXT is the only
/// signal a real exhaustion has — and the limit must carry `None`, because the
/// inner slot is a parsed reset hint the vendor never publishes. Putting prose
/// there makes the runner read it as a schedule and abandon the issue.
#[test]
fn a_provider_throttle_is_a_limit_with_no_reset_hint() {
    // The fold MUST carry a vendor sentence: that is the value an
    // implementation would be tempted to smuggle into the reset slot, and a
    // fold without one cannot tell the two apart.
    let fold = fold_gemini_stream(
        r#"{"type":"result","status":"error","error":{"type":"unknown","message":"[API Error: 429 quota exceeded]"}}"#,
    );
    assert!(fold.vendor_error.is_some(), "the fixture must carry prose");
    for phrase in [
        "Error: 429 Too Many Requests",
        "RESOURCE_EXHAUSTED: quota exceeded for this project",
        "you have hit a rate limit",
    ] {
        assert!(gemini_limit_note(phrase).is_some(), "{phrase}");
        assert_eq!(
            classify_gemini_outcome(&fold, phrase, false, false, false, Some(1), None),
            Outcome::Limit(None),
            "a textual throttle must be a limit with NO reset hint: {phrase}"
        );
    }
    // The documented 429 exit reaches the same place with no text at all.
    assert_eq!(
        classify_gemini_outcome(&fold, "", false, false, false, Some(429), None),
        Outcome::Limit(None)
    );
    // …and ordinary prose is not a limit.
    assert_eq!(gemini_limit_note("everything is fine"), None);
    assert_ne!(
        classify_gemini_outcome(
            &fold,
            "everything is fine",
            false,
            false,
            false,
            Some(1),
            None
        ),
        Outcome::Limit(None)
    );
}

/// D11 (#264): the CLI's own `retryWithBackoff` absorbs transient 429s
/// silently, so a run that still finishes green can carry the throttle
/// banner in its combined log. `limit` outranks `done` in
/// `ralphy_adapter_support::classify`, so an ungated textual match parks a
/// finished run for ~30 min — the gate is on `!succeeded`, not on the
/// predicate itself.
#[test]
fn a_vendor_absorbed_transient_throttle_does_not_park_a_green_run() {
    let green = fold_gemini_stream(&format!(
        "{}\n{}\n",
        msg("assistant", "all green\nRALPHY_DONE_EXIT"),
        serde_json::json!({"type": "result", "status": "success"})
    ));
    let log = "Attempt 1 failed with status 429 Too Many Requests. Retrying with backoff...\n";
    // The phrase DOES match — it is the gate, not the predicate, that changed.
    assert!(gemini_limit_note(log).is_some());
    assert_eq!(
        classify_gemini_outcome(&green, log, true, false, true, Some(0), None),
        Outcome::Done,
        "a vendor-absorbed retry banner on a green run must not become a limit"
    );
    // Discriminating control: the same phrase on a run that did NOT succeed
    // is still a limit.
    assert_eq!(
        classify_gemini_outcome(
            &fold_gemini_stream(""),
            log,
            false,
            false,
            false,
            Some(1),
            None
        ),
        Outcome::Limit(None)
    );
    // The exit code stays ungated even on an otherwise-green fold.
    assert_eq!(
        classify_gemini_outcome(&green, "", false, false, false, Some(429), None),
        Outcome::Limit(None)
    );
}

/// The two stops must stay distinct: a turn-ceiling stop (exit 53) is a
/// budget stop, not a quota stop, and neither `gemini_limit_note` nor
/// `classify_gemini_outcome` may conflate them. Converse arm pins that a
/// real quota sentence at exit 1 is a limit and never `Blocked`.
#[test]
fn a_turn_ceiling_stop_is_not_a_quota_stop() {
    let fold = fold_gemini_stream("");
    let turn_log = "FatalTurnLimitedError: reached the maximum number of turns\n";
    assert_eq!(gemini_limit_note(turn_log), None);
    match classify_gemini_outcome(&fold, turn_log, false, false, false, Some(53), None) {
        Outcome::Blocked(reason) => assert!(
            reason.to_ascii_lowercase().contains("turn ceiling"),
            "got {reason:?}"
        ),
        other => panic!("exit 53 must be a named stop, got {other:?}"),
    }
    assert_ne!(
        classify_gemini_outcome(&fold, turn_log, false, false, false, Some(53), None),
        Outcome::Limit(None)
    );

    let quota_log = "Error: quota exceeded for this project\n";
    let outcome = classify_gemini_outcome(&fold, quota_log, false, false, false, Some(1), None);
    assert_eq!(outcome, Outcome::Limit(None));
    assert!(!matches!(outcome, Outcome::Blocked(_)));
}

/// D11's ⚠ stance is the decision most likely to need revising — pinned so a
/// future edit to the ADR cannot silently drop the disclaimer this plan's
/// caveats rely on.
#[test]
fn the_limit_stance_is_documented_as_the_one_most_likely_to_be_revised() {
    const ADR: &str = include_str!("../../../../docs/adr/0043-gemini-adapter.md");
    // The prose is hard-wrapped in the file, so match phrases that do not
    // straddle a line break rather than one contiguous sentence.
    assert!(ADR.contains("most likely in this ADR to need"));
    assert!(ADR.contains("requires no reset parsing to be correct"));
    assert!(ADR.contains("Ralphy adds no retry layer"));
}

/// D5: an actionable refusal is a NAMED stop, never a silent degradation into
/// `Stuck`. Without this an enterprise Strict Mode that stripped the autonomy
/// flag is indistinguishable from a confused agent.
#[test]
fn an_actionable_exit_stops_with_a_sentence_not_a_mute_stuck() {
    let fold = fold_gemini_stream("");
    for (code, needle) in [
        (55, "untrusted"),
        (44, "sandbox"),
        (52, "configuration"),
        (42, "command line"),
    ] {
        match classify_gemini_outcome(&fold, "", false, false, false, Some(code), None) {
            Outcome::Blocked(reason) => assert!(
                reason.to_ascii_lowercase().contains(needle),
                "exit {code} must name its cause, got {reason:?}"
            ),
            other => panic!("exit {code} must be a named stop, got {other:?}"),
        }
    }
    // A plain failure keeps falling through the ladder — this must not turn
    // every non-zero exit into a `Blocked`.
    assert!(!matches!(
        classify_gemini_outcome(&fold, "", false, false, false, Some(1), None),
        Outcome::Blocked(_)
    ));
    // …and a SUCCESSFUL run never carries a stop sentence.
    let ok = fold_gemini_stream(&format!(
        "{}\n{}\n",
        msg("assistant", "done"),
        serde_json::json!({"type": "result", "status": "success"})
    ));
    assert!(!matches!(
        classify_gemini_outcome(&ok, "", true, false, true, Some(0), None),
        Outcome::Blocked(_)
    ));
}

/// The two stops that mean "the session ran out of budget" and "a tool broke",
/// which both reached the operator as a mute `Stuck` before their sentences
/// existed. A budget stop and a crash call for opposite reactions — raise the
/// ceiling versus debug the run — so collapsing them is a real loss.
#[test]
fn a_turn_ceiling_is_a_budget_stop_not_a_failure() {
    let fold = fold_gemini_stream("");
    for (code, needle) in [(53, "turn ceiling"), (54, "tool")] {
        match classify_gemini_outcome(&fold, "", false, false, false, Some(code), None) {
            Outcome::Blocked(reason) => assert!(
                reason.to_ascii_lowercase().contains(needle),
                "exit {code} must name {needle:?}, got {reason:?}"
            ),
            other => panic!("exit {code} must be a named stop, got {other:?}"),
        }
    }
    // The discriminating control: exit 1 is the vendor's generic/model failure
    // and must keep falling through the ladder, or every non-zero exit becomes
    // a `Blocked` and the distinction this test buys is worthless.
    assert!(!matches!(
        classify_gemini_outcome(&fold, "", false, false, false, Some(1), None),
        Outcome::Blocked(_)
    ));
}

/// D18: the `199` sentinel should never be observed, because the CLI's wrapper
/// re-execs itself. Observing it means that re-exec broke — a diagnosis worth
/// its own sentence rather than a fold into the unmapped catch-all.
#[test]
fn the_relaunch_sentinel_is_mapped() {
    assert_eq!(classify_exit(Some(199)), ExitClass::Relaunch);
    match classify_gemini_outcome(
        &fold_gemini_stream(""),
        "",
        false,
        false,
        false,
        Some(199),
        None,
    ) {
        Outcome::Blocked(reason) => assert!(
            reason.contains("199") && reason.to_ascii_lowercase().contains("relaunch"),
            "the sentinel must name itself, got {reason:?}"
        ),
        other => panic!("exit 199 must be a named stop, got {other:?}"),
    }
}

/// The `fold.status != Some("error")` half of `succeeded`, which no other test
/// discriminates: a clean exit code alone must not make a run green when the
/// terminal envelope says the session errored.
#[test]
fn the_envelope_status_is_honoured_when_present() {
    let stream = |status: &str| {
        format!(
            "{}\n{}\n",
            msg("assistant", "work is done\nRALPHY_DONE_EXIT"),
            serde_json::json!({"type": "result", "status": status})
        )
    };
    assert_ne!(
        classify_gemini_outcome(
            &fold_gemini_stream(&stream("error")),
            "",
            true,
            false,
            true,
            Some(0),
            None
        ),
        Outcome::Done,
        "an errored envelope must not be reported as a completed run"
    );
    // Same stream, same clean exit — only the status differs, so the assertion
    // above cannot be passing for some unrelated reason.
    assert_eq!(
        classify_gemini_outcome(
            &fold_gemini_stream(&stream("success")),
            "",
            true,
            false,
            true,
            Some(0),
            None
        ),
        Outcome::Done
    );
}

/// The vendor prints this preamble on stderr on EVERY run, successful ones
/// included (spike §"stderr is never empty", 2026-07-20) — note the YOLO line
/// arrives TWICE. A health check keyed on a non-empty stderr, or a limit
/// matcher loose enough to catch "not available", would report every healthy
/// run as degraded.
#[test]
fn the_startup_preamble_is_not_a_degraded_run() {
    const PREAMBLE: &str = "Warning: 256-color support not detected. Using a terminal with at least 256-color support is recommended…\n\
             YOLO mode is enabled. All tool calls will be automatically approved.\n\
             YOLO mode is enabled. All tool calls will be automatically approved.\n\
             Ripgrep is not available. Falling back to GrepTool.\n";
    let fold = fold_gemini_stream(&format!(
        "{}\n{}\n",
        msg("assistant", "all green\nRALPHY_DONE_EXIT"),
        serde_json::json!({"type": "result", "status": "success"})
    ));
    assert_eq!(
        classify_gemini_outcome(&fold, PREAMBLE, true, false, true, Some(0), None),
        Outcome::Done,
        "the routine preamble must not cost a healthy run its Done"
    );
    assert_eq!(gemini_limit_note(PREAMBLE), None);
    assert!(!crate::auth::is_gemini_auth_error(PREAMBLE));
}

/// The verbatim sentences the shipped 0.51.0 bundle prints for each of the
/// three silent revocations (read 2026-07-21; see the module doc of
/// `revocation.rs` for the exact bundle sites).
const ADMIN_LOG: &str = "YOLO mode is disabled by your administrator. To enable it, please \
         request an update to the settings at: https://goo.gle/manage-gemini-cli";
const UNTRUSTED_LOG: &str = "Gemini CLI is not running in a trusted directory. To proceed, \
         either use `--skip-trust`, set the `GEMINI_CLI_TRUST_WORKSPACE=true` environment \
         variable, or trust this directory in interactive mode.";
const DEMOTION_LOG: &str = "YOLO mode is enabled. All tool calls will be automatically \
         approved.\nApproval mode overridden to \"default\" because the current folder is not \
         trusted.";

/// #255 AC1: an enterprise control that disables autonomous mode is a NAMED
/// stop reproducing that control's own name — not exit 52's ordinary
/// "check ralphy's owned root", which would blame Ralphy for the enterprise.
#[test]
fn strict_mode_is_a_named_stop_not_a_config_error() {
    let fold = fold_gemini_stream("");
    match classify_gemini_outcome(&fold, ADMIN_LOG, false, false, false, Some(52), None) {
        Outcome::Blocked(reason) => {
            for needle in ["secureModeEnabled", "disableYoloMode"] {
                assert!(reason.contains(needle), "{needle} missing from {reason:?}");
            }
            assert!(
                !reason.contains(".ralphy/gemini-home"),
                "the enterprise stop must not be diagnosed as ralphy's own root: {reason:?}"
            );
        }
        other => panic!("the admin stop must be a named block, got {other:?}"),
    }
    // The discriminating control: exit 52 with an UNRELATED log keeps the
    // ordinary diagnosis, so the override is scoped to the admin needle.
    match classify_gemini_outcome(
        &fold,
        "Error: bad settings\n",
        false,
        false,
        false,
        Some(52),
        None,
    ) {
        Outcome::Blocked(reason) => assert!(
            reason.contains(".ralphy/gemini-home"),
            "an ordinary exit 52 keeps its own sentence, got {reason:?}"
        ),
        other => panic!("exit 52 must stay a named stop, got {other:?}"),
    }
}

/// #255 AC3: the untrusted-workspace stop is recognised by its dedicated exit
/// code AND surfaces the vendor's own remediation clause, so the operator gets
/// the fix rather than a Ralphy paraphrase of it.
#[test]
fn the_untrusted_stop_surfaces_the_vendors_own_sentence() {
    let fold = fold_gemini_stream("");
    match classify_gemini_outcome(&fold, UNTRUSTED_LOG, false, false, false, Some(55), None) {
        Outcome::Blocked(reason) => {
            for needle in [
                "exit 55",
                "--skip-trust",
                // These three come ONLY from the new path: the pre-existing
                // `ExitClass::Untrusted` sentence already carried `exit 55`
                // and `--skip-trust`, so asserting those alone would pass
                // against a reverted production change.
                "gemini said:",
                "GEMINI_CLI_TRUST_WORKSPACE",
                "interactive mode",
            ] {
                assert!(reason.contains(needle), "{needle} missing from {reason:?}");
            }
        }
        other => panic!("exit 55 must be a named stop, got {other:?}"),
    }
    // The code alone is sufficient: an empty log carries no vendor sentence,
    // and the exit-class diagnosis still names the code.
    match classify_gemini_outcome(&fold, "", false, false, false, Some(55), None) {
        Outcome::Blocked(reason) => {
            assert!(reason.contains("exit 55"), "{reason:?}");
            assert!(!reason.contains("gemini said:"), "{reason:?}");
        }
        other => panic!("exit 55 must be a named stop, got {other:?}"),
    }
    // The observed code is reported, never a canonical one substring-matched
    // against it: exit 5 must not be laundered into the hard-coded 55.
    let five = crate::revocation::Revocation::UntrustedWorkspace.message(Some(5), "");
    assert!(five.contains("exit 5)"), "{five:?}");
    assert!(!five.contains("exit 55"), "{five:?}");
}

/// #255 AC4: the demotion notice is a REVOCATION, not preamble noise — the
/// session kept running but is no longer autonomous. The invariant control is
/// that the same notice on a run that still went green keeps its `Done`.
#[test]
fn the_demotion_notice_is_a_revocation_not_noise() {
    assert_eq!(
        crate::revocation::detect_revocation(DEMOTION_LOG),
        Some(crate::revocation::Revocation::Demoted)
    );
    match classify_gemini_outcome(
        &fold_gemini_stream(""),
        DEMOTION_LOG,
        false,
        false,
        false,
        Some(1),
        None,
    ) {
        Outcome::Blocked(reason) => assert!(
            reason.contains("no longer autonomous"),
            "the demotion must be named, got {reason:?}"
        ),
        other => panic!("a failed demoted run must be a named stop, got {other:?}"),
    }
    // The invariant: a green run keeps its Done even when it was demoted —
    // without this, the routine preamble of an untrusted-but-successful run
    // would cost every such run its completion.
    let green = fold_gemini_stream(&format!(
        "{}\n{}\n",
        msg("assistant", "all green\nRALPHY_DONE_EXIT"),
        serde_json::json!({"type": "result", "status": "success"})
    ));
    assert_eq!(
        classify_gemini_outcome(&green, DEMOTION_LOG, true, false, true, Some(0), None),
        Outcome::Done,
        "a revocation must never flip a run that succeeded"
    );
    // …and a revocation must not shadow a provider throttle, which needs its
    // own retry schedule rather than a block.
    assert_eq!(
        classify_gemini_outcome(
            &fold_gemini_stream(""),
            &format!("{DEMOTION_LOG}\nError: 429 Too Many Requests"),
            false,
            false,
            false,
            Some(1),
            None,
        ),
        Outcome::Limit(None)
    );
}

/// The self-review's HIGH finding, from the other side of the seam: on a
/// managed host the administrator's tool-server notice is in EVERY log, so an
/// informational revocation must never outrank a diagnosis that is actually
/// about why this run stopped.
#[test]
fn an_informational_notice_never_outranks_the_exit_class() {
    const NOTICE: &str = "MCP servers are disabled by administrator. Check admin settings or \
                              contact your admin.";
    assert_eq!(
        crate::revocation::detect_revocation(NOTICE),
        Some(crate::revocation::Revocation::AdminToolServers)
    );
    assert!(!crate::revocation::Revocation::AdminToolServers.is_hard_stop());
    let fold = fold_gemini_stream("");
    // Every exit class with its own sentence keeps it, notice or no notice.
    for (code, needle) in [
        (44, "sandbox"),
        (54, "tool"),
        (42, "command line"),
        (53, "turn ceiling"),
        (52, "check ralphy's"),
    ] {
        match classify_gemini_outcome(&fold, NOTICE, false, false, false, Some(code), None) {
            Outcome::Blocked(reason) => assert!(
                reason.to_ascii_lowercase().contains(needle),
                "exit {code} lost its own diagnosis to a routine notice: {reason:?}"
            ),
            other => panic!("exit {code} must stay a named stop, got {other:?}"),
        }
    }
    // …and the notice is still surfaced where nothing better exists: exit 1
    // has no sentence of its own, so the control is named rather than mute.
    match classify_gemini_outcome(&fold, NOTICE, false, false, false, Some(1), None) {
        Outcome::Blocked(reason) => assert!(reason.contains("administrator"), "{reason:?}"),
        other => panic!("a bare failure carrying a notice must be named, got {other:?}"),
    }
    // A HARD stop still outranks the exit class — that is the whole point of
    // the exit-52 override, and this proves the split is not a blanket demotion.
    match classify_gemini_outcome(&fold, ADMIN_LOG, false, false, false, Some(52), None) {
        Outcome::Blocked(reason) => assert!(reason.contains("secureModeEnabled"), "{reason:?}"),
        other => panic!("expected the enterprise stop, got {other:?}"),
    }
}

/// A pinned id the vendor does not serve is a NAMED stop that quotes the id —
/// but it sits below a hard-stop revocation in the chain (#255's ordering).
#[test]
fn a_model_404_is_named_but_yields_to_a_hard_stop() {
    const NOT_FOUND: &str = "ModelNotFoundError: models/no-such-model is not found for API \
                                 version v1beta, or is not supported for generateContent. \
                                 { code: 404 }";
    let fold = fold_gemini_stream("");
    // Exit 1 has no sentence of its own: the 404 names the stop.
    match classify_gemini_outcome(
        &fold,
        NOT_FOUND,
        false,
        false,
        false,
        Some(1),
        Some("no-such-model"),
    ) {
        Outcome::Blocked(reason) => {
            assert!(reason.contains("no-such-model"), "{reason:?}");
            assert!(reason.contains("ModelNotFoundError"), "{reason:?}");
        }
        other => panic!("a model 404 must be a named stop, got {other:?}"),
    }
    // …and an exit class WITH its own sentence still outranks it: the 404 sits
    // BELOW `actionable_stop` in the chain, so exit 44 keeps "sandbox".
    // Without this leg the test passes under either ordering.
    match classify_gemini_outcome(
        &fold,
        NOT_FOUND,
        false,
        false,
        false,
        Some(44),
        Some("no-such-model"),
    ) {
        Outcome::Blocked(reason) => assert!(
            reason.to_ascii_lowercase().contains("sandbox"),
            "exit 44 must keep its own diagnosis: {reason:?}"
        ),
        other => panic!("expected the sandbox stop, got {other:?}"),
    }
    // A hard-stop revocation still wins — the model is not why the run died.
    let both = format!("{ADMIN_LOG}\n{NOT_FOUND}");
    match classify_gemini_outcome(
        &fold,
        &both,
        false,
        false,
        false,
        Some(52),
        Some("no-such-model"),
    ) {
        Outcome::Blocked(reason) => assert!(reason.contains("secureModeEnabled"), "{reason:?}"),
        other => panic!("expected the enterprise stop, got {other:?}"),
    }
    // A GREEN run is never blocked by a 404 quoted in its own transcript.
    let green = fold_gemini_stream(
        r#"{"type":"message","role":"assistant","content":"RALPHY_DONE_EXIT"}
{"type":"result","status":"success"}"#,
    );
    assert!(!matches!(
        classify_gemini_outcome(&green, NOT_FOUND, true, false, true, Some(0), None),
        Outcome::Blocked(_)
    ));
}

/// D-both-channels: under `stream-json` the well-typed error object rides
/// stdout while the readable prose goes to stderr, so a classifier that reads
/// either one alone is blind to exactly the failures it must name.
#[test]
fn both_channels_feed_the_diagnosis() {
    // (a) stdout only: the typed error object is read into the fold.
    let stdout_only = fold_gemini_stream(
        r#"{"type":"result","status":"error","error":{"type":"unknown","message":"[API Error: An unknown error occurred.]"}}"#,
    );
    let vendor = stdout_only
        .vendor_error
        .as_deref()
        .expect("the typed error object must reach the fold");
    assert!(
        vendor.contains("unknown"),
        "the vendor's own sentence must be preserved, got {vendor:?}"
    );

    // (b) stderr only: stdout carries NO record at all — the shape a
    // pre-provider failure leaves — and the diagnosis has to come from the
    // combined log plus the exit code.
    let empty = fold_gemini_stream("");
    assert!(!empty.saw_result && empty.vendor_error.is_none());
    match classify_gemini_outcome(
        &empty,
        "FatalTurnLimitedError: reached the maximum number of turns\n",
        false,
        false,
        false,
        Some(53),
        None,
    ) {
        Outcome::Blocked(reason) => assert!(
            reason.to_ascii_lowercase().contains("turn ceiling"),
            "a stderr-only failure must still be named, got {reason:?}"
        ),
        other => panic!("expected a named stop, got {other:?}"),
    }
}

/// D2, live: standard input is PREPENDED to the argv prompt and joined with a
/// blank line — the vendor's documentation states this backwards, and a
/// charter delivered after the argv word would be read as a trailing note.
///
/// The fixture is one real invocation (2026-07-21, gemini 0.51.0): the
/// assembled `prompt.plan.gemini.md` piped on stdin with `RALPHY_CHARTER_HEAD_9F2A`
/// planted on its first line and the non-ASCII payload plus
/// `RALPHY_CHARTER_TAIL_7B31` on its last, and `-p "RALPHY_ARGV_TAIL_51CD"`.
#[test]
fn stdin_arrives_before_the_argv_prompt() {
    let user = CHARTER_ROUNDTRIP
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l.trim()).ok())
        .find(|v| v.get("role").and_then(Value::as_str) == Some("user"))
        .expect("the fixture must carry the user record");
    let text = record_text(&user);

    assert!(
        text.starts_with("RALPHY_CHARTER_HEAD_9F2A"),
        "stdin must come FIRST: {:?}",
        &text[..text.len().min(120)]
    );
    assert!(
        text.ends_with("RALPHY_ARGV_TAIL_51CD"),
        "the argv prompt must come LAST: {:?}",
        &text[text.len().saturating_sub(120)..]
    );
    // Exactly one blank line joins the two, and the astral-plane payload
    // planted just before the stdin tail marker survived the round trip.
    assert!(
        text.contains("𝄞 café 日本語 — ✅ RALPHY_CHARTER_TAIL_7B31\n\nRALPHY_ARGV_TAIL_51CD"),
        "stdin and argv must be joined by exactly one blank line, with the \
             non-ASCII payload intact"
    );
}

/// The same fixture proves the argv carried no prompt flag other than the one
/// marker this probe deliberately planted: everything else the session saw
/// arrived on stdin.
#[test]
fn the_roundtrip_fixture_carries_the_whole_charter() {
    let user = CHARTER_ROUNDTRIP
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l.trim()).ok())
        .find(|v| v.get("role").and_then(Value::as_str) == Some("user"))
        .expect("the fixture must carry the user record");
    let text = record_text(&user);
    assert!(
        text.len() > 23_000,
        "the whole ~24 KB charter must have arrived, got {} bytes",
        text.len()
    );
}
