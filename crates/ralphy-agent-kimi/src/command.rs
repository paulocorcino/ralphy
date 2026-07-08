//! Building the headless `kimi --print` invocation. A single point that fixes the
//! flags, points Kimi at the materialized skills store, and neutralizes the
//! `PYTHONIOENCODING` trap (ADR-0028 D5) so the contract holds regardless of the
//! operator's env.

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
/// `PYTHONIOENCODING` is removed on the child: an inherited `PYTHONIOENCODING=utf-8`
/// flips Kimi into starting the Textual TUI ("No Windows console found"), so
/// stripping it guarantees the headless contract (ADR-0028 D5).
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
        .env_remove("PYTHONIOENCODING");
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
    }
}
