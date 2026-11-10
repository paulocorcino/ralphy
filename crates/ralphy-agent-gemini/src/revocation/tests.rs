use super::*;

/// The verbatim `FatalConfigError` message the bundle throws when
/// `settings.security?.disableYoloMode || settings.admin?.secureModeEnabled`
/// meets `--approval-mode yolo` (`bundle/gemini-EVKJWIDN.js:21186`, read
/// 2026-07-21).
const ADMIN_YOLO_STOP: &str = "YOLO mode is disabled by your administrator. To enable it, \
     please request an update to the settings at: https://goo.gle/manage-gemini-cli";

/// Strict Mode is a NAMED stop that reproduces the vendor's own control names —
/// not the generic "check ralphy's owned root" that exit 52 otherwise means.
#[test]
fn strict_mode_is_a_named_stop_not_a_config_error() {
    assert_eq!(
        detect_revocation(ADMIN_YOLO_STOP),
        Some(Revocation::AutonomyDisabled)
    );
    let msg = Revocation::AutonomyDisabled.message(Some(52), ADMIN_YOLO_STOP);
    for needle in [
        "secureModeEnabled",
        "disableYoloMode",
        "ralphy does not work around it",
        "exit 52",
        "goo.gle/manage-gemini-cli",
    ] {
        assert!(msg.contains(needle), "{needle} missing from {msg:?}");
    }
    // The debug-logger spellings of the same gate reach the same verdict.
    for line in [
        r#"YOLO mode is disabled by "secureModeEnabled" setting."#,
        r#"YOLO mode is disabled by the "disableYolo" setting."#,
    ] {
        assert_eq!(
            detect_revocation(line),
            Some(Revocation::AutonomyDisabled),
            "{line}"
        );
    }
    // The discriminating control: the routine preamble announces the OPPOSITE
    // and must not be read as a revocation.
    assert_eq!(
        detect_revocation("YOLO mode is enabled. All tool calls will be automatically approved."),
        None
    );
    assert_eq!(detect_revocation(""), None);
}

/// Severity order: the same untrusted run prints BOTH the trust refusal and the
/// demotion notice, and reporting the milder one loses the diagnosis.
#[test]
fn the_hardest_revocation_in_the_log_wins() {
    let both = "Approval mode overridden to \"default\" because the current folder is not \
                trusted.\nGemini CLI is not running in a trusted directory. To proceed, either \
                use `--skip-trust`, …";
    assert_eq!(
        detect_revocation(both),
        Some(Revocation::UntrustedWorkspace)
    );
    // …and each variant on its own still resolves to itself.
    for (log, want) in [
        (
            "The enforced authentication type is 'vertex-ai', but the current type is \
             'gemini-api-key'.",
            Revocation::EnforcedAuth,
        ),
        (
            "The auth type 'vertex-ai' is enforced, but no authentication is configured.",
            Revocation::EnforcedAuth,
        ),
        (
            "MCP servers are disabled by administrator. Check admin settings or contact your \
             admin.",
            Revocation::AdminToolServers,
        ),
        (
            "Server `corp` is not allowlisted by your administrator. To enable it…",
            Revocation::AdminToolServers,
        ),
    ] {
        assert_eq!(detect_revocation(log), Some(want), "{log}");
    }
}

/// `vendor_line` surfaces the vendor's WHOLE sentence, so the remediation
/// clause reaches the operator without Ralphy restating it.
#[test]
fn vendor_line_returns_the_whole_matching_line() {
    let log = "Warning: 256-color support not detected.\n  Gemini CLI is not running in a \
               trusted directory. To proceed, either use `--skip-trust`, set the \
               `GEMINI_CLI_TRUST_WORKSPACE=true` environment variable.\nbye\n";
    let line = vendor_line(log, "not running in a trusted directory").expect("a line matches");
    assert!(line.starts_with("Gemini CLI is not running"), "{line:?}");
    assert!(line.contains("--skip-trust"), "{line:?}");
    assert!(!line.contains("256-color"), "{line:?}");
    assert_eq!(vendor_line(log, "no such phrase"), None);

    // The LIVE capture (2026-07-21, `gemini -p hello` from a repo root, exit
    // 55): the vendor colours this line, and the escape bytes must not reach
    // the run report the runner publishes on the issue.
    const LIVE: &str = "\u{1b}[31mGemini CLI is not running in a trusted directory. To \
         proceed, either use `--skip-trust`, set the `GEMINI_CLI_TRUST_WORKSPACE=true` \
         environment variable, or trust this directory in interactive mode. For more \
         details, see https://geminicli.com/docs/cli/trusted-folders/\u{1b}[0m";
    let live = vendor_line(LIVE, "not running in a trusted directory").expect("matches");
    assert!(!live.contains('\u{1b}'), "escape bytes survived: {live:?}");
    assert!(live.starts_with("Gemini CLI is not running"), "{live:?}");
    assert!(live.ends_with("trusted-folders/"), "{live:?}");
    assert_eq!(
        detect_revocation(LIVE),
        Some(Revocation::UntrustedWorkspace),
        "colour must not defeat detection"
    );
}

/// AC5: every enterprise-tier control the settings file carries is detected and
/// reported by name — and nothing in this module mutates or bypasses one.
#[test]
fn admin_tier_controls_are_reported_never_worked_around() {
    const DOC: &str = r#"{"admin":{"secureModeEnabled":true},
             "security":{"auth":{"enforcedType":"vertex-ai"}},
             "mcpServers":{"corp-server":{"url":"https://mcp.corp","trust":true}}}"#;
    let dir = tempfile::tempdir().unwrap();
    let controls = inspect_admin_tier(Some(DOC), Some(dir.path()));
    assert_eq!(
        controls,
        vec![
            AdminControl::AutonomyDisabled("admin.secureModeEnabled"),
            AdminControl::EnforcedAuth("vertex-ai".into()),
            AdminControl::InjectedToolServers(vec!["corp-server".into()]),
            AdminControl::AdminPolicies(dir.path().to_path_buf()),
        ],
        "one control per enterprise setting, in inspection order"
    );
    let report = controls
        .iter()
        .map(AdminControl::message)
        .collect::<Vec<_>>()
        .join("\n");
    for needle in [
        "admin.secureModeEnabled",
        "security.auth.enforcedType",
        "corp-server",
        &dir.path().display().to_string(),
    ] {
        assert!(report.contains(needle), "{needle} missing from {report}");
    }
    // Nothing here rewrites the administrator's file or drops a control: the
    // module reads, and the only write verbs it could use are absent.
    let production = crate::tests::production_text(include_str!("../revocation.rs"));
    for banned in [concat!("fs::", "write"), concat!("fs::", "remove_file")] {
        assert!(
            !production.contains(banned),
            "the admin tier is read-only; found {banned}"
        );
    }

    // Controls: an empty document, malformed JSON and a missing dir are all
    // silent — an unreadable admin file must never fail a run.
    assert_eq!(
        inspect_admin_tier(Some("{}"), None),
        Vec::<AdminControl>::new()
    );
    assert_eq!(
        inspect_admin_tier(Some("not json at all"), None),
        Vec::<AdminControl>::new()
    );
    assert_eq!(inspect_admin_tier(None, None), Vec::<AdminControl>::new());
    assert_eq!(
        inspect_admin_tier(Some(DOC), Some(&dir.path().join("nope"))).len(),
        3,
        "a policy directory that does not exist is not a control"
    );
    // The second autonomy key is read too, and named separately.
    assert_eq!(
        inspect_admin_tier(Some(r#"{"security":{"disableYoloMode":true}}"#), None),
        vec![AdminControl::AutonomyDisabled("security.disableYoloMode")]
    );
}

/// The path tier names the vendor's own documented location and never the
/// `GEMINI_`-prefixed override the child would not see.
#[test]
fn the_system_settings_path_is_the_os_default_never_the_env_override() {
    let settings = system_settings_path().expect("every supported OS has a default");
    let policies = admin_policy_dir().expect("…and a policy directory beside it");
    assert!(settings.ends_with("settings.json"), "{settings:?}");
    assert!(policies.ends_with("policies"), "{policies:?}");
    assert_eq!(settings.parent(), policies.parent());
    let production = crate::tests::production_text(include_str!("../revocation.rs"));
    assert!(
        !production.contains(concat!("var_os(\"GEMINI_", "CLI_SYSTEM_SETTINGS_PATH")),
        "the child never sees that variable; honouring it would split the oracle"
    );
    // `read_admin_tier` on this host must not panic, whatever is (not) there.
    let _ = read_admin_tier();
}
