//! Building the headless `kimi --print` invocation. A single point that fixes the
//! flags, points Kimi at the materialized skills store, and settles Kimi's Windows
//! stdio encoding (ADR-0028 D5 + 0028-kimi-validation) so the contract holds
//! regardless of the operator's env.

use std::path::Path;
use std::process::{Command, Stdio};

use ralphy_adapter_support::resolve_program;

/// The single Kimi model this slice drives. Full `provider/model` id — Kimi's Typer
/// CLI rejects a bare model name (ADR-0028 D4).
pub(crate) const DEFAULT_KIMI_MODEL: &str = "kimi-code/kimi-for-coding";

/// Build the headless `kimi --print` command both `plan` and `execute` go through.
///
/// The prompt is NEVER passed as a positional argument: Kimi's Typer front-end
/// parses a positional word as a subcommand, so the charter is piped on stdin and
/// `--input-format text` tells Kimi to read it. `--output-format stream-json`
/// forces the ASCII-safe role-JSONL stream (avoids the cp1252 Textual-TUI crash);
/// `-y` auto-approves tool use; `--skills-dir` points at the ralphy-owned store.
///
/// Windows stdio encoding is settled with two env moves that must go together
/// (validated live, 0028-kimi-validation):
/// - `PYTHONIOENCODING` is **removed**: an inherited `PYTHONIOENCODING=utf-8` flips
///   Kimi into starting the Textual TUI ("No Windows console found"), breaking the
///   headless contract (ADR-0028 D5).
/// - `PYTHONUTF8=1` is **set**: without it Kimi's Python stdio defaults to cp1252 on
///   Windows and crashes with `'charmap' codec can't encode…` (exit 1) the moment a
///   tool subprocess prints a non-cp1252 char — e.g. Prisma/npm's `✔` during
///   `npm install`, which killed the first live execute. UTF-8 Mode (PEP 540) fixes
///   the capture without touching Kimi's console detection, so it does **not**
///   re-trigger the TUI trap. No-op on an already-UTF-8 Linux locale.
pub(crate) fn build_kimi_command(model: &str, work_dir: &Path, skills_dir: &Path) -> Command {
    let mut cmd = Command::new(resolve_program("kimi"));
    cmd.arg("--work-dir")
        .arg(work_dir)
        .arg("--print")
        .arg("--input-format")
        .arg("text")
        .arg("--output-format")
        .arg("stream-json")
        .arg("-y")
        .arg("-m")
        .arg(model)
        .arg("--skills-dir")
        .arg(skills_dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env_remove("PYTHONIOENCODING")
        .env("PYTHONUTF8", "1");
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

/// Build the headless `kimi --print` command for an `init` one-shot session
/// (diagnose/draft/triage). Unlike [`build_kimi_command`] it omits `--skills-dir`:
/// none of the init charters invoke the reviewer skill. `cwd` is the session's
/// working directory — a neutral dir outside the repo for diagnosis, the repo
/// itself for the issues draft and triage. The prompt is piped on stdin, never a
/// positional argument (see [`build_kimi_command`] doc for why).
pub(crate) fn build_kimi_init_command(model: &str, cwd: &Path) -> Command {
    let mut cmd = Command::new(resolve_program("kimi"));
    cmd.arg("--work-dir")
        .arg(cwd)
        .arg("--print")
        .arg("--input-format")
        .arg("text")
        .arg("--output-format")
        .arg("stream-json")
        .arg("-y")
        .arg("-m")
        .arg(model)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env_remove("PYTHONIOENCODING")
        .env("PYTHONUTF8", "1");
    cmd
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn build_command_argv_and_env() {
        let cmd = build_kimi_command(
            DEFAULT_KIMI_MODEL,
            Path::new("/repo"),
            Path::new("/repo/.ralphy/skills"),
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
        assert!(args.contains(&"--print".to_string()), "argv: {args:?}");
        let pos = |flag: &str, val: &str| {
            let i = args.iter().position(|a| a == flag);
            assert!(i.is_some(), "missing {flag}: {args:?}");
            assert_eq!(args[i.unwrap() + 1], val, "value after {flag}: {args:?}");
        };
        pos("--input-format", "text");
        pos("--output-format", "stream-json");
        pos("-m", DEFAULT_KIMI_MODEL);
        assert!(args.contains(&"-y".to_string()), "argv: {args:?}");
        // Assert the path VALUES, not just flag presence, so a swap of the two path
        // args (work-dir ↔ skills-dir) would fail.
        pos("--work-dir", "/repo");
        pos("--skills-dir", "/repo/.ralphy/skills");

        // NO positional prompt arg: Typer would parse a bare word as a subcommand.
        assert!(
            !args.iter().any(|a| a == "hello"),
            "prompt must be piped on stdin, never argv: {args:?}"
        );

        // PYTHONIOENCODING is removed on the child so an inherited value can't flip
        // Kimi into the Textual TUI.
        let removed = cmd
            .get_envs()
            .any(|(k, v)| k == "PYTHONIOENCODING" && v.is_none());
        assert!(removed, "PYTHONIOENCODING should be removed on the child");

        // PYTHONUTF8=1 is set so Kimi's Python stdio is UTF-8 and captured tool
        // subprocess output with non-cp1252 chars (e.g. `✔`) can't crash it.
        let utf8 = cmd
            .get_envs()
            .any(|(k, v)| k == "PYTHONUTF8" && v == Some("1".as_ref()));
        assert!(utf8, "PYTHONUTF8 should be set to 1 on the child");
    }

    #[test]
    fn build_init_command_argv_and_env() {
        let cmd = build_kimi_init_command(DEFAULT_KIMI_MODEL, Path::new("/repo"));
        let stem = PathBuf::from(cmd.get_program())
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        assert_eq!(stem, "kimi");

        let args: Vec<String> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        let pos = |flag: &str, val: &str| {
            let i = args.iter().position(|a| a == flag);
            assert!(i.is_some(), "missing {flag}: {args:?}");
            assert_eq!(args[i.unwrap() + 1], val, "value after {flag}: {args:?}");
        };
        pos("--work-dir", "/repo");
        pos("--input-format", "text");
        pos("--output-format", "stream-json");
        pos("-m", DEFAULT_KIMI_MODEL);
        assert!(args.contains(&"-y".to_string()), "argv: {args:?}");

        assert!(
            !args.iter().any(|a| a == "--skills-dir"),
            "init sessions don't invoke the reviewer skill: {args:?}"
        );
        assert!(
            !args.iter().any(|a| a == "hello"),
            "prompt must be piped on stdin, never argv: {args:?}"
        );

        let removed = cmd
            .get_envs()
            .any(|(k, v)| k == "PYTHONIOENCODING" && v.is_none());
        assert!(removed, "PYTHONIOENCODING should be removed on the child");

        let utf8 = cmd
            .get_envs()
            .any(|(k, v)| k == "PYTHONUTF8" && v == Some("1".as_ref()));
        assert!(utf8, "PYTHONUTF8 should be set to 1 on the child");
    }

    #[test]
    fn resolve_init_kimi_model_override_wins() {
        assert_eq!(resolve_init_kimi_model(Some("x")), "x");
        assert_eq!(resolve_init_kimi_model(None), DEFAULT_KIMI_MODEL);
    }
}
