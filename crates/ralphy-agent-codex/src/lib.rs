//! The Codex CLI adapter: drives `codex exec` behind the core [`Agent`] contract.
//! Everything Codex-specific — the binary, the model and reasoning-effort flags,
//! the headless invocation, and the signal→[`Outcome`] mapping — is confined here.
//! See docs/adr/0004.
//!
//! Unlike the Claude adapter (a live PTY session with a Stop-hook flag file),
//! Codex needs no interactive session: `plan` and `execute` both run headless
//! `codex exec` with the prompt piped on stdin, and completion is detected from
//! Codex-native signals — the `RALPHY_DONE_EXIT`/`RALPHY_BLOCKED_EXIT` sentinels
//! in the `-o` final-message file, the process exit code, and a HEAD-diff commit
//! check — mapped onto the same core [`Outcome`].

use std::fs;
use std::io::{BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use include_dir::{include_dir, Dir};
use ralphy_core::{git, plan, Agent, Issue, Outcome, Plan, PlanLimit, Workspace};
use tracing::info;

/// The skills subtree, embedded at build time so the binary is self-contained.
/// Codex auto-discovers skills in `.agents/skills/`; we extract this tree there
/// before every plan and execute call so a run never depends on globally-installed
/// skills (mirrors `materialize_plugin` in the Claude adapter).
static SKILLS: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/../../assets/plugin/skills");

/// Materialize the embedded skills into `<repo>/.agents/skills/` so Codex's
/// auto-discovery finds them. Clears any prior copy, re-extracts fresh, and
/// writes `<repo>/.agents/.gitignore` (`*`) to keep the materialized tree out of
/// the executor's commits. Returns the `.agents/skills` path.
fn materialize_codex_skills(ws: &Workspace) -> Result<PathBuf> {
    let agents_dir = ws.repo_root().join(".agents");
    let skills_dir = agents_dir.join("skills");
    if skills_dir.exists() {
        fs::remove_dir_all(&skills_dir).context("clearing stale .agents/skills")?;
    }
    fs::create_dir_all(&skills_dir).context("creating .agents/skills")?;
    SKILLS
        .extract(&skills_dir)
        .context("extracting the embedded skills to .agents/skills")?;
    fs::write(agents_dir.join(".gitignore"), "*\n").context("writing .agents/.gitignore")?;
    Ok(skills_dir)
}

/// The last-resort Codex model, used only when neither `--exec-model` nor the
/// user's Codex config names one. ChatGPT-auth accounts reject `gpt-5-codex`, so
/// in practice the config-derived model (see [`codex_config_model`]) is what most
/// subscription runs use; this constant is the floor for an unconfigured setup.
const DEFAULT_CODEX_MODEL: &str = "gpt-5-codex";

/// The Codex planning prompt, embedded so the binary is self-contained as a global
/// tool. A variant of `prompt.plan.md` that emits a vendor-neutral
/// `low|medium|high` complexity tier (mapped to reasoning effort) instead of a
/// Claude model name. Single source of truth lives at `assets/prompts/`.
const PROMPT_PLAN_CODEX: &str = include_str!("../../../assets/prompts/prompt.plan.codex.md");

/// The vendor-neutral execution charter, piped to `codex exec` on stdin. Shared
/// verbatim with the Claude path — it already names the `RALPHY_DONE_EXIT` /
/// `RALPHY_BLOCKED_EXIT` sentinels and is not Claude-specific.
const PROMPT_EXECUTE: &str = include_str!("../../../assets/prompts/prompt.execute.md");

/// Drives the `codex` CLI. `model` is the operator override (else
/// [`DEFAULT_CODEX_MODEL`]); `run_dir` is where the captured logs live;
/// `max_minutes_per_issue` is the per-issue wall budget, clamped to `run_deadline`
/// when the run carries a global deadline.
pub struct CodexAgent {
    model: Option<String>,
    run_dir: PathBuf,
    max_minutes_per_issue: u64,
    run_deadline: Option<Instant>,
}

impl CodexAgent {
    pub fn new(model: Option<String>, run_dir: PathBuf) -> Self {
        Self {
            model,
            run_dir,
            max_minutes_per_issue: 45,
            run_deadline: None,
        }
    }

    /// Set the run's global wall-clock deadline (from `--deadline-hours`). Each
    /// issue's budget is then clamped to it, so an issue started just under the
    /// global limit can't overrun by a whole per-issue window (mirrors
    /// `ClaudeAgent::with_run_deadline`).
    pub fn with_run_deadline(mut self, run_deadline: Option<Instant>) -> Self {
        self.run_deadline = run_deadline;
        self
    }

    /// The deadline for the current issue: the per-issue budget, clamped to the
    /// run's global deadline when one is set.
    fn issue_deadline(&self) -> Instant {
        let per_issue = Instant::now() + Duration::from_secs(self.max_minutes_per_issue * 60);
        match self.run_deadline {
            Some(rd) => per_issue.min(rd),
            None => per_issue,
        }
    }

    /// The single model decision point, in precedence order: the explicit
    /// `--exec-model` override, then the `model` from the user's Codex config, then
    /// [`DEFAULT_CODEX_MODEL`]. Honouring the config means a ChatGPT-auth account —
    /// which rejects `gpt-5-codex` — picks up the model it is actually entitled to
    /// with no explicit flag. Codex routes complexity by reasoning effort, not a
    /// model swap (ADR-0004 D3), so this stays a single value.
    fn resolve_model(&self) -> String {
        if let Some(m) = self.model.as_deref() {
            return m.to_string();
        }
        codex_config_model().unwrap_or_else(|| DEFAULT_CODEX_MODEL.to_string())
    }
}

/// Locate the Codex config file: `$CODEX_HOME/config.toml` when `CODEX_HOME` is
/// set (matching Codex's own resolution), else `<home>/.codex/config.toml`
/// (`USERPROFILE` on Windows, `HOME` elsewhere). `None` when no home is known.
fn codex_config_path() -> Option<PathBuf> {
    if let Some(codex_home) = std::env::var_os("CODEX_HOME") {
        return Some(PathBuf::from(codex_home).join("config.toml"));
    }
    let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME"))?;
    Some(PathBuf::from(home).join(".codex").join("config.toml"))
}

/// The top-level `model = "..."` from the user's Codex config, if present and
/// readable. `None` when the file or the key is absent — the caller then falls
/// back to [`DEFAULT_CODEX_MODEL`].
fn codex_config_model() -> Option<String> {
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
fn recommended_tier(md: &str) -> Option<String> {
    use regex::Regex;
    let re =
        Regex::new(r"(?im)^\s*##\s*Execution model:\s*(low|medium|high)").expect("valid regex");
    re.captures(md).map(|c| c[1].to_lowercase())
}

/// Map a neutral complexity tier to the `model_reasoning_effort` value. Unknown or
/// absent tiers default to `medium` — the single tier→effort point (ADR-0004 D3).
fn tier_to_effort(tier: Option<&str>) -> &'static str {
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
fn build_codex_command(model: &str, effort: &str, root: &Path, out_path: &Path) -> Command {
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

/// Return `true` when `text` contains a Codex usage-limit message (case-insensitive).
fn is_codex_limit_text(text: &str) -> bool {
    // to_ascii_lowercase is used so byte offsets are preserved (ASCII-only pattern).
    let lower = text.to_ascii_lowercase();
    lower.contains("you've hit your usage limit")
        || lower.contains("usage limit")
        || lower.contains("rate limit reached")
}

/// Extract the reset hint from a Codex limit message: the text following
/// `try again at ` (trimmed, to end of line). Returns `None` when absent.
fn parse_codex_reset_hint(text: &str) -> Option<String> {
    for line in text.lines() {
        // to_ascii_lowercase preserves byte positions so the pos from find()
        // can safely index back into line (no Unicode expansion hazard).
        let lower = line.to_ascii_lowercase();
        if let Some(pos) = lower.find("try again at ") {
            let rest = line[pos + "try again at ".len()..].trim();
            if !rest.is_empty() {
                return Some(rest.to_string());
            }
        }
    }
    None
}

/// Map an execution call's end state onto a core [`Outcome`] (ADR-0004 D2):
/// the wall timeout wins (`Timeout`); a `RALPHY_BLOCKED_EXIT <reason>` sentinel is
/// `Blocked(reason)`; a clean exit that both committed and emitted
/// `RALPHY_DONE_EXIT` is `Done`; anything else — a non-zero exit, no new commit, or
/// no sentinel — is `Stuck`. The HEAD-diff `committed` check is the same progress
/// guard the Claude headless loop uses, so a `Done` claim with no commit is
/// distrusted and downgraded to `Stuck`.
fn classify_codex_outcome(
    exited_cleanly: bool,
    timed_out: bool,
    committed: bool,
    out: &str,
    log: &str,
) -> Outcome {
    if timed_out {
        return Outcome::Timeout;
    }
    if let Some(line) = out.lines().find(|l| l.contains("RALPHY_BLOCKED_EXIT")) {
        let reason = line
            .split_once("RALPHY_BLOCKED_EXIT")
            .map(|(_, rest)| rest.trim().to_string())
            .unwrap_or_default();
        return Outcome::Blocked(reason);
    }
    if exited_cleanly && committed && out.contains("RALPHY_DONE_EXIT") {
        return Outcome::Done;
    }
    if is_codex_limit_text(log) {
        return Outcome::Limit(parse_codex_reset_hint(log));
    }
    Outcome::Stuck
}

impl CodexAgent {
    /// Spawn a single `codex exec` call, piping `prompt` on stdin and draining
    /// stdout/stderr via reader threads to avoid pipe-buffer deadlock (mirrors
    /// `ClaudeAgent::run_headless_call`). Polls `try_wait` until `timeout`; kills
    /// the child on expiry. Returns `(exited_cleanly, timed_out, combined_log)` —
    /// the combined log is also written to `run_dir/codex.log`; the agent's final
    /// message is read from the `-o` file by the caller.
    fn run_codex(
        &self,
        mut cmd: Command,
        prompt: &str,
        timeout: Duration,
    ) -> Result<(bool, bool, String)> {
        let mut child = cmd
            .spawn()
            .context("failed to spawn the `codex` CLI (is it installed and on PATH?)")?;

        // Spawn the reader threads *before* writing stdin, so a prompt larger than
        // the pipe buffer (~64KB) can't deadlock against a child that starts
        // emitting output before it finishes draining stdin.
        let mut stdin = child.stdin.take().expect("stdin was piped");
        let stdout = child.stdout.take().expect("stdout was piped");
        let stderr = child.stderr.take().expect("stderr was piped");

        let (tx_out, rx_out) = mpsc::channel::<Vec<u8>>();
        let (tx_err, rx_err) = mpsc::channel::<Vec<u8>>();
        thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = BufReader::new(stdout).read_to_end(&mut buf);
            let _ = tx_out.send(buf);
        });
        thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = BufReader::new(stderr).read_to_end(&mut buf);
            let _ = tx_err.send(buf);
        });

        stdin
            .write_all(prompt.as_bytes())
            .context("piping the prompt to codex")?;
        drop(stdin); // close stdin so codex sees EOF

        let deadline = Instant::now() + timeout;
        let mut timed_out = false;
        let status = loop {
            if let Some(s) = child.try_wait().context("polling codex child")? {
                break Some(s);
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                timed_out = true;
                break None;
            }
            thread::sleep(Duration::from_millis(500));
        };

        let collect = Duration::from_secs(5);
        let stdout_bytes = rx_out.recv_timeout(collect).unwrap_or_default();
        let stderr_bytes = rx_err.recv_timeout(collect).unwrap_or_default();
        let mut text = String::from_utf8_lossy(&stdout_bytes).into_owned();
        text.push_str(&String::from_utf8_lossy(&stderr_bytes));
        let _ = fs::write(self.run_dir.join("codex.log"), &text);

        let exited_cleanly = status.map(|s| s.success()).unwrap_or(false);
        Ok((exited_cleanly, timed_out, text))
    }
}

impl Agent for CodexAgent {
    fn plan(&self, _issue: &Issue, ws: &Workspace) -> Result<Plan> {
        fs::create_dir_all(ws.ralphy_dir()).ok();
        fs::create_dir_all(&self.run_dir).ok();
        materialize_codex_skills(ws)?;

        let plan_path = ws.plan_path();
        // Plan fresh every run; never reuse a stale artifact.
        let _ = fs::remove_file(&plan_path);

        let model = self.resolve_model();
        let out_path = ws.ralphy_dir().join("codex-last.txt");
        let _ = fs::remove_file(&out_path);

        // Planning always runs at `high` effort (ADR-0004 D3).
        let cmd = build_codex_command(&model, "high", ws.repo_root(), &out_path);
        let timeout = self
            .issue_deadline()
            .saturating_duration_since(Instant::now());
        info!(model = %model, effort = "high", "planning with codex exec");
        let (_, _, log) = self.run_codex(cmd, PROMPT_PLAN_CODEX, timeout)?;

        if !plan_path.exists() {
            // A usage limit during planning is not a generic failure: surface it
            // as a typed `PlanLimit` (with the parsed reset hint) so the runner
            // routes it through the same stop-and-report / auto-resume path as an
            // execute-time `Outcome::Limit`, rather than aborting the whole run.
            if is_codex_limit_text(&log) {
                return Err(PlanLimit {
                    reset: parse_codex_reset_hint(&log),
                }
                .into());
            }
            bail!(
                "codex produced no plan at {} (see {})",
                plan_path.display(),
                self.run_dir.join("codex.log").display()
            );
        }
        let md = fs::read_to_string(&plan_path).context("reading the written plan.md")?;
        Ok(Plan {
            open_steps: plan::count_open_steps(&md),
            recommended_model: recommended_tier(&md),
            path: plan_path,
        })
    }

    fn execute(&self, plan: &Plan, ws: &Workspace) -> Result<Outcome> {
        fs::create_dir_all(&self.run_dir).ok();
        fs::create_dir_all(ws.ralphy_dir()).ok();
        materialize_codex_skills(ws)?;

        let model = self.resolve_model();
        // Execution takes the plan's neutral complexity tier as reasoning effort.
        let effort = tier_to_effort(plan.recommended_model.as_deref());
        let out_path = ws.ralphy_dir().join("codex-last.txt");
        let _ = fs::remove_file(&out_path);

        // HEAD before/after bounds the work this call committed (progress guard).
        let before_sha = git::head_sha(ws.repo_root()).unwrap_or_default();
        let timeout = self
            .issue_deadline()
            .saturating_duration_since(Instant::now());
        let cmd = build_codex_command(&model, effort, ws.repo_root(), &out_path);
        info!(model = %model, effort, "executing with codex exec");
        let (exited_cleanly, timed_out, log) = self.run_codex(cmd, PROMPT_EXECUTE, timeout)?;

        let after_sha = git::head_sha(ws.repo_root()).unwrap_or_default();
        let committed = before_sha != after_sha;
        let out = fs::read_to_string(&out_path).unwrap_or_default();

        let outcome = classify_codex_outcome(exited_cleanly, timed_out, committed, &out, &log);
        info!(
            ?outcome,
            exited_cleanly, timed_out, committed, "codex execution ended"
        );
        Ok(outcome)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    // ── classify_codex_outcome ──────────────────────────────────────────────

    #[test]
    fn classify_done_on_clean_exit_commit_and_sentinel() {
        let out = "all steps green\nRALPHY_DONE_EXIT\n";
        assert_eq!(
            classify_codex_outcome(true, false, true, out, ""),
            Outcome::Done
        );
    }

    #[test]
    fn classify_blocked_on_blocked_sentinel() {
        let out = "did some work\nRALPHY_BLOCKED_EXIT missing upstream crate\n";
        assert_eq!(
            classify_codex_outcome(true, false, true, out, ""),
            Outcome::Blocked("missing upstream crate".into())
        );
    }

    #[test]
    fn classify_stuck_on_non_zero_exit() {
        // A non-zero exit is Stuck even when the output carries a DONE sentinel.
        let out = "RALPHY_DONE_EXIT\n";
        assert_eq!(
            classify_codex_outcome(false, false, true, out, ""),
            Outcome::Stuck
        );
    }

    #[test]
    fn classify_stuck_on_no_commit() {
        // A DONE claim with no new commit is distrusted (HEAD-diff progress guard).
        let out = "RALPHY_DONE_EXIT\n";
        assert_eq!(
            classify_codex_outcome(true, false, false, out, ""),
            Outcome::Stuck
        );
    }

    #[test]
    fn classify_stuck_on_no_sentinel() {
        assert_eq!(
            classify_codex_outcome(true, false, true, "quiet exit, no sentinel", ""),
            Outcome::Stuck
        );
    }

    #[test]
    fn classify_timeout_wins() {
        // The wall timeout wins over everything, including a DONE sentinel.
        let out = "RALPHY_DONE_EXIT\n";
        assert_eq!(
            classify_codex_outcome(false, true, false, out, ""),
            Outcome::Timeout
        );
    }

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

    // ── resolve_model ───────────────────────────────────────────────────────

    #[test]
    fn resolve_model_override_wins() {
        // The explicit --exec-model override wins over config and default, with no
        // dependence on the machine's Codex config.
        let overridden = CodexAgent::new(Some("gpt-5".into()), PathBuf::from("/run"));
        assert_eq!(overridden.resolve_model(), "gpt-5");
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

    // ── trait binding (compile-level) ───────────────────────────────────────

    #[test]
    fn codex_agent_is_a_dyn_agent() {
        // Proves `CodexAgent: Agent` and that it can be handed to the core as a
        // `&dyn Agent` (the core never learns the vendor).
        let agent = CodexAgent::new(None, PathBuf::from("/run"));
        let _as_dyn: &dyn Agent = &agent;
    }

    // ── materialize_codex_skills ────────────────────────────────────────────

    #[test]
    fn materialize_codex_skills_extracts_required_skills() {
        let base = std::env::temp_dir().join(format!("ralphy-codex-skills-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        let ws = Workspace::new(&base);

        let skills_dir = materialize_codex_skills(&ws).expect("materialize");
        assert_eq!(skills_dir, ws.repo_root().join(".agents").join("skills"));
        assert!(
            skills_dir.join("reviewer/SKILL.md").is_file(),
            "reviewer/SKILL.md must be materialized"
        );
        assert!(
            skills_dir.join("staged-plan/SKILL.md").is_file(),
            "staged-plan/SKILL.md must be materialized"
        );
        assert!(
            skills_dir.join("reviewer/scripts/audit.py").is_file(),
            "reviewer/scripts/audit.py must be materialized"
        );
        assert!(
            ws.repo_root().join(".agents/.gitignore").is_file(),
            ".agents/.gitignore must be written"
        );

        // Idempotent: a second call clears and re-extracts cleanly.
        materialize_codex_skills(&ws).expect("re-materialize");
        assert!(skills_dir.join("reviewer/SKILL.md").is_file());

        let _ = fs::remove_dir_all(&base);
    }

    // ── is_codex_limit_text ─────────────────────────────────────────────────

    #[test]
    fn is_codex_limit_text_matches_known_phrases() {
        assert!(is_codex_limit_text(
            "Sorry, you've hit your usage limit for today."
        ));
        assert!(is_codex_limit_text("You've Hit Your Usage Limit"));
        assert!(is_codex_limit_text("usage limit exceeded"));
        assert!(is_codex_limit_text(
            "Error: Rate Limit Reached. Please try again later."
        ));
        assert!(!is_codex_limit_text("all steps green\nRALPHY_DONE_EXIT\n"));
    }

    // ── parse_codex_reset_hint ──────────────────────────────────────────────

    #[test]
    fn parse_codex_reset_hint_extracts_datetime() {
        let text = "You've hit your usage limit. Try again at 2026-06-09T18:00:00Z.";
        assert_eq!(
            parse_codex_reset_hint(text).as_deref(),
            Some("2026-06-09T18:00:00Z.")
        );
    }

    #[test]
    fn parse_codex_reset_hint_returns_none_when_absent() {
        assert_eq!(
            parse_codex_reset_hint("usage limit exceeded, no reset info"),
            None
        );
    }

    #[test]
    fn detects_real_codex_plan_limit_log() {
        // The exact ERROR line a `codex exec` plan emitted on a usage limit: the
        // adapter's plan() classifies this into a PlanLimit carrying the hint.
        let log = "ERROR: You've hit your usage limit. Upgrade to Pro \
                   (https://chatgpt.com/explore/pro), visit \
                   https://chatgpt.com/codex/settings/usage to purchase more \
                   credits or try again at Jun 10th, 2026 12:23 AM.";
        assert!(is_codex_limit_text(log));
        assert_eq!(
            parse_codex_reset_hint(log).as_deref(),
            Some("Jun 10th, 2026 12:23 AM.")
        );
    }

    // ── classify_codex_outcome — limit branch ───────────────────────────────

    #[test]
    fn classify_limit_with_reset_hint() {
        let log = "You've hit your usage limit. Try again at 2026-06-09T18:00:00Z.";
        assert_eq!(
            classify_codex_outcome(false, false, false, "", log),
            Outcome::Limit(Some("2026-06-09T18:00:00Z.".into()))
        );
    }

    #[test]
    fn classify_limit_bare_when_no_hint() {
        let log = "Error: usage limit exceeded.";
        assert_eq!(
            classify_codex_outcome(false, false, false, "", log),
            Outcome::Limit(None)
        );
    }

    // ── PROMPT_PLAN_CODEX reviewer step ────────────────────────────────────

    #[test]
    fn prompt_plan_codex_contains_reviewer_step() {
        assert!(
            PROMPT_PLAN_CODEX.contains("reviewer"),
            "planning prompt must reference the reviewer skill"
        );
        let lower = PROMPT_PLAN_CODEX.to_lowercase();
        assert!(
            lower.contains("only") && lower.contains("commits you made"),
            "reviewer step must scope to this issue's own commits"
        );
        assert!(
            !PROMPT_PLAN_CODEX.contains("independent subagent"),
            "must not use Claude 'independent subagent' phrasing"
        );
    }
}
