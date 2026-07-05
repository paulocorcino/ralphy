//! Building the headless `opencode run` invocation both `plan` and `execute`
//! go through.

use std::path::Path;
use std::process::{Command, Stdio};

use ralphy_adapter_support::resolve_program;

/// Build the headless `opencode run` command both `plan` and `execute` go through
/// — the single point that fixes the invocation, always passes
/// `--dangerously-skip-permissions` (the headless-hang guard, ADR-0005 D5) and
/// `--format json`, omits `-m` unless the operator set one (D4), passes
/// `--variant` only when set (D3), injects `OPENCODE_CONFIG_CONTENT` with the
/// skills path (D7), runs in the repo root, and defensively removes both
/// `ANTHROPIC_API_KEY` and `OPENAI_API_KEY` so an inherited key can't switch
/// the run to metered API billing (D6). The prompt is written on stdin.
///
/// GUARD ASYMMETRY: `--dangerously-skip-permissions` runs with no equivalent
/// of the Claude adapter's PreToolUse guard hook (opencode has no such hook
/// point wired here) — safety rests on the isolated run branch and the
/// prompt's hard rules.
pub(crate) fn build_opencode_command(
    model: Option<&str>,
    variant: Option<&str>,
    root: &Path,
    skills_config: &str,
) -> Command {
    // Resolve `opencode` to its real path: on Windows it ships as an npm `.cmd`
    // shim with no `.exe`, which a bare `Command::new("opencode")` cannot find.
    let mut cmd = Command::new(resolve_program("opencode"));
    cmd.arg("run")
        .arg("--format")
        .arg("json")
        .arg("--dangerously-skip-permissions");
    if let Some(m) = model {
        cmd.arg("-m").arg(m);
    }
    if let Some(v) = variant {
        cmd.arg("--variant").arg(v);
    }
    cmd.current_dir(root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("OPENCODE_CONFIG_CONTENT", skills_config)
        .env_remove("ANTHROPIC_API_KEY")
        .env_remove("OPENAI_API_KEY");
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

    #[test]
    fn build_command_omits_model_when_none() {
        let cmd = build_opencode_command(None, None, Path::new("/repo"), "{}");
        // The program is `resolve_program("opencode")`: a full path (e.g.
        // `opencode.cmd` on Windows) when found on PATH, else the bare name. Either
        // way the file stem is `opencode`.
        let program = PathBuf::from(cmd.get_program());
        assert_eq!(
            program.file_stem().and_then(|s| s.to_str()),
            Some("opencode"),
            "program: {program:?}"
        );
        let args = argv(&cmd);
        assert!(args.contains(&"run".to_string()), "argv: {args:?}");
        // No -m flag is passed; OpenCode resolves its own model (ADR-0005 D4).
        assert!(!args.contains(&"-m".to_string()), "argv: {args:?}");
        // --variant is absent unless the operator set one (D3).
        assert!(!args.contains(&"--variant".to_string()), "argv: {args:?}");
        // Always-on flags.
        assert!(
            args.contains(&"--dangerously-skip-permissions".to_string()),
            "argv: {args:?}"
        );
        assert!(args.contains(&"--format".to_string()), "argv: {args:?}");
        assert!(args.contains(&"json".to_string()), "argv: {args:?}");
    }

    #[test]
    fn build_command_includes_model_when_some() {
        let cmd = build_opencode_command(
            Some("anthropic/claude-sonnet-4-6"),
            None,
            Path::new("/repo"),
            "{}",
        );
        let args = argv(&cmd);
        assert!(args.contains(&"-m".to_string()), "argv: {args:?}");
        assert!(
            args.contains(&"anthropic/claude-sonnet-4-6".to_string()),
            "argv: {args:?}"
        );
    }

    #[test]
    fn build_command_includes_variant_only_when_some() {
        let without = build_opencode_command(None, None, Path::new("/repo"), "{}");
        assert!(!argv(&without).contains(&"--variant".to_string()));

        let with = build_opencode_command(None, Some("high"), Path::new("/repo"), "{}");
        let args = argv(&with);
        assert!(args.contains(&"--variant".to_string()), "argv: {args:?}");
        assert!(args.contains(&"high".to_string()), "argv: {args:?}");
    }

    #[test]
    fn build_command_removes_both_api_keys() {
        let cmd = build_opencode_command(None, None, Path::new("/repo"), "{}");
        let anthropic_removed = cmd
            .get_envs()
            .any(|(k, v)| k == "ANTHROPIC_API_KEY" && v.is_none());
        let openai_removed = cmd
            .get_envs()
            .any(|(k, v)| k == "OPENAI_API_KEY" && v.is_none());
        assert!(
            anthropic_removed,
            "ANTHROPIC_API_KEY should be removed on the child"
        );
        assert!(
            openai_removed,
            "OPENAI_API_KEY should be removed on the child"
        );
    }

    #[test]
    fn build_command_injects_skills_config() {
        let cfg = r#"{"skills":{"paths":["/some/skills"]}}"#;
        let cmd = build_opencode_command(None, None, Path::new("/repo"), cfg);
        let injected = cmd
            .get_envs()
            .find(|(k, _)| *k == "OPENCODE_CONFIG_CONTENT")
            .and_then(|(_, v)| v)
            .map(|v| v.to_string_lossy().into_owned());
        assert_eq!(injected.as_deref(), Some(cfg));
    }
}
