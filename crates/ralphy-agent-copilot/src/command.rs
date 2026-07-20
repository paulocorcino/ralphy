//! Building the headless `copilot` invocation. A single point that fixes the
//! argv, mints the session id, and shrinks the blast radius Copilot ships with
//! on by default (ADR-0041 D7/D8).

use std::path::Path;
use std::process::{Command, Stdio};

use ralphy_adapter_support::resolve_program;

/// Mint the session id Ralphy hands the CLI with `--session-id`. A v4 UUID: the
/// vendor's `sessions.id` primary key is a UUID, so the later usage slice (D10)
/// can read the row back by key instead of diffing a session store. Not a ULID —
/// 48 of a ULID's 128 bits are a timestamp, which would misrepresent the shape.
pub(crate) fn mint_session_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// Build the headless `copilot` command both `plan` and `execute` go through.
///
/// The prompt is NEVER passed on argv and there is **no `-p`**: `prompt.execute.md`
/// is 23 884 bytes before the issue body is appended, against a Windows argv
/// ceiling of ~32 KB, so stdin is the only safe channel (ADR-0041 D2; spike C1
/// probe P3 verified a 24 250-byte payload arriving intact with no `-p` at all).
///
/// `--allow-all-tools` is *required* for non-interactive mode.
/// `--output-format json` selects the JSONL stream the parser reads (and avoids
/// the un-stress-tested `text` renderer under redirection).
/// `--session-id` is Ralphy's own minted id, so the session is addressable before
/// the child is even spawned.
///
/// Five flags shrink Copilot's default blast radius (D7), because each one is a
/// capability Ralphy's ethos forbids outright: `--no-remote` /
/// `--no-remote-export` (no remote control of, or export of, the session to
/// GitHub web/mobile), `--disable-builtin-mcps` (the bundled GitHub MCP server
/// holds the operator's token and can open PRs), `--no-auto-update` (a run must
/// not mutate its own toolchain mid-flight), `--no-ask-user` (disables the
/// `ask_user` tool outright — stronger than relying on the non-interactive mode
/// to auto-dismiss a prompt; no human is watching).
///
/// Four of the five are unconditional. `--disable-builtin-mcps` is the one the
/// operator can deliberately give back, via `allow_builtin_mcps` (persisted as
/// `copilot.allow_builtin_mcp_servers_i_understand_the_risk`) — the D7 escape
/// hatch, which also suppresses the receipt guard below.
///
/// `--model` is passed only when the operator supplied one: omission selects the
/// account's *current default*, which is the correct default rather than a
/// degraded fallback, and a hardcoded id hard-fails every run on a free plan
/// (ADR-0041 D4, spike §4a).
///
/// `--effort` follows the same omit-by-default rule (D5), and `effort` is already
/// CLAMPED when it arrives here: Copilot's effort vocabulary is per-model, so
/// `crate::effort` maps the operator's request onto the chosen model's published
/// support list before this function sees it (D5a). A `None` means the operator
/// asked for nothing, the model takes no effort argument, or no support list was
/// knowable — in every case the flag is omitted and the model's own default wins.
///
/// The repo root is set with `current_dir`, not `-C`: the CLI honours the spawned
/// process's cwd (spike C1).
///
/// The flags are no longer merely trusted: `crate::guards` now asserts BOTH in
/// band — the D7 receipt (`session.mcp_servers_loaded` must report every builtin
/// server off, and an absent receipt fails closed) and D11's `continueOnAutoMode`,
/// checked as a preflight before any child is spawned.
pub(crate) fn build_copilot_command(
    session_id: &str,
    model: Option<&str>,
    effort: Option<&str>,
    work_dir: &Path,
    allow_builtin_mcps: bool,
) -> Command {
    let mut cmd = Command::new(resolve_program("copilot"));
    cmd.current_dir(work_dir)
        .arg("--allow-all-tools")
        .arg("--output-format")
        .arg("json")
        .arg("--session-id")
        .arg(session_id)
        .arg("--no-remote")
        .arg("--no-remote-export")
        .arg("--no-auto-update")
        .arg("--no-ask-user");
    if !allow_builtin_mcps {
        cmd.arg("--disable-builtin-mcps");
    }
    if let Some(m) = model {
        cmd.arg("--model").arg(m);
    }
    if let Some(e) = effort {
        cmd.arg("--effort").arg(e);
    }
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // D8: the three token variables are removed so an inherited operator
        // token can never authenticate the child. Copilot's own OAuth session
        // (`copilot login`) is the only credential Ralphy drives it with — an
        // ambient PAT would silently widen the run's GitHub reach.
        .env_remove("COPILOT_GITHUB_TOKEN")
        .env_remove("GH_TOKEN")
        .env_remove("GITHUB_TOKEN");
    cmd
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn argv(cmd: &Command) -> Vec<String> {
        cmd.get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    fn stem(cmd: &Command) -> String {
        PathBuf::from(cmd.get_program())
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    /// The #229 baseline, pinned: with the escape hatch OFF nothing about the
    /// default argv changed when D7's hatch was introduced (#234).
    #[test]
    fn defaults_are_unchanged_from_229() {
        let id = mint_session_id();
        let cmd = build_copilot_command(&id, None, None, Path::new("/repo"), false);
        assert_eq!(stem(&cmd), "copilot");

        let args = argv(&cmd);
        let pos = |flag: &str, val: &str| {
            let i = args.iter().position(|a| a == flag);
            assert!(i.is_some(), "missing {flag}: {args:?}");
            assert_eq!(args[i.unwrap() + 1], val, "value after {flag}: {args:?}");
        };
        assert!(
            args.contains(&"--allow-all-tools".to_string()),
            "argv: {args:?}"
        );
        pos("--output-format", "json");
        pos("--session-id", &id);

        // D7: all five blast-radius flags, unconditionally.
        for flag in [
            "--no-remote",
            "--no-remote-export",
            "--disable-builtin-mcps",
            "--no-auto-update",
            "--no-ask-user",
        ] {
            assert!(
                args.iter().any(|a| a == flag),
                "missing blast-radius flag {flag}: {args:?}"
            );
        }

        // D2: the charter rides on stdin — no `-p`, no positional prompt word.
        assert!(
            !args.iter().any(|a| a == "-p"),
            "the charter must be piped on stdin, never argv: {args:?}"
        );
        // D4/D5: omitted, not defaulted.
        assert!(!args.iter().any(|a| a == "--model"), "argv: {args:?}");
        assert!(!args.iter().any(|a| a == "--effort"), "argv: {args:?}");

        // D8: each token var is REMOVED (present in the env delta as `None`).
        for key in ["COPILOT_GITHUB_TOKEN", "GH_TOKEN", "GITHUB_TOKEN"] {
            let removed = cmd.get_envs().any(|(k, v)| k == key && v.is_none());
            assert!(removed, "{key} should be removed on the child");
        }

        // The persisted side of "unchanged": the new escape-hatch field must not
        // start serializing into every settings file.
        assert_eq!(
            serde_json::to_string(&crate::CopilotSettings::default()).unwrap(),
            "{}"
        );
    }

    #[test]
    fn build_command_passes_model_when_some() {
        let cmd = build_copilot_command(
            "s1",
            Some("claude-sonnet-5"),
            None,
            Path::new("/repo"),
            false,
        );
        let args = argv(&cmd);
        let i = args
            .iter()
            .position(|a| a == "--model")
            .expect("--model missing");
        assert_eq!(args[i + 1], "claude-sonnet-5");
        // The blast-radius flags survive a model override.
        assert!(args.iter().any(|a| a == "--no-ask-user"), "argv: {args:?}");
        assert!(!args.iter().any(|a| a == "--effort"), "argv: {args:?}");
    }

    /// D5: an effort that reached here is already clamped — it rides the argv as
    /// the pair `--effort <level>`, and the blast-radius flags survive it.
    #[test]
    fn build_command_passes_effort_when_some() {
        let cmd = build_copilot_command(
            "s1",
            Some("gpt-5-mini"),
            Some("high"),
            Path::new("/repo"),
            false,
        );
        let args = argv(&cmd);
        let i = args
            .iter()
            .position(|a| a == "--effort")
            .expect("--effort missing");
        assert_eq!(args[i + 1], "high");
        assert!(args.iter().any(|a| a == "--no-ask-user"), "argv: {args:?}");
    }

    /// D7's escape hatch drops exactly ONE flag — the other four blast-radius
    /// flags are not negotiable.
    #[test]
    fn escape_hatch_drops_disable_builtin_mcps_from_argv() {
        let cmd = build_copilot_command("s1", None, None, Path::new("/repo"), true);
        let args = argv(&cmd);
        assert!(
            !args.iter().any(|a| a == "--disable-builtin-mcps"),
            "the hatch must give the builtin MCP surface back: {args:?}"
        );
        for flag in [
            "--no-remote",
            "--no-remote-export",
            "--no-auto-update",
            "--no-ask-user",
        ] {
            assert!(
                args.iter().any(|a| a == flag),
                "the hatch must not widen {flag}: {args:?}"
            );
        }
    }

    #[test]
    fn mint_session_id_is_a_fresh_uuid() {
        let a = mint_session_id();
        let b = mint_session_id();
        assert_ne!(a, b);
        assert_eq!(a.len(), 36, "not a hyphenated UUID: {a}");
        assert_eq!(a.matches('-').count(), 4, "not a hyphenated UUID: {a}");
    }

    /// ADR-0040 C1: the binary is resolved through `resolve_program`, which honours
    /// the platform's `.exe`/shim lookup — naming the bare binary in a `Command`
    /// constructor fails on Windows for a `.cmd` shim and bypasses any override.
    /// The guard string is assembled from fragments so this assertion cannot match
    /// itself.
    #[test]
    fn no_direct_command_new() {
        assert!(
            !include_str!("command.rs").contains(concat!("Command::", "new(\"copilot\")")),
            "resolve_program is the only way to name the binary"
        );
    }
}
