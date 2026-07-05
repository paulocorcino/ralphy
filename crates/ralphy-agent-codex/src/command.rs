//! Building the `codex exec` invocation and resolving the model/effort it runs
//! with, from the operator override, the user's Codex config, and the
//! planner's neutral complexity tier.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// The last-resort Codex model, used only when neither `--exec-model` nor the
/// user's Codex config names one. ChatGPT-auth accounts reject `gpt-5-codex`, so
/// in practice the config-derived model (see [`codex_config_model`]) is what most
/// subscription runs use; this constant is the floor for an unconfigured setup.
pub(crate) const DEFAULT_CODEX_MODEL: &str = "gpt-5-codex";

/// Locate the Codex config file: `$CODEX_HOME/config.toml` when `CODEX_HOME` is
/// set (matching Codex's own resolution), else `<home>/.codex/config.toml`
/// (`USERPROFILE` on Windows, `HOME` elsewhere). `None` when no home is known.
fn codex_config_path() -> Option<PathBuf> {
    ralphy_adapter_support::home_scoped_path(
        std::env::var_os("CODEX_HOME"),
        Path::new(".codex"),
        Path::new("config.toml"),
    )
}

/// The top-level `model = "..."` from the user's Codex config, if present and
/// readable. `None` when the file or the key is absent — the caller then falls
/// back to [`DEFAULT_CODEX_MODEL`].
pub(crate) fn codex_config_model() -> Option<String> {
    let text = fs::read_to_string(codex_config_path()?).ok()?;
    parse_codex_config_model(&text)
}

/// Extract the root-table `model` key from Codex `config.toml` text. Only the
/// root table is considered (scanning stops at the first `[section]` header) so a
/// `model` under e.g. `[mcp_servers.x]` can't be mistaken for the active default.
/// `model_reasoning_effort` is not matched: the `=` must follow `model` directly.
fn parse_codex_config_model(toml: &str) -> Option<String> {
    use regex::Regex;
    let re = Regex::new(r#"(?m)^\s*model\s*=\s*"([^"]+)""#).expect("valid regex");
    for line in toml.lines() {
        if line.trim_start().starts_with('[') {
            break; // left the root table
        }
        if let Some(c) = re.captures(line) {
            return Some(c[1].to_string());
        }
    }
    None
}

/// The planner's `## Execution model: low|medium|high` complexity tier, lowercased,
/// if any. The Codex plan variant emits a vendor-neutral tier rather than a Claude
/// model name; this is the private mirror of `plan::recommended_model` for the
/// Codex path, leaving the core's `opus|sonnet` parser untouched.
pub(crate) fn recommended_tier(md: &str) -> Option<String> {
    use regex::Regex;
    let re =
        Regex::new(r"(?im)^\s*##\s*Execution model:\s*(low|medium|high)").expect("valid regex");
    re.captures(md).map(|c| c[1].to_lowercase())
}

/// Map a neutral complexity tier to the `model_reasoning_effort` value. Unknown or
/// absent tiers default to `medium` — the single tier→effort point (ADR-0004 D3).
pub(crate) fn tier_to_effort(tier: Option<&str>) -> &'static str {
    match tier {
        Some("low") => "low",
        Some("high") => "high",
        _ => "medium",
    }
}

/// Build the headless `codex exec` command both `plan` and `execute` go through —
/// the single point that fixes the invocation, defensively removes
/// `OPENAI_API_KEY` (so an inherited key can't switch the run to API billing,
/// ADR-0004 D5), and pipes stdin/stdout/stderr. The prompt is written on stdin
/// (the trailing `-`); the agent's final message is captured to `out_path` for the
/// sentinel read.
///
/// GUARD ASYMMETRY: `danger-full-access` runs with no equivalent of the Claude
/// adapter's PreToolUse guard hook (codex-cli has no such hook point) — safety
/// here rests on the isolated run branch and the prompt's hard rules.
pub(crate) fn build_codex_command(
    model: &str,
    effort: &str,
    root: &Path,
    out_path: &Path,
) -> Command {
    let mut cmd = Command::new("codex");
    cmd.arg("exec")
        .arg("-C")
        .arg(root)
        .arg("-m")
        .arg(model)
        .arg("-c")
        .arg(format!("model_reasoning_effort=\"{effort}\""))
        .arg("-s")
        .arg("danger-full-access")
        .arg("-o")
        .arg(out_path)
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env_remove("OPENAI_API_KEY");
    cmd
}

// ── init one-shot sessions (ADR-0012 stages 2 & 8) ──────────────────────────

/// Resolve the Codex model for a one-shot `init` session: the explicit override,
/// then the user's Codex config model, then [`DEFAULT_CODEX_MODEL`]. Mirrors
/// `CodexAgent::resolve_model` without needing an agent instance.
pub(crate) fn resolve_init_model(model: Option<&str>) -> String {
    model
        .map(str::to_string)
        .or_else(codex_config_model)
        .unwrap_or_else(|| DEFAULT_CODEX_MODEL.to_string())
}

/// Build the headless `codex exec` command for an `init` one-shot session. Unlike
/// [`build_codex_command`] it omits `-o`: the session writes its JSON artifact to
/// the path named in the prompt using its own (full-access) tools, so capturing
/// the final message would clobber that file. `cwd` is the session's working
/// directory — a neutral dir outside the repo for diagnosis, the repo itself for
/// the issues draft. The prompt is piped on stdin (the trailing `-`).
///
/// `--skip-git-repo-check` is required because the diagnosis session's cwd is a
/// fresh neutral dir OUTSIDE any git repo (the mechanism that stops Codex from
/// auto-loading the target's `AGENTS.md`); without the flag `codex exec` refuses
/// to start there. It is a harmless no-op on the draft path, whose cwd is the
/// repo itself.
pub(crate) fn build_codex_init_command(model: &str, effort: &str, cwd: &Path) -> Command {
    let mut cmd = Command::new("codex");
    cmd.arg("exec")
        .arg("-C")
        .arg(cwd)
        .arg("--skip-git-repo-check")
        .arg("-m")
        .arg(model)
        .arg("-c")
        .arg(format!("model_reasoning_effort=\"{effort}\""))
        .arg("-s")
        .arg("danger-full-access")
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env_remove("OPENAI_API_KEY");
    cmd
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── build_codex_command ─────────────────────────────────────────────────

    #[test]
    fn build_command_argv_and_env() {
        let cmd = build_codex_command(
            "gpt-5-codex",
            "high",
            Path::new("/repo"),
            Path::new("/repo/.ralphy/codex-last.txt"),
        );
        assert_eq!(cmd.get_program().to_string_lossy(), "codex");

        let args: Vec<String> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert!(args.contains(&"exec".to_string()), "argv: {args:?}");
        assert!(args.contains(&"-C".to_string()), "argv: {args:?}");
        assert!(args.contains(&"-m".to_string()), "argv: {args:?}");
        assert!(args.contains(&"-o".to_string()), "argv: {args:?}");
        assert!(
            args.iter().any(|a| a == "model_reasoning_effort=\"high\""),
            "effort arg missing: {args:?}"
        );
        // Sandbox posture and the trailing stdin marker. `codex exec` defaults to
        // approval=never, so no explicit `-a` flag is passed (it is rejected by
        // codex-cli ≥0.138).
        assert!(args.contains(&"danger-full-access".to_string()));
        assert!(args.contains(&"-".to_string()));

        // OPENAI_API_KEY is removed on the child so an inherited key can't switch
        // the run to API billing.
        let removed = cmd
            .get_envs()
            .any(|(k, v)| k == "OPENAI_API_KEY" && v.is_none());
        assert!(removed, "OPENAI_API_KEY should be removed on the child");
    }

    #[test]
    fn build_command_threads_the_effort_through() {
        let cmd = build_codex_command(
            "gpt-5-codex",
            "low",
            Path::new("/repo"),
            Path::new("/repo/out.txt"),
        );
        let args: Vec<String> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert!(args.iter().any(|a| a == "model_reasoning_effort=\"low\""));
    }

    // ── recommended_tier ────────────────────────────────────────────────────

    #[test]
    fn recommended_tier_parses_each_tier() {
        assert_eq!(
            recommended_tier("## Execution model: low\nbecause").as_deref(),
            Some("low")
        );
        assert_eq!(
            recommended_tier("## Execution model: Medium\n").as_deref(),
            Some("medium")
        );
        assert_eq!(
            recommended_tier("## Execution model: HIGH\n").as_deref(),
            Some("high")
        );
    }

    #[test]
    fn recommended_tier_none_on_no_match() {
        assert_eq!(recommended_tier("no judgment here"), None);
        // A Claude model name is not a Codex tier.
        assert_eq!(recommended_tier("## Execution model: opus"), None);
    }

    // ── tier_to_effort ──────────────────────────────────────────────────────

    #[test]
    fn tier_to_effort_maps_and_defaults() {
        assert_eq!(tier_to_effort(Some("low")), "low");
        assert_eq!(tier_to_effort(Some("medium")), "medium");
        assert_eq!(tier_to_effort(Some("high")), "high");
        // Absent or unrecognized tiers default to medium.
        assert_eq!(tier_to_effort(None), "medium");
        assert_eq!(tier_to_effort(Some("bogus")), "medium");
    }

    // ── parse_codex_config_model ────────────────────────────────────────────

    #[test]
    fn parse_codex_config_model_reads_root_model() {
        let toml =
            "model = \"gpt-5.5\"\nmodel_reasoning_effort = \"high\"\n\n[features]\nx = true\n";
        assert_eq!(parse_codex_config_model(toml).as_deref(), Some("gpt-5.5"));
    }

    #[test]
    fn parse_codex_config_model_ignores_effort_and_non_root() {
        // model_reasoning_effort is a different key, not the model.
        assert_eq!(
            parse_codex_config_model("model_reasoning_effort = \"high\"\n"),
            None
        );
        // A `model` under a section is not the root default.
        assert_eq!(
            parse_codex_config_model("[mcp_servers.x]\nmodel = \"other\"\n"),
            None
        );
        assert_eq!(parse_codex_config_model("no keys here\n"), None);
    }
}
