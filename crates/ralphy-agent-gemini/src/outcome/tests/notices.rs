use super::*;

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
        other @ (Outcome::Done | Outcome::Timeout | Outcome::Stuck | Outcome::Limit(_)) => {
            panic!("the admin stop must be a named block, got {other:?}")
        }
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
        other @ (Outcome::Done | Outcome::Timeout | Outcome::Stuck | Outcome::Limit(_)) => {
            panic!("exit 52 must stay a named stop, got {other:?}")
        }
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
        other @ (Outcome::Done | Outcome::Timeout | Outcome::Stuck | Outcome::Limit(_)) => {
            panic!("exit 55 must be a named stop, got {other:?}")
        }
    }
    // The code alone is sufficient: an empty log carries no vendor sentence,
    // and the exit-class diagnosis still names the code.
    match classify_gemini_outcome(&fold, "", false, false, false, Some(55), None) {
        Outcome::Blocked(reason) => {
            assert!(reason.contains("exit 55"), "{reason:?}");
            assert!(!reason.contains("gemini said:"), "{reason:?}");
        }
        other @ (Outcome::Done | Outcome::Timeout | Outcome::Stuck | Outcome::Limit(_)) => {
            panic!("exit 55 must be a named stop, got {other:?}")
        }
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
        other @ (Outcome::Done | Outcome::Timeout | Outcome::Stuck | Outcome::Limit(_)) => {
            panic!("a failed demoted run must be a named stop, got {other:?}")
        }
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
            other @ (Outcome::Done | Outcome::Timeout | Outcome::Stuck | Outcome::Limit(_)) => {
                panic!("exit {code} must stay a named stop, got {other:?}")
            }
        }
    }
    // …and the notice is still surfaced where nothing better exists: exit 1
    // has no sentence of its own, so the control is named rather than mute.
    match classify_gemini_outcome(&fold, NOTICE, false, false, false, Some(1), None) {
        Outcome::Blocked(reason) => assert!(reason.contains("administrator"), "{reason:?}"),
        other @ (Outcome::Done | Outcome::Timeout | Outcome::Stuck | Outcome::Limit(_)) => {
            panic!("a bare failure carrying a notice must be named, got {other:?}")
        }
    }
    // A HARD stop still outranks the exit class — that is the whole point of
    // the exit-52 override, and this proves the split is not a blanket demotion.
    match classify_gemini_outcome(&fold, ADMIN_LOG, false, false, false, Some(52), None) {
        Outcome::Blocked(reason) => assert!(reason.contains("secureModeEnabled"), "{reason:?}"),
        other @ (Outcome::Done | Outcome::Timeout | Outcome::Stuck | Outcome::Limit(_)) => {
            panic!("expected the enterprise stop, got {other:?}")
        }
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
        other @ (Outcome::Done | Outcome::Timeout | Outcome::Stuck | Outcome::Limit(_)) => {
            panic!("a model 404 must be a named stop, got {other:?}")
        }
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
        other @ (Outcome::Done | Outcome::Timeout | Outcome::Stuck | Outcome::Limit(_)) => {
            panic!("expected the sandbox stop, got {other:?}")
        }
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
        other @ (Outcome::Done | Outcome::Timeout | Outcome::Stuck | Outcome::Limit(_)) => {
            panic!("expected the enterprise stop, got {other:?}")
        }
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
        other @ (Outcome::Done | Outcome::Timeout | Outcome::Stuck | Outcome::Limit(_)) => {
            panic!("expected a named stop, got {other:?}")
        }
    }
}

/// The terminal `result` record names the model the vendor actually used, so it
/// replaces the one `init` announced; the session id has only `init` to come from.
#[test]
fn the_result_model_replaces_the_init_model_and_the_session_id_stays() {
    let fold = fold_gemini_stream(&format!(
        "{}\n{}\n",
        serde_json::json!({"type": "init", "session_id": "s-1", "model": "model-a"}),
        serde_json::json!({"type": "result", "status": "success", "model": "model-b"})
    ));
    assert_eq!(fold.model.as_deref(), Some("model-b"));
    assert_eq!(fold.session_id.as_deref(), Some("s-1"));
}

/// The first sentence the vendor gave is the one the operator sees; a later
/// record's error does not replace it.
#[test]
fn the_first_vendor_error_is_kept() {
    let fold = fold_gemini_stream(&format!(
        "{}\n{}\n",
        serde_json::json!({"type": "result", "status": "error", "error": {"message": "first"}}),
        serde_json::json!({"error": {"message": "second"}})
    ));
    assert_eq!(fold.vendor_error.as_deref(), Some("first"));
}
