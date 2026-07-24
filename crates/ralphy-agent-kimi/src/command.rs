//! Building the headless `kimi` invocation. A single point that fixes the flags
//! and points Kimi at the materialized skills store (ADR-0028 D5).

use std::path::Path;
use std::process::{Command, Stdio};

use ralphy_adapter_support::resolve_program;

/// The single Kimi model this slice drives. Full `provider/model` id (ADR-0028 D4).
pub(crate) const DEFAULT_KIMI_MODEL: &str = "kimi-code/k3";

/// Build the headless `kimi` command both `plan` and `execute` go through.
///
/// The 0.28 contract (ADR-0028 D5): `-p <prompt>` carries the charter on argv —
/// 0.28 has no stdin prompt channel — `--output-format stream-json` selects the
/// JSONL event stream, `-m` pins the full `provider/model` id, and `--skills-dir`
/// points at the ralphy-owned skills store. The child's env is inherited
/// unmodified; 0.28 is not a Python CLI and needs no stdio-encoding coercion.
///
/// Stdin stays piped (and is closed empty) because `HeadlessCall` requires a piped
/// stdin handle.
pub(crate) fn build_kimi_command(
    model: &str,
    work_dir: &Path,
    skills_dir: &Path,
    prompt: &str,
) -> Command {
    let mut cmd = Command::new(resolve_program("kimi"));
    cmd.current_dir(work_dir)
        .arg("-p")
        .arg(prompt)
        .arg("--output-format")
        .arg("stream-json")
        .arg("-m")
        .arg(model)
        .arg("--skills-dir")
        .arg(skills_dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    cmd
}

/// Resolve the model for a one-shot init session (diagnose/draft/triage): the
/// explicit override, else [`DEFAULT_KIMI_MODEL`]. No config parse in this slice
/// (ADR-0028 D4) — mirrors [`resolve_init_kimi_model`]'s sibling on Codex minus
/// the `codex_config_model` lookup.
pub(crate) fn resolve_init_kimi_model(model: Option<&str>) -> String {
    model
        .map(str::to_string)
        .unwrap_or_else(|| DEFAULT_KIMI_MODEL.to_string())
}

/// Build the headless `kimi` command for an `init` one-shot session
/// (diagnose/draft/triage). Unlike [`build_kimi_command`] it omits `--skills-dir`:
/// none of the init charters invoke the reviewer skill. `cwd` is the session's
/// working directory — a neutral dir outside the repo for diagnosis, the repo
/// itself for the issues draft and triage. The prompt rides `-p` on argv (see
/// [`build_kimi_command`]).
pub(crate) fn build_kimi_init_command(model: &str, cwd: &Path, prompt: &str) -> Command {
    let mut cmd = Command::new(resolve_program("kimi"));
    cmd.current_dir(cwd)
        .arg("-p")
        .arg(prompt)
        .arg("--output-format")
        .arg("stream-json")
        .arg("-m")
        .arg(model)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    cmd
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn build_command_argv_is_the_0_28_contract() {
        let cmd = build_kimi_command(
            DEFAULT_KIMI_MODEL,
            Path::new("/repo"),
            Path::new("/repo/.ralphy/skills"),
            "hello",
        );
        // The program is the resolved `kimi` binary (file stem `kimi` regardless of
        // any `.exe`/absolute-path resolution).
        let stem = PathBuf::from(cmd.get_program())
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        assert_eq!(stem, "kimi");

        let args: Vec<String> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            args,
            vec![
                "-p",
                "hello",
                "--output-format",
                "stream-json",
                "-m",
                "kimi-code/k3",
                "--skills-dir",
                "/repo/.ralphy/skills",
            ]
        );
        assert!(
            !args.iter().any(|a| a == "--effort"),
            "Kimi has no effort flag (ADR-0044 D4): {args:?}"
        );
        assert_eq!(cmd.get_current_dir(), Some(Path::new("/repo")));
        // The 0.28 contract inherits the operator env untouched: no stdio-encoding
        // coercion of any kind.
        assert_eq!(cmd.get_envs().count(), 0);
    }

    #[test]
    fn build_init_command_argv_and_env() {
        let cmd = build_kimi_init_command(DEFAULT_KIMI_MODEL, Path::new("/repo"), "hello");
        let stem = PathBuf::from(cmd.get_program())
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        assert_eq!(stem, "kimi");

        let args: Vec<String> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            args,
            vec![
                "-p",
                "hello",
                "--output-format",
                "stream-json",
                "-m",
                "kimi-code/k3",
            ]
        );
        assert_eq!(cmd.get_current_dir(), Some(Path::new("/repo")));
        assert!(
            !args.iter().any(|a| a == "--skills-dir"),
            "init sessions don't invoke the reviewer skill: {args:?}"
        );
        assert_eq!(cmd.get_envs().count(), 0);
    }

    #[test]
    fn resolve_init_kimi_model_override_wins() {
        assert_eq!(resolve_init_kimi_model(Some("x")), "x");
        assert_eq!(resolve_init_kimi_model(None), DEFAULT_KIMI_MODEL);
    }
}
