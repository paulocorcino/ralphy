//! Building the headless `opencode run` invocation both `plan` and `execute`
//! go through.

use std::path::Path;
use std::process::{Command, Stdio};

use ralphy_adapter_support::{resolve_program, DENIED_FORGE_WRITES};

/// Build the headless `opencode run` command both `plan` and `execute` go through
/// — the single point that fixes the invocation, always passes
/// `--dangerously-skip-permissions` (the headless-hang guard, ADR-0005 D5) and
/// `--format json`, omits `-m` unless the operator set one (D4), passes
/// `--variant` only when set (D3), injects `OPENCODE_CONFIG_CONTENT` (see
/// [`opencode_config`]), runs in `root`, and defensively removes both
/// `ANTHROPIC_API_KEY` and `OPENAI_API_KEY` so an inherited key can't switch
/// the run to metered API billing (D6). The prompt is written on stdin.
pub(crate) fn build_opencode_command(
    model: Option<&str>,
    variant: Option<&str>,
    root: &Path,
    skills_dir: Option<&Path>,
) -> Command {
    // Resolve `opencode` to its real path: on Windows it ships as an npm `.cmd`
    // shim with no `.exe`, which a bare `Command::new("opencode")` cannot find.
    let mut cmd = Command::new(resolve_program("opencode"));
    cmd.arg("run")
        .arg("--format")
        .arg("json")
        .arg("--dangerously-skip-permissions")
        // Route opencode's own logs (logfmt) onto stderr at ERROR level. Some
        // providers (Z.ai `zai-coding-plan`/GLM) never emit a `{type:"error"}`
        // JSON event on a quota block: their ai-sdk treats the `AI_APICallError:
        // Usage limit reached` as a *retryable* stream error and loops on backoff,
        // printing it only to the server log — so the `--format json` stream stays
        // silent and the run stalls until the wall timeout. `--print-logs` brings
        // that line onto the stderr we already drain, where `parse_opencode_log_limit`
        // can see it and classify the run as `Limit` instead of a mute `Timeout`
        // (observed live 2026-07-11, FinCal #71, glm-5.2). ERROR keeps the combined
        // log lean; the quota line is logged at ERROR.
        .arg("--print-logs")
        .arg("--log-level")
        .arg("ERROR");
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
        .env("OPENCODE_CONFIG_CONTENT", opencode_config(skills_dir))
        .env_remove("ANTHROPIC_API_KEY")
        .env_remove("OPENAI_API_KEY");
    cmd
}

/// The JSON injected as `OPENCODE_CONFIG_CONTENT`: `skills.paths` when the
/// session has materialized skills (plan and execute, ADR-0005 D7), and in
/// every session a `permission.bash` map that denies the forge writes
/// (ADR-0072 D6, ADR-0005 D5 amendment). opencode applies an explicit `deny`
/// even under `--dangerously-skip-permissions`, and the refused call reaches
/// the model as a tool error; the run goes on (measured with opencode 1.18.32).
/// The map has no `"*"` entry, so the operator's own bash rules still apply.
fn opencode_config(skills_dir: Option<&Path>) -> String {
    let mut bash = serde_json::Map::new();
    for words in DENIED_FORGE_WRITES {
        let command = words.join(" ");
        bash.insert(format!("{command} *"), "deny".into());
        bash.insert(command, "deny".into());
    }
    let mut config = serde_json::json!({ "permission": { "bash": bash } });
    if let Some(dir) = skills_dir {
        // Canonicalized for robustness; on failure the path is used as-is.
        let abs = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
        config["skills"] = serde_json::json!({ "paths": [abs] });
    }
    config.to_string()
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
        let cmd = build_opencode_command(None, None, Path::new("/repo"), None);
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
        // `--print-logs`/`--log-level ERROR` route opencode's own logs to stderr so
        // a provider quota block that never reaches the JSON stream is still visible
        // to the limit detector (D9 — silent-quota fix, FinCal #71).
        assert!(args.contains(&"--print-logs".to_string()), "argv: {args:?}");
        assert!(args.contains(&"--log-level".to_string()), "argv: {args:?}");
        assert!(args.contains(&"ERROR".to_string()), "argv: {args:?}");
    }

    #[test]
    fn build_command_includes_model_when_some() {
        let cmd = build_opencode_command(
            Some("anthropic/claude-sonnet-4-6"),
            None,
            Path::new("/repo"),
            None,
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
        let without = build_opencode_command(None, None, Path::new("/repo"), None);
        assert!(!argv(&without).contains(&"--variant".to_string()));

        let with = build_opencode_command(None, Some("high"), Path::new("/repo"), None);
        let args = argv(&with);
        assert!(args.contains(&"--variant".to_string()), "argv: {args:?}");
        assert!(args.contains(&"high".to_string()), "argv: {args:?}");
    }

    #[test]
    fn build_command_removes_both_api_keys() {
        let cmd = build_opencode_command(None, None, Path::new("/repo"), None);
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

    fn injected_config(cmd: &Command) -> serde_json::Value {
        let raw = cmd
            .get_envs()
            .find(|(k, _)| *k == "OPENCODE_CONFIG_CONTENT")
            .and_then(|(_, v)| v)
            .map(|v| v.to_string_lossy().into_owned())
            .expect("OPENCODE_CONFIG_CONTENT is set");
        serde_json::from_str(&raw).expect("the injected config is JSON")
    }

    #[test]
    fn build_command_injects_the_skills_path_only_when_given() {
        let dir = std::env::temp_dir().join("ralphy-opencode-skills-cfg");
        std::fs::create_dir_all(&dir).unwrap();
        let cmd = build_opencode_command(None, None, Path::new("/repo"), Some(&dir));
        let expected = dir.canonicalize().unwrap_or_else(|_| dir.clone());
        assert_eq!(
            injected_config(&cmd)["skills"]["paths"],
            serde_json::json!([expected])
        );

        let cfg = injected_config(&build_opencode_command(
            None,
            None,
            Path::new("/repo"),
            None,
        ));
        assert!(cfg.get("skills").is_none(), "{cfg}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// opencode's documented wildcard rule: `*` is any run of characters and
    /// the pattern must match the whole command.
    fn glob(pattern: &str, text: &str) -> bool {
        match pattern.split_once('*') {
            None => pattern == text,
            Some((head, tail)) => {
                let Some(rest) = text.strip_prefix(head) else {
                    return false;
                };
                rest.char_indices()
                    .map(|(i, _)| i)
                    .chain([rest.len()])
                    .any(|i| glob(tail, &rest[i..]))
            }
        }
    }

    /// Every session, with or without skills, carries the deny map, and the
    /// map denies the forge writes and nothing else.
    #[test]
    fn every_session_denies_the_forge_writes_and_only_them() {
        let skills = std::env::temp_dir();
        for skills_dir in [None, Some(skills.as_path())] {
            let cmd = build_opencode_command(None, None, Path::new("/repo"), skills_dir);
            let cfg = injected_config(&cmd);
            let rules = cfg["permission"]["bash"]
                .as_object()
                .expect("permission.bash is a map");
            assert!(rules.values().all(|v| v == "deny"), "{cfg}");
            assert!(!rules.contains_key("*"), "the operator's own rules apply");
            let denied = |command: &str| rules.keys().any(|p| glob(p, command));
            for command in [
                "git push",
                "git push origin HEAD",
                "gh pr create --fill",
                "gh pr edit 5",
                "gh pr ready",
                "gh pr reopen 5",
                "gh pr review 5 --approve",
                "gh pr comment 5 -b hi",
                "gh pr merge 5",
                "gh pr close 5",
            ] {
                assert!(denied(command), "{command} must be denied");
            }
            for command in [
                "git status",
                "git commit -m \"fix push logic\"",
                "git pull",
                "gh pr view 5",
                "gh pr list",
                "gh pr diff 5",
                "gh pr checks 5",
                "gh api repos/o/r/pulls",
                "gh issue comment 5 -b hi",
            ] {
                assert!(!denied(command), "{command} must stay allowed");
            }
        }
    }
}
