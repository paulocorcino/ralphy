//! Building the `codex exec` invocation and resolving the model/effort it runs
//! with, from the operator override, the user's Codex config, and the
//! planner's neutral complexity tier (routed to a model — ADR-0004, Amendment
//! 2026-07-10).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use ralphy_proc_util::hook_form;

// The Codex model family, positioned by weight (ADR-0004, Amendment 2026-07-10).
// This table is the SINGLE place the pinned `gpt-5.6-*` generation lives: when
// the vendor ships the next family, the migration is these constants plus a new
// ADR amendment — never a hunt across the repo.
/// Flagship model: planning and tier-`high` execution (the `opus` analogue).
pub(crate) const CODEX_MODEL_SOL: &str = "gpt-5.6-sol";
/// Balanced everyday model: tier-`medium` execution (the `sonnet` analogue).
pub(crate) const CODEX_MODEL_TERRA: &str = "gpt-5.6-terra";
/// Fast/affordable model: tier-`low` (mechanical, well-understood) execution.
pub(crate) const CODEX_MODEL_LUNA: &str = "gpt-5.6-luna";

/// Default `model_reasoning_effort` when the operator leaves `--*-effort` unset.
/// The tier chooses the MODEL; effort is an orthogonal axis (ADR-0004 Amendment
/// 2026-07-23). This `-c` override supersedes the user's own `config.toml`
/// effort (observed live, ADR-0004 Amendment 2026-07-10) — deliberate, so runs
/// don't inherit an interactive-use setting.
pub(crate) const DEFAULT_CODEX_EFFORT: &str = "medium";

/// Clamp a neutral effort word to a value Codex's `model_reasoning_effort`
/// accepts (`minimal|low|medium|high`). The neutral lexicon's top rungs
/// `xhigh`/`max` (ADR-0044) have no Codex analogue, so they saturate to `high`
/// — otherwise `codex exec` rejects the `-c` override at startup. Only the argv
/// is clamped; telemetry keeps the operator's original word.
pub(crate) fn codex_reasoning_effort(word: &str) -> &str {
    match word {
        "xhigh" | "max" => "high",
        other => other,
    }
}

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
/// back to the tier-routed family table (or omits `-m` on init one-shots).
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

/// The planner's `## Execution model: low|medium|high|xhigh` complexity tier,
/// lowercased, if any. The Codex plan variant emits a vendor-neutral tier rather
/// than a Claude model name; this is the private mirror of `plan::recommended_model`
/// for the Codex path, leaving the core's `opus|sonnet` parser untouched. `xhigh`
/// is the fourth rung (ADR-0004, Amendment 2026-07-24) that reaches Sol at high
/// effort. `xhigh|high` order matters: alternation is leftmost-first, so `high`
/// must not shadow `xhigh`.
pub(crate) fn recommended_tier(md: &str) -> Option<String> {
    use regex::Regex;
    let re = Regex::new(r"(?im)^\s*##\s*Execution model:\s*(low|medium|xhigh|high)")
        .expect("valid regex");
    re.captures(md).map(|c| c[1].to_lowercase())
}

/// Map a neutral complexity tier to the executor `(model, default effort)` — one
/// rung on the cost/power ladder (ADR-0004, Amendment 2026-07-24). The effort is
/// the tier's DEFAULT; an explicit `--exec-effort` overrides it (see
/// `CodexAgent::resolved_exec_effort`). Unknown or absent tiers default to
/// Terra:medium — the single tier→model+effort point, the mirror of the Claude
/// adapter's tier↔model point (ADR-0002). The `xhigh` rung's effort is the concrete
/// Codex word `high` (the neutral `xhigh` names the rung, not a Codex value).
pub(crate) fn tier_to_model_effort(tier: Option<&str>) -> (&'static str, &'static str) {
    match tier {
        Some("low") => (CODEX_MODEL_LUNA, "low"),
        Some("high") => (CODEX_MODEL_SOL, "medium"),
        Some("xhigh") => (CODEX_MODEL_SOL, "high"),
        _ => (CODEX_MODEL_TERRA, "medium"),
    }
}

/// Build the headless `codex exec` command both `plan` and `execute` go through —
/// the single point that fixes the invocation, defensively removes
/// `OPENAI_API_KEY` (so an inherited key can't switch the run to API billing,
/// ADR-0004 D5), and pipes stdin/stdout/stderr. The prompt is written on stdin
/// (the trailing `-`); the agent's final message is captured to `out_path` for the
/// sentinel read.
///
/// The child also gets the status hooks (ADR-0085 D2): each runs
/// `<hook_exe> hook status codex` and appends to `status_file`, which reaches
/// the hook as `RALPHY_STATUS_FILE`. The hook `-c` values follow
/// `model_reasoning_effort` after `exec`: Codex drops a `-c` list given before
/// `exec` when another one follows it.
///
/// GUARD ASYMMETRY: `danger-full-access` runs with no equivalent of the Claude
/// adapter's PreToolUse guard hook — Ralphy gives Codex the status hooks only
/// — so safety here rests on the isolated run branch and the prompt's hard
/// rules.
pub(crate) fn build_codex_command(
    model: &str,
    effort: &str,
    root: &Path,
    out_path: &Path,
    hook_exe: &Path,
    status_file: &Path,
) -> Command {
    let mut cmd = Command::new("codex");
    cmd.arg("exec")
        .arg("-C")
        .arg(root)
        .arg("-m")
        .arg(model)
        .arg("-c")
        .arg(format!(
            "model_reasoning_effort=\"{}\"",
            codex_reasoning_effort(effort)
        ))
        .args(status_hook_args(root, hook_exe))
        .env(crate::status::STATUS_ENV, status_file)
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

/// The status hook arguments of a run in `root`, or none when the project
/// has Codex hooks of its own: the trust flag would run them with no trust
/// prompt, so the run goes without an agent state (ADR-0085 amendment of
/// 2026-10-29).
fn status_hook_args(root: &Path, hook_exe: &Path) -> Vec<String> {
    if hook_form::codex_project_has_hooks(root) {
        tracing::warn!(
            "this project has its own Codex hooks, so Ralphy does not turn on hook trust \
             for it; the run has no agent state"
        );
        return Vec::new();
    }
    hook_form::codex_hook_args(hook_exe, &hook_form::CODEX_STATUS_EVENTS)
}

// ── init one-shot sessions (ADR-0012 stages 2 & 8) ──────────────────────────

/// Resolve the Codex model for a one-shot `init` session: the explicit override,
/// then the user's Codex config model. `None` when neither names one — the init
/// command then OMITS `-m`, delegating to Codex's own recommended default
/// (ADR-0004, Amendment 2026-07-10). Init sessions have no plan tier to route,
/// which is why they delegate where plan/execute fall back to the family table.
pub(crate) fn resolve_init_model(model: Option<&str>) -> Option<String> {
    model.map(str::to_string).or_else(codex_config_model)
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
///
/// Each `images` path is attached with `-i <path>`, inserted BEFORE the trailing
/// `-` stdin marker (`-i`/`--image` is an option that must precede the `-`
/// positional prompt, per `codex exec --help`, ADR-0025 §4).
pub(crate) fn build_codex_init_command(
    model: Option<&str>,
    effort: &str,
    cwd: &Path,
    images: &[PathBuf],
) -> Command {
    let mut cmd = Command::new("codex");
    cmd.arg("exec")
        .arg("-C")
        .arg(cwd)
        .arg("--skip-git-repo-check");
    // `-m` only when the operator/config named a model; otherwise delegate to
    // Codex's own recommended default (ADR-0004, Amendment 2026-07-10).
    if let Some(m) = model {
        cmd.arg("-m").arg(m);
    }
    cmd.arg("-c")
        .arg(format!(
            "model_reasoning_effort=\"{}\"",
            codex_reasoning_effort(effort)
        ))
        .arg("-s")
        .arg("danger-full-access");
    for p in images {
        cmd.arg("-i").arg(p);
    }
    cmd.arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env_remove("OPENAI_API_KEY");
    cmd
}

#[cfg(test)]
mod tests;
