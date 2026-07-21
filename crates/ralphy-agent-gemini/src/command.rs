//! Building the headless `gemini` invocation: resolving a binary npm installs
//! without an executable extension (ADR-0043 D16), fixing the argv that refuses
//! this vendor's default blast radius (D2/D12), and scrubbing every inherited
//! authentication variable outside an explicit allowlist (D7).

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::Result;

/// Mint the session id Ralphy hands the CLI with `--session-id`. A v4 UUID, so the
/// session is addressable before the child is spawned.
pub(crate) fn mint_session_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// The vendor's binary name — also its `PATH` name, unlike Cursor's.
const NAME: &str = "gemini";

/// The vendor's stdin ceiling: the CLI reads at most 8 MiB from standard input and
/// silently truncates beyond it (ADR-0043 D2). A charter that would cross this
/// must fail loudly — a truncated charter produces a plausible-looking session
/// that was never given its rules.
pub(crate) const MAX_STDIN_BYTES: usize = 8 * 1024 * 1024;

/// Locate the Gemini CLI against the real environment. `None` means the vendor is
/// not installed — `ralphy init`'s gate reports presence through this.
///
/// npm installs it on Windows as an extensionless shim plus `gemini.cmd`, and
/// under a version-managed Node on Linux into a path a non-login shell omits;
/// both cases are handled inside `locate_program` (ADR-0043 D16).
pub fn locate_gemini() -> Option<PathBuf> {
    ralphy_proc_util::locate_program(NAME)
}

/// What a `Command` is constructed with. Falls back to the bare name so the spawn
/// failure names the vendor rather than an empty path.
pub(crate) fn resolve_gemini_program() -> OsString {
    locate_gemini()
        .map(PathBuf::into_os_string)
        .unwrap_or_else(|| NAME.into())
}

/// Refuse a prompt the vendor would silently truncate (D2).
///
/// The check is on BYTES, not characters: the ceiling is a read limit, and a
/// non-ASCII charter is longer in bytes than in `chars()`.
pub(crate) fn check_stdin_ceiling(prompt: &str) -> Result<()> {
    if prompt.len() > MAX_STDIN_BYTES {
        anyhow::bail!(
            "the charter is {} bytes, over the gemini CLI's 8 MiB stdin ceiling — \
             it would arrive truncated and the session would run without its rules",
            prompt.len()
        );
    }
    Ok(())
}

/// Build the headless `gemini` command both `plan` and `execute` go through.
///
/// The charter is NEVER on argv: the assembled planning charter is ~24 KB before
/// any issue body against a Windows argv ceiling of ~32 KB, and stdin is
/// **prepended** to any argv prompt with a blank line between (D2 — the vendor's
/// own documentation states this backwards; the shipped source is authoritative).
/// So no prompt flag appears here at all: not `-p`/`--prompt`, not
/// `-i`/`--prompt-interactive`.
///
/// `--approval-mode yolo` is the non-interactive autonomy this run needs; the
/// deprecated `--yolo` spelling is not used. The vendor's native **plan mode** is
/// refused by absence: it writes its plan into a vendor-private directory whatever
/// it is instructed and whatever a policy permits, so Ralphy's planner writes
/// `.ralphy/plan.md` itself (D12).
///
/// `--skip-trust` bypasses the interactive folder-trust prompt, which is fatal
/// headless. `--output-format stream-json` selects the record stream the fold
/// reads. `--policy` carries Ralphy's own policy document, which is sovereign over
/// the user tier (D5). `--resume`/`--session-file` are absent: this adapter drives
/// one turn per invocation.
///
/// `auth_type` is the operator's declared authentication mode, read from their
/// `settings.json` as a non-secret pointer — it selects D7's allowlist, and
/// nothing else about their root is consulted.
pub(crate) fn build_gemini_command(
    session_id: &str,
    model: Option<&str>,
    work_dir: &Path,
    home: &Path,
    policy: &Path,
    auth_type: Option<&str>,
) -> Command {
    let mut cmd = Command::new(resolve_gemini_program());
    cmd.current_dir(work_dir)
        .arg("--approval-mode")
        .arg("yolo")
        .arg("--skip-trust")
        .arg("--session-id")
        .arg(session_id)
        .arg("--output-format")
        .arg("stream-json")
        .arg("--policy")
        .arg(policy)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(m) = model {
        cmd.arg("-m").arg(m);
    }
    apply_auth_env(&mut cmd, std::env::vars().map(|(k, _)| k), auth_type, home);
    cmd
}

/// The authentication variables that MAY be forwarded, per the operator's declared
/// auth mode (ADR-0043 D7).
///
/// An allowlist rather than a denylist because the failure direction matters: an
/// inherited `GOOGLE_GENAI_USE_VERTEXAI=true` from unrelated cloud tooling silently
/// redirects the run to another account and another bill, and the run still looks
/// green. An unknown or absent auth mode forwards nothing — the vendor then answers
/// with its own exit 41 and its own sentence (D6), which is the actionable failure.
pub(crate) fn allowed_auth_vars(auth_type: Option<&str>) -> &'static [&'static str] {
    match auth_type {
        Some("gemini-api-key") => &["GEMINI_API_KEY"],
        Some("vertex-ai") => &[
            "GOOGLE_GENAI_USE_VERTEXAI",
            "GOOGLE_CLOUD_PROJECT",
            "GOOGLE_CLOUD_LOCATION",
            // Application Default Credentials: a Vertex operator authenticating
            // with a service-account key file has this as their ONLY credential
            // pointer. Scrubbing it drops them to exit 41 on every run — the
            // "wrongly dropped" direction the allowlist is supposed to avoid.
            "GOOGLE_APPLICATION_CREDENTIALS",
        ],
        // `oauth-personal` and `cloud-shell` authenticate out of band; no
        // environment variable is theirs to forward.
        _ => &[],
    }
}

/// Every authentication-relevant name in `parent` that is NOT in `keep` — what the
/// child's environment must have removed.
///
/// The namespaces are matched by PREFIX (`GEMINI_`, `GOOGLE_GENAI_`,
/// `GOOGLE_CLOUD_`) plus two exact names, so a variable the vendor adds later is
/// scrubbed by default rather than forwarded by default.
///
/// Matching is CASE-INSENSITIVE, unconditionally. Windows environment lookup is
/// case-insensitive — Node resolves `process.env.GOOGLE_GENAI_USE_VERTEXAI`
/// against a variable stored as `google_genai_use_vertexai` — so a case-sensitive
/// filter would let exactly that spelling survive the scrub while remaining fully
/// effective for the child. Harmless on Unix, where such a name is a different
/// variable the vendor does not read.
pub(crate) fn scrubbed_names<'a>(
    parent: impl Iterator<Item = &'a str>,
    keep: &[&str],
) -> Vec<String> {
    parent
        .filter(|n| {
            let u = n.to_ascii_uppercase();
            u.starts_with("GEMINI_")
                || u.starts_with("GOOGLE_GENAI_")
                || u.starts_with("GOOGLE_CLOUD_")
                || u == "GOOGLE_API_KEY"
                || u == "GOOGLE_APPLICATION_CREDENTIALS"
        })
        .filter(|n| {
            let u = n.to_ascii_uppercase();
            !keep.iter().any(|k| k.eq_ignore_ascii_case(&u))
        })
        .map(str::to_string)
        .collect()
}

/// Apply D7's allowlist to `cmd`, then point the child at Ralphy's owned root.
///
/// **Never `env_clear()`**: the child is a Node process and needs `PATH`,
/// `SystemRoot`, `APPDATA` and friends to start at all. Removal is per name.
///
/// `GEMINI_CLI_HOME` is set LAST and unconditionally — it is the D4 containment,
/// and it must survive the scrub that its own `GEMINI_` prefix would otherwise
/// catch.
pub(crate) fn apply_auth_env<I, S>(
    cmd: &mut Command,
    parent: I,
    auth_type: Option<&str>,
    home: &Path,
) where
    I: Iterator<Item = S>,
    S: AsRef<str>,
{
    let keep = allowed_auth_vars(auth_type);
    let names: Vec<String> = parent.map(|s| s.as_ref().to_string()).collect();
    for name in scrubbed_names(names.iter().map(String::as_str), keep) {
        cmd.env_remove(&name);
    }
    cmd.env("GEMINI_CLI_HOME", home);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(cmd: &Command) -> Vec<String> {
        cmd.get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    fn env_of(cmd: &Command, key: &str) -> Option<Option<String>> {
        cmd.get_envs()
            .find(|(k, _)| *k == key)
            .map(|(_, v)| v.map(|v| v.to_string_lossy().into_owned()))
    }

    /// D2: the charter rides stdin. Every flag that would put a prompt on argv —
    /// or resume a stored session instead of driving this one — is absent.
    #[test]
    fn argv_never_carries_a_prompt_flag() {
        let cmd = build_gemini_command(
            "s1",
            None,
            Path::new("/repo"),
            Path::new("/ws/.ralphy/gemini-home"),
            Path::new("/ws/.ralphy/gemini-home/ralphy-policy.toml"),
            Some("gemini-api-key"),
        );
        let args = argv(&cmd);
        for flag in [
            "-p",
            "--prompt",
            "-i",
            "--prompt-interactive",
            "--resume",
            "--session-file",
        ] {
            assert!(
                !args.iter().any(|a| a == flag),
                "prompt/resume flag {flag} reached argv: {args:?}"
            );
        }
        // Nothing on the argv is charter-sized prose.
        assert!(
            args.iter().all(|a| a.len() < 128),
            "a prompt-shaped argument reached argv: {args:?}"
        );
    }

    /// D12: the vendor's native plan mode writes into a vendor-private directory
    /// regardless of instruction, so it is refused by absence — and the autonomy
    /// flag is the current `--approval-mode yolo`, not the deprecated `--yolo`.
    #[test]
    fn argv_never_carries_plan_mode() {
        let cmd = build_gemini_command(
            "s1",
            None,
            Path::new("/repo"),
            Path::new("/home"),
            Path::new("/home/ralphy-policy.toml"),
            Some("gemini-api-key"),
        );
        let args = argv(&cmd);
        let i = args
            .iter()
            .position(|a| a == "--approval-mode")
            .unwrap_or_else(|| panic!("--approval-mode must be requested: {args:?}"));
        assert_eq!(args[i + 1], "yolo", "argv: {args:?}");
        assert!(
            !args.iter().any(|a| a.contains("plan")),
            "the vendor's native plan mode must never be selected: {args:?}"
        );
        assert!(
            !args.iter().any(|a| a == "--yolo"),
            "the deprecated spelling must not be used: {args:?}"
        );
        assert!(args.iter().any(|a| a == "--skip-trust"), "argv: {args:?}");
        let i = args.iter().position(|a| a == "--output-format").unwrap();
        assert_eq!(args[i + 1], "stream-json", "argv: {args:?}");
        let i = args.iter().position(|a| a == "--session-id").unwrap();
        assert_eq!(args[i + 1], "s1", "argv: {args:?}");
        // `-m` is present only when Ralphy has a preference.
        assert!(!args.iter().any(|a| a == "-m"), "argv: {args:?}");
        let pinned = build_gemini_command(
            "s1",
            Some("gemini-3.1-pro"),
            Path::new("/repo"),
            Path::new("/home"),
            Path::new("/home/ralphy-policy.toml"),
            Some("gemini-api-key"),
        );
        let args = argv(&pinned);
        let i = args.iter().position(|a| a == "-m").unwrap();
        assert_eq!(args[i + 1], "gemini-3.1-pro", "argv: {args:?}");
    }

    /// #255 AC1's other half: a detected revocation must never be answered by
    /// QUIETLY ASKING FOR LESS. The argv keeps requesting full autonomy on every
    /// invocation, and no second spelling of the request exists anywhere in the
    /// crate's production source for a future fallback to reach for.
    ///
    /// Comment lines are stripped before the source scan: the doc comments here
    /// and in `outcome.rs` legitimately NAME the flags they refuse, and counting
    /// prose would make the pin unmaintainable rather than sharp.
    #[test]
    fn autonomy_argv_is_never_downgraded() {
        let cmd = build_gemini_command(
            "s1",
            None,
            Path::new("/repo"),
            Path::new("/home"),
            Path::new("/home/ralphy-policy.toml"),
            None,
        );
        let args = argv(&cmd);
        let i = args
            .iter()
            .position(|a| a == "--approval-mode")
            .unwrap_or_else(|| panic!("autonomy must still be requested: {args:?}"));
        assert_eq!(args[i + 1], "yolo", "argv: {args:?}");
        assert!(
            args.iter().any(|a| a == "--skip-trust"),
            "the trust prompt is fatal headless: {args:?}"
        );

        let code: String = [
            include_str!("command.rs"),
            include_str!("outcome.rs"),
            include_str!("revocation.rs"),
            include_str!("lib.rs"),
        ]
        .map(|s| {
            s.split("#[cfg(test)]")
                .next()
                .unwrap()
                .lines()
                .filter(|l| !l.trim_start().starts_with("//"))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .join("\n");
        assert_eq!(
            code.matches(concat!("--approval-", "mode")).count(),
            1,
            "exactly one place asks for an approval mode — a second would be the \
             work-around this issue forbids"
        );
        for downgrade in ["auto_edit", concat!("--", "yolo\"")] {
            assert!(
                !code.contains(downgrade),
                "no weaker autonomy spelling may exist in production source: {downgrade}"
            );
        }
    }

    /// The child runs where Ralphy put it — a builder that dropped `work_dir`
    /// would plan one repository and edit another.
    #[test]
    fn the_child_runs_from_the_workspace_root() {
        let cmd = build_gemini_command(
            "s1",
            None,
            Path::new("/repo"),
            Path::new("/home"),
            Path::new("/home/ralphy-policy.toml"),
            Some("gemini-api-key"),
        );
        assert_eq!(cmd.get_current_dir(), Some(Path::new("/repo")));
    }

    /// D4: the child is pointed at the root Ralphy owns, and the operator's own
    /// `~/.gemini` appears nowhere on the argv or in the environment.
    #[test]
    fn the_child_is_pointed_at_the_owned_root_and_never_the_operators() {
        let owned = Path::new("/ws/.ralphy/gemini-home");
        let cmd = build_gemini_command(
            "s1",
            None,
            Path::new("/ws"),
            owned,
            Path::new("/ws/.ralphy/gemini-home/ralphy-policy.toml"),
            Some("gemini-api-key"),
        );
        assert_eq!(
            env_of(&cmd, "GEMINI_CLI_HOME"),
            Some(Some(owned.display().to_string())),
            "GEMINI_CLI_HOME must name the owned root"
        );

        // Asserting "the operator's root is absent" against THIS function would be
        // tautological — it never sees `operator_root()`. What can go wrong is one
        // level up, at the only place the `home` argument is chosen: a call site
        // that passed the operator's root, or `root::operator_root()` directly,
        // would isolate nothing. Pin that instead, on the source.
        // (The whole file, not the production half: `lib.rs` carries a `#[cfg(test)]`
        // helper ABOVE these call sites, so splitting on that marker would cut the
        // very lines under assertion. Its test module calls no builder.)
        let lib = include_str!("lib.rs");
        assert_eq!(
            lib.matches(concat!("build_gemini_", "command(")).count(),
            2,
            "exactly two call sites (plan, execute) — a third would need its own \
             root argument audited"
        );
        // Both pass the root `prepare_root` just ensured, and nothing else.
        assert_eq!(
            lib.matches("&root.home,").count(),
            2,
            "both call sites must point the child at the root ralphy just ensured"
        );
        // The operator's root IS read there — for the non-secret auth pointer and
        // their deny rules — but it must never become a child's `home`.
        assert_eq!(
            lib.matches(concat!("operator_", "root()")).count(),
            1,
            "one read of the operator's root, bound once"
        );
        for handed_to_child in [
            concat!("build_gemini_", "command(\n                &session_id,\n                model,\n                ws.repo_root(),\n                &operator"),
            concat!("root::operator_", "root()?,"),
        ] {
            assert!(
                !lib.contains(handed_to_child),
                "lib.rs must never hand the operator's own root to a child (D4)"
            );
        }
    }

    /// D7's failure direction, exactly: an inherited VertexAI flag from unrelated
    /// cloud tooling would silently redirect a `gemini-api-key` operator's run to
    /// another account, and the run would still look green.
    #[test]
    fn an_inherited_vertexai_flag_is_scrubbed() {
        let parent = [
            "GOOGLE_GENAI_USE_VERTEXAI",
            "GOOGLE_CLOUD_PROJECT",
            "GOOGLE_API_KEY",
            "GEMINI_API_KEY",
            // Not in any scrubbed namespace: the child is Node and needs these.
            "PATH",
            "SystemRoot",
        ];
        let mut cmd = Command::new("x");
        apply_auth_env(
            &mut cmd,
            parent.iter().copied(),
            Some("gemini-api-key"),
            Path::new("/owned"),
        );

        let removed: Vec<&str> = cmd
            .get_envs()
            .filter(|(_, v)| v.is_none())
            .map(|(k, _)| k.to_str().unwrap())
            .collect();
        for name in [
            "GOOGLE_GENAI_USE_VERTEXAI",
            "GOOGLE_CLOUD_PROJECT",
            "GOOGLE_API_KEY",
        ] {
            assert!(
                removed.contains(&name),
                "{name} must be scrubbed: {removed:?}"
            );
        }
        // What survives from those namespaces is exactly the allowlist plus the
        // containment variable.
        let mut kept: Vec<&str> = parent
            .iter()
            .copied()
            .filter(|n| !removed.contains(n))
            .filter(|n| n.starts_with("GEMINI_") || n.starts_with("GOOGLE_"))
            .collect();
        kept.push("GEMINI_CLI_HOME");
        kept.sort();
        assert_eq!(kept, ["GEMINI_API_KEY", "GEMINI_CLI_HOME"]);
        // Never `env_clear`: the child is Node.
        assert!(!removed.contains(&"PATH"), "{removed:?}");
        assert!(!removed.contains(&"SystemRoot"), "{removed:?}");
        assert_eq!(
            env_of(&cmd, "GEMINI_CLI_HOME"),
            Some(Some("/owned".to_string()))
        );
    }

    /// Windows resolves `process.env` case-insensitively, so a variable stored as
    /// `google_genai_use_vertexai` is fully effective for the Node child — and a
    /// case-sensitive filter would let exactly that spelling through the scrub.
    #[test]
    fn the_scrub_is_case_insensitive() {
        let scrubbed = scrubbed_names(
            [
                "google_genai_use_vertexai",
                "Google_Cloud_Project",
                "gemini_api_key",
                "PATH",
            ]
            .into_iter(),
            allowed_auth_vars(Some("gemini-api-key")),
        );
        assert_eq!(
            scrubbed,
            ["google_genai_use_vertexai", "Google_Cloud_Project"],
            "lowercase auth variables must be scrubbed, and the allowlist must \
             match case-insensitively too"
        );
        assert!(!scrubbed.iter().any(|n| n == "PATH"));
    }

    /// The allowlist per auth mode, including the two that own no variable at all.
    #[test]
    fn the_allowlist_follows_the_declared_auth_mode() {
        assert_eq!(
            allowed_auth_vars(Some("gemini-api-key")),
            ["GEMINI_API_KEY"]
        );
        assert_eq!(
            allowed_auth_vars(Some("vertex-ai")),
            [
                "GOOGLE_GENAI_USE_VERTEXAI",
                "GOOGLE_CLOUD_PROJECT",
                "GOOGLE_CLOUD_LOCATION",
                // ADC: a service-account key file is a Vertex operator's ONLY
                // credential pointer. Dropping it is exit 41 on every run.
                "GOOGLE_APPLICATION_CREDENTIALS"
            ]
        );
        for mode in [
            Some("oauth-personal"),
            Some("cloud-shell"),
            Some("junk"),
            None,
        ] {
            assert!(
                allowed_auth_vars(mode).is_empty(),
                "{mode:?} owns no forwardable variable"
            );
        }
        // A vertex-ai operator keeps their own four — INCLUDING the ADC pointer —
        // and loses the API-key namespace that would silently reroute the run.
        let scrubbed = scrubbed_names(
            [
                "GOOGLE_GENAI_USE_VERTEXAI",
                "GOOGLE_CLOUD_PROJECT",
                "GOOGLE_APPLICATION_CREDENTIALS",
                "GEMINI_API_KEY",
                "GOOGLE_API_KEY",
                "HOME",
            ]
            .into_iter(),
            allowed_auth_vars(Some("vertex-ai")),
        );
        assert_eq!(scrubbed, ["GEMINI_API_KEY", "GOOGLE_API_KEY"]);
    }

    /// D2: a charter the vendor would silently truncate must fail loudly.
    #[test]
    fn a_charter_over_the_stdin_ceiling_fails_loudly() {
        check_stdin_ceiling("a small charter").expect("an ordinary charter passes");
        let huge = "x".repeat(MAX_STDIN_BYTES + 1);
        let err = check_stdin_ceiling(&huge).expect_err("over the ceiling must fail");
        let msg = err.to_string();
        assert!(msg.contains("8 MiB"), "{msg}");
        assert!(msg.contains(&(MAX_STDIN_BYTES + 1).to_string()), "{msg}");
        // Exactly at the ceiling is still accepted — the limit is a read cap.
        check_stdin_ceiling(&"x".repeat(MAX_STDIN_BYTES)).expect("the boundary is inclusive");
    }

    /// ADR-0040 C1: naming the bare binary in a `Command` constructor fails on
    /// Windows, where npm ships this CLI as an extensionless shim plus a `.cmd`
    /// (D16). Fragments assembled with `concat!` so this cannot match itself.
    #[test]
    fn no_direct_command_new() {
        let production = include_str!("command.rs")
            .split("#[cfg(test)]")
            .next()
            .unwrap();
        assert!(
            !production.contains(concat!("Command::", "new(\"")),
            "resolve_gemini_program is the only way to name the binary"
        );
        assert_eq!(
            production.matches(concat!("Command::", "new(")).count(),
            1,
            "one constructor, and it takes the resolved program"
        );
    }

    #[test]
    fn mint_session_id_is_a_fresh_uuid() {
        let a = mint_session_id();
        assert_ne!(a, mint_session_id());
        assert_eq!(a.len(), 36, "not a hyphenated UUID: {a}");
        assert_eq!(a.matches('-').count(), 4, "not a hyphenated UUID: {a}");
    }
}
