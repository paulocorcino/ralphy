//! Detecting the ways this vendor silently revokes the autonomy Ralphy asked for
//! (ADR-0043 D5: "detected and reported, never worked around").
//!
//! Two tiers, because the controls arrive by two different routes:
//! - **in-flight** — [`detect_revocation`] matches the child's combined log against
//!   the sentences the shipped CLI prints. This is the only tier that sees an
//!   enterprise control PUSHED from Google's management console: the bundle's
//!   `fetchAdminControls` loads them into `settings.admin` at runtime, so they are
//!   never on disk.
//! - **pre-spawn** — [`read_admin_tier`] reads the admin-owned *system settings*
//!   file (and policy directory) at its OS default path, catching a
//!   locally-provisioned control cheaply and before a child exists.
//!
//! The file tier is therefore a deliberately PARTIAL oracle; neither tier
//! subsumes the other.
//!
//! Every literal here is copied from the shipped `@google/gemini-cli` 0.51.0
//! bundle, not from the documentation — the docs state the yolo/trust interaction
//! differently from the code that enforces it.

use std::path::{Path, PathBuf};

use serde_json::Value;

/// A control the vendor announced in-flight, in the order [`detect_revocation`]
/// resolves them: a hard stop must never be reported as the milder demotion that
/// the same run also prints.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Revocation {
    /// `settings.security.disableYoloMode` or `settings.admin.secureModeEnabled`
    /// turned `--approval-mode yolo` into a `FatalConfigError` (exit 52).
    AutonomyDisabled,
    /// The folder-trust check refused the workspace (exit 55).
    UntrustedWorkspace,
    /// The administrator pinned an authentication method the run does not satisfy.
    EnforcedAuth,
    /// Tool servers are administrator-controlled: disabled outright, or the
    /// requested one is not on the allowlist.
    AdminToolServers,
    /// The approval mode was silently overridden back to the prompting default —
    /// the session keeps running, but it is no longer autonomous.
    Demoted,
}

/// Needle → variant, matched over a lowercased haystack in THIS order.
///
/// Order is load-bearing: an untrusted folder prints the demotion notice too, so
/// a first-match-wins table sorted by severity is what keeps a Strict-Mode stop
/// from being reported as mere noise.
const NEEDLES: &[(&str, Revocation)] = &[
    (
        "yolo mode is disabled by your administrator",
        Revocation::AutonomyDisabled,
    ),
    ("yolo mode is disabled by", Revocation::AutonomyDisabled),
    (
        "gemini cli is not running in a trusted directory",
        Revocation::UntrustedWorkspace,
    ),
    (
        "is enforced, but no authentication is configured",
        Revocation::EnforcedAuth,
    ),
    (
        "the enforced authentication type is",
        Revocation::EnforcedAuth,
    ),
    ("disabled by administrator", Revocation::AdminToolServers),
    (
        "not allowlisted by your administrator",
        Revocation::AdminToolServers,
    ),
    (
        r#"approval mode overridden to "default""#,
        Revocation::Demoted,
    ),
];

/// The first revocation `log` announces, or `None`.
///
/// Case-insensitive substring matching over a lowercased haystack, mirroring
/// [`crate::outcome::gemini_limit_note`] — the sentences carry no alternation a
/// regex would buy, and the vendor capitalises them inconsistently across the
/// debug logger and the thrown error.
pub(crate) fn detect_revocation(log: &str) -> Option<Revocation> {
    let hay = log.to_ascii_lowercase();
    NEEDLES
        .iter()
        .find(|(n, _)| hay.contains(n))
        .map(|(_, r)| *r)
}

/// The first WHOLE line of `log` whose lowercase form contains `needle`, trimmed.
///
/// What this buys over a paraphrase: the operator gets the vendor's own sentence,
/// including the remediation clause Ralphy would otherwise have to restate and
/// keep in sync (the trust refusal names `--skip-trust`; the Strict-Mode stop
/// names the management console URL).
///
/// ANSI colour is STRIPPED. Observed live 2026-07-21: the trust refusal arrives on
/// stderr wrapped in `ESC[31m … ESC[0m`, so a raw copy would paste escape bytes
/// into the run report and the GitHub issue the runner publishes.
pub(crate) fn vendor_line(log: &str, needle: &str) -> Option<String> {
    log.lines()
        .find(|l| l.to_ascii_lowercase().contains(needle))
        .map(strip_ansi)
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
}

/// Drop CSI escape sequences (`ESC [ … <final byte>`), which is the only form this
/// vendor's colouring uses.
fn strip_ansi(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut it = line.chars().peekable();
    while let Some(c) = it.next() {
        if c == '\u{1b}' && it.peek() == Some(&'[') {
            it.next();
            for c in it.by_ref() {
                if c.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

impl Revocation {
    /// The needle used to pull the vendor's own line back out of the log — the
    /// broadest spelling of this variant, so either capitalisation matches.
    fn line_needle(self) -> &'static str {
        match self {
            Revocation::AutonomyDisabled => "yolo mode is disabled by",
            Revocation::UntrustedWorkspace => "not running in a trusted directory",
            Revocation::EnforcedAuth => "enforced",
            Revocation::AdminToolServers => "administrator",
            Revocation::Demoted => r#"approval mode overridden to "default""#,
        }
    }

    /// The operator-facing sentence: what was revoked, which control did it, and —
    /// when the log carried one — the vendor's own words verbatim.
    pub(crate) fn message(self, exit_code: Option<i32>, log: &str) -> String {
        let mut msg = match self {
            Revocation::AutonomyDisabled => "gemini's autonomous mode is disabled by an enterprise \
                 control (`admin.secureModeEnabled` or `security.disableYoloMode`) — the run stops \
                 here and ralphy does not work around it"
                .to_string(),
            Revocation::UntrustedWorkspace => "gemini refused the workspace as untrusted (exit 55) \
                 — the folder-trust check outranks `--skip-trust`, so ralphy does not work around it"
                .to_string(),
            Revocation::EnforcedAuth => "gemini's administrator enforces an authentication method \
                 (`security.auth.enforcedType`) this run does not satisfy — reported, never worked \
                 around"
                .to_string(),
            Revocation::AdminToolServers => "gemini's tool servers are governed by your \
                 administrator — reported, never worked around"
                .to_string(),
            Revocation::Demoted => "gemini overrode its approval mode back to the prompting \
                 default — the session is no longer autonomous and will stall on the first tool \
                 call"
                .to_string(),
        };
        // Never twice: `UntrustedWorkspace` already names its dedicated code.
        if let Some(c) = exit_code {
            let tag = format!("exit {c}");
            if !msg.contains(&tag) {
                msg.push_str(&format!(" (exit {c})"));
            }
        }
        if let Some(line) = vendor_line(log, self.line_needle()) {
            msg.push_str(&format!(" — gemini said: {line}"));
        }
        msg
    }
}

/// A control read from the administrator's own settings BEFORE a child is spawned.
///
/// Each variant carries what a human needs to go change: the setting key, the
/// enforced value, the injected server names, the policy directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AdminControl {
    /// The key that disables autonomous mode, named exactly as it appears on disk.
    AutonomyDisabled(&'static str),
    /// `security.auth.enforcedType`.
    EnforcedAuth(String),
    /// The `mcpServers` keys the administrator injected, sorted.
    InjectedToolServers(Vec<String>),
    /// A system policy directory, whose rules outrank Ralphy's own document.
    AdminPolicies(PathBuf),
}

impl AdminControl {
    /// The operator-facing report line for this control.
    pub(crate) fn message(&self) -> String {
        match self {
            AdminControl::AutonomyDisabled(key) => format!(
                "gemini's autonomous mode is disabled by the administrator setting `{key}` in the \
                 system settings file — ralphy reports it and does not work around it"
            ),
            AdminControl::EnforcedAuth(t) => format!(
                "gemini's administrator enforces `security.auth.enforcedType` = `{t}` — reported, \
                 never worked around"
            ),
            AdminControl::InjectedToolServers(names) => format!(
                "gemini's administrator injected tool servers ({}) — reported, never worked around",
                names.join(", ")
            ),
            AdminControl::AdminPolicies(dir) => format!(
                "an administrator policy directory exists at {} — its rules outrank ralphy's own \
                 policy document (ADR-0043 D5)",
                dir.display()
            ),
        }
    }
}

/// Read the administrator's controls out of an already-loaded system settings
/// document. PURE — the I/O lives in [`read_admin_tier`].
///
/// Fails SOFT in every direction: a missing, unreadable or malformed document
/// yields an empty vec, never an error. An admin file Ralphy cannot parse is not
/// a reason to refuse a run the vendor would have accepted.
pub(crate) fn inspect_admin_tier(
    settings_json: Option<&str>,
    admin_policy_dir: Option<&Path>,
) -> Vec<AdminControl> {
    let mut out = Vec::new();
    if let Some(v) = settings_json.and_then(|s| serde_json::from_str::<Value>(s).ok()) {
        let flag = |path: [&str; 2]| -> bool {
            v.get(path[0])
                .and_then(|o| o.get(path[1]))
                .and_then(Value::as_bool)
                .unwrap_or(false)
        };
        if flag(["admin", "secureModeEnabled"]) {
            out.push(AdminControl::AutonomyDisabled("admin.secureModeEnabled"));
        }
        if flag(["security", "disableYoloMode"]) {
            out.push(AdminControl::AutonomyDisabled("security.disableYoloMode"));
        }
        if let Some(t) = v
            .get("security")
            .and_then(|s| s.get("auth"))
            .and_then(|a| a.get("enforcedType"))
            .and_then(Value::as_str)
        {
            out.push(AdminControl::EnforcedAuth(t.to_string()));
        }
        if let Some(servers) = v.get("mcpServers").and_then(Value::as_object) {
            let mut names: Vec<String> = servers.keys().cloned().collect();
            names.sort();
            if !names.is_empty() {
                out.push(AdminControl::InjectedToolServers(names));
            }
        }
    }
    if let Some(dir) = admin_policy_dir.filter(|d| d.exists()) {
        out.push(AdminControl::AdminPolicies(dir.to_path_buf()));
    }
    out
}

/// The administrator's settings file at its OS DEFAULT path.
///
/// `GEMINI_CLI_SYSTEM_SETTINGS_PATH` is deliberately NOT consulted:
/// [`crate::command::scrubbed_names`] strips every `GEMINI_`-prefixed variable
/// from the child, so an inherited override would reach Ralphy but never the
/// vendor — and the two would then disagree about which file governs the run.
pub(crate) fn system_settings_path() -> Option<PathBuf> {
    system_dir().map(|d| d.join("settings.json"))
}

/// The administrator's policy directory, beside the settings file.
pub(crate) fn admin_policy_dir() -> Option<PathBuf> {
    system_dir().map(|d| d.join("policies"))
}

/// The vendor's own system-wide configuration directory, per
/// `bundle/docs/cli/enterprise.md` and `bundle/docs/reference/policy-engine.md`.
fn system_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        let base = std::env::var_os("ProgramData").unwrap_or_else(|| r"C:\ProgramData".into());
        Some(PathBuf::from(base).join("gemini-cli"))
    }
    #[cfg(target_os = "macos")]
    {
        Some(PathBuf::from("/Library/Application Support/GeminiCli"))
    }
    #[cfg(all(not(windows), not(target_os = "macos")))]
    {
        Some(PathBuf::from("/etc/gemini-cli"))
    }
}

/// The pre-spawn tier against the real filesystem.
pub(crate) fn read_admin_tier() -> Vec<AdminControl> {
    let settings = system_settings_path().and_then(|p| std::fs::read_to_string(p).ok());
    inspect_admin_tier(settings.as_deref(), admin_policy_dir().as_deref())
}

#[cfg(test)]
mod tests {
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
            detect_revocation(
                "YOLO mode is enabled. All tool calls will be automatically approved."
            ),
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
        let production = include_str!("revocation.rs")
            .split("#[cfg(test)]")
            .next()
            .unwrap();
        for banned in [concat!("fs::", "write"), concat!("fs::", "remove_file")] {
            assert!(
                !production.contains(banned),
                "the admin tier is read-only; found {banned}"
            );
        }

        // Controls: an empty document, malformed JSON and a missing dir are all
        // silent — an unreadable admin file must never fail a run.
        assert!(inspect_admin_tier(Some("{}"), None).is_empty());
        assert!(inspect_admin_tier(Some("not json at all"), None).is_empty());
        assert!(inspect_admin_tier(None, None).is_empty());
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
        let production = include_str!("revocation.rs")
            .split("#[cfg(test)]")
            .next()
            .unwrap();
        assert!(
            !production.contains(concat!("var_os(\"GEMINI_", "CLI_SYSTEM_SETTINGS_PATH")),
            "the child never sees that variable; honouring it would split the oracle"
        );
        // `read_admin_tier` on this host must not panic, whatever is (not) there.
        let _ = read_admin_tier();
    }
}
