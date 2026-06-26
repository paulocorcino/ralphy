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
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use include_dir::{include_dir, Dir};
use ralphy_adapter_support::run_headless;
use ralphy_core::{
    build_diagnose_prompt, build_init_issues_prompt, git, plan, Agent, DiagnosisReport,
    DraftRequest, Execution, Issue, IssuesDraft, Outcome, Plan, PlanLimit, Usage, Workspace,
};
use tracing::info;

/// The skills subtree, embedded at build time so the binary is self-contained.
/// Codex auto-discovers skills in `.agents/skills/`; we extract this tree there
/// before every plan and execute call so a run never depends on globally-installed
/// skills (mirrors `materialize_plugin` in the Claude adapter).
static SKILLS: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/../../assets/plugin/skills");

/// Materialize the embedded skills into the canonical, ralphy-owned `.ralphy/skills`
/// store, then expose them to Codex by linking each one into `.agents/skills/<name>`.
///
/// Codex offers no way to point at a private skills directory: it only ever scans
/// the conventional `.agents/skills` hierarchy (CWD up to repo root, plus
/// `$HOME/.agents/skills` and `/etc/codex/skills`), and its sole skills config key,
/// `[[skills.config]]`, just toggles a skill on/off — there is no additional-path
/// setting (unlike OpenCode's `skills.paths`). `.agents/skills` is therefore a
/// user-owned, shared location we must NOT wipe.
///
/// So the real skill content lives in `.ralphy/skills` (cleared-and-replaced
/// wholesale, like the OpenCode adapter, and kept out of git by `.ralphy/.gitignore`),
/// and only per-skill symlinks are placed into `.agents/skills/<name>` —
/// **additively**, replacing just the subdirectories ralphy owns and leaving sibling
/// (user) skills intact. On Windows, where a symlink needs Developer Mode/admin, the
/// link falls back to copying the skill tree. A merged `.agents/skills/.gitignore`
/// keeps our entries out of the executor's commits without clobbering the user's own.
///
/// Returns the `.agents/skills` path Codex discovers.
fn materialize_codex_skills(ws: &Workspace) -> Result<PathBuf> {
    // 1. Canonical store: real files under `.ralphy/skills`, fully ralphy-owned, so
    //    clearing and re-extracting wholesale (and `.ralphy/.gitignore = *`) is safe.
    let store = ws.ralphy_dir().join("skills");
    ralphy_adapter_support::materialize_assets(&SKILLS, &store, Some(&ws.ralphy_dir()))?;

    // 2. Expose to Codex's discovery path additively: reuse `.agents/skills` if it
    //    already exists, else create it, and (re)link each ralphy skill into it.
    let skills_dir = ws.repo_root().join(".agents").join("skills");
    fs::create_dir_all(&skills_dir).context("creating .agents/skills")?;

    let mut names: Vec<std::ffi::OsString> = Vec::new();
    for skill in SKILLS.dirs() {
        let name = skill
            .path()
            .file_name()
            .context("embedded skill directory has no name")?
            .to_owned();
        let src = store.join(&name);
        let dest = skills_dir.join(&name);

        // Replace only our own subdir; never touch sibling (user) skills.
        if dest.symlink_metadata().is_ok() {
            remove_path(&dest).with_context(|| format!("clearing stale {}", dest.display()))?;
        }
        link_or_copy_dir(&src, &dest)
            .with_context(|| format!("exposing skill {}", name.to_string_lossy()))?;
        names.push(name);
    }

    // 3. Keep our linked skills out of the executor's commits, preserving any
    //    `.gitignore` the user already maintains in `.agents/skills`.
    ensure_gitignore_entries(&skills_dir.join(".gitignore"), &names)?;

    Ok(skills_dir)
}

/// Link `src` into `dest` as a directory symlink, falling back to a recursive copy
/// when the symlink is rejected on Windows (no Developer Mode / not elevated).
fn link_or_copy_dir(src: &Path, dest: &Path) -> Result<()> {
    match symlink_dir(src, dest) {
        Ok(()) => Ok(()),
        Err(_) if cfg!(windows) => copy_dir_all(src, dest)
            .with_context(|| format!("copying {} -> {}", src.display(), dest.display())),
        Err(e) => {
            Err(e).with_context(|| format!("symlinking {} -> {}", src.display(), dest.display()))
        }
    }
}

#[cfg(unix)]
fn symlink_dir(src: &Path, dest: &Path) -> Result<()> {
    std::os::unix::fs::symlink(src, dest).map_err(Into::into)
}

#[cfg(windows)]
fn symlink_dir(src: &Path, dest: &Path) -> Result<()> {
    std::os::windows::fs::symlink_dir(src, dest).map_err(Into::into)
}

/// Remove a path that may be a symlink, a real directory, or a file — without
/// following a symlink into its target. On Windows a directory symlink must be
/// removed via `remove_dir`, a file symlink via `remove_file`, so both are tried.
fn remove_path(p: &Path) -> Result<()> {
    let ft = fs::symlink_metadata(p)?.file_type();
    if ft.is_symlink() {
        #[cfg(windows)]
        {
            fs::remove_file(p).or_else(|_| fs::remove_dir(p))?;
        }
        #[cfg(unix)]
        {
            fs::remove_file(p)?;
        }
    } else if ft.is_dir() {
        fs::remove_dir_all(p)?;
    } else {
        fs::remove_file(p)?;
    }
    Ok(())
}

/// Recursively copy `src` into `dest` (the Windows fallback when symlinks are
/// unavailable). Creates `dest` and every intermediate directory.
fn copy_dir_all(src: &Path, dest: &Path) -> Result<()> {
    fs::create_dir_all(dest)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let from = entry.path();
        let to = dest.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir_all(&from, &to)?;
        } else {
            fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

/// Ensure a `/<name>` ignore line exists for each ralphy skill — plus a
/// `/.gitignore` self-ignore — in `.agents/skills/.gitignore`, appending only
/// what's missing so any entries the user already keeps there survive. The
/// self-ignore keeps the file itself from being the one untracked thing that
/// surfaces `.agents/` in `git status`. Idempotent: a no-op once the lines exist.
fn ensure_gitignore_entries(path: &Path, names: &[std::ffi::OsString]) -> Result<()> {
    let existing = fs::read_to_string(path).unwrap_or_default();
    let mut lines: Vec<String> = existing.lines().map(str::to_string).collect();
    let mut changed = false;
    // Self-ignore `.gitignore` itself (`/.gitignore`) alongside each skill subdir.
    // Without the self-entry this file is the lone unignored thing left in
    // `.agents/skills/`, so `.agents/` shows as untracked and dirties the working
    // tree — aborting the next run's clean-tree check. (The OpenCode adapter avoids
    // this with a blanket `.ralphy/.gitignore = *`; Codex shares `.agents/skills`
    // with the user's own skills, so it ignores precisely its own subdirs plus
    // this file rather than the whole directory.)
    let entries = std::iter::once("/.gitignore".to_string())
        .chain(names.iter().map(|n| format!("/{}", n.to_string_lossy())));
    for entry in entries {
        if !lines.iter().any(|l| l.trim() == entry) {
            lines.push(entry);
            changed = true;
        }
    }
    if changed {
        let mut out = lines.join("\n");
        out.push('\n');
        fs::write(path, out).with_context(|| format!("writing {}", path.display()))?;
    }
    Ok(())
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
            max_minutes_per_issue: 90,
            run_deadline: None,
        }
    }

    /// Set the per-issue wall-clock budget in minutes (mirrors `ClaudeAgent::with_max_minutes_per_issue`).
    pub fn with_max_minutes_per_issue(mut self, minutes: u64) -> Self {
        self.max_minutes_per_issue = minutes;
        self
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

// ── init one-shot sessions (ADR-0012 stages 2 & 8) ──────────────────────────

/// Resolve the Codex model for a one-shot `init` session: the explicit override,
/// then the user's Codex config model, then [`DEFAULT_CODEX_MODEL`]. Mirrors
/// [`CodexAgent::resolve_model`] without needing an agent instance.
fn resolve_init_model(model: Option<&str>) -> String {
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
fn build_codex_init_command(model: &str, effort: &str, cwd: &Path) -> Command {
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

/// Run a one-shot headless `codex exec` repo-diagnosis session (ADR-0012 stage 2)
/// from `neutral_cwd` — a directory OUTSIDE the target repo, so Codex never
/// auto-loads the target's `AGENTS.md` as instructions. The target `repo` is
/// passed as data in the prompt; the session writes its JSON report to
/// `<neutral_cwd>/diagnosis.json`, which this function reads, validates against
/// [`DiagnosisReport`], and returns. Mirrors the Claude adapter's
/// `diagnose_repo` signature so the cli can dispatch on the selected agent.
pub fn diagnose_repo(
    repo: &Path,
    neutral_cwd: &Path,
    model: Option<&str>,
    effort: Option<&str>,
    timeout: Duration,
) -> Result<DiagnosisReport> {
    fs::create_dir_all(neutral_cwd).ok();
    let out_path = neutral_cwd.join("diagnosis.json");
    // A stale report from a prior run must never masquerade as this session's
    // output, so clear it before the session runs.
    let _ = fs::remove_file(&out_path);

    let model = resolve_init_model(model);
    let effort = effort.unwrap_or("medium");
    let prompt = build_diagnose_prompt(repo, &out_path);

    info!(%model, effort, "diagnosing repo with codex exec");
    let cmd = build_codex_init_command(&model, effort, neutral_cwd);
    let r = run_headless(cmd, &prompt, timeout)
        .context("failed to spawn the `codex` CLI (is it installed and on PATH?)")?;
    let mut log = r.stdout;
    log.push_str(&r.stderr);
    let log_path = neutral_cwd.join("diagnose.log");
    let _ = fs::write(&log_path, &log);

    if is_codex_auth_error(&log) {
        bail!("{} (see {})", CODEX_AUTH_ERROR_MSG, log_path.display());
    }
    if r.timed_out {
        bail!(
            "diagnosis session hit the wall timeout (see {})",
            log_path.display()
        );
    }

    let raw = fs::read_to_string(&out_path).with_context(|| {
        format!(
            "diagnosis session left no report at {} (see {})",
            out_path.display(),
            log_path.display()
        )
    })?;
    serde_json::from_str(&raw).with_context(|| {
        format!(
            "diagnosis report at {} did not match the schema",
            out_path.display()
        )
    })
}

/// Run a one-shot headless `codex exec` backlog/milestone → issues session
/// (ADR-0012 stage 8). Unlike [`diagnose_repo`] this runs IN the repo cwd — it
/// needs the repo's domain glossary/ADRs and (on the milestone path) writes a PRD
/// under `docs/prd/`. The session writes its [`IssuesDraft`] JSON to `out_path`,
/// which this function reads, validates against the schema, and returns. It NEVER
/// publishes to GitHub — that is the cli's job after the dev confirms.
pub fn draft_issues(
    repo: &Path,
    out_path: &Path,
    req: &DraftRequest,
    model: Option<&str>,
    effort: Option<&str>,
    timeout: Duration,
) -> Result<IssuesDraft> {
    if let Some(parent) = out_path.parent() {
        fs::create_dir_all(parent).ok();
    }
    // A stale draft from a prior run must never masquerade as this session's
    // output, so clear it before the session runs.
    let _ = fs::remove_file(out_path);

    let model = resolve_init_model(model);
    let effort = effort.unwrap_or("medium");
    let prompt =
        build_init_issues_prompt(repo, req.mode, req.source_docs, req.triage_label, out_path);

    info!(%model, effort, mode = req.mode.as_str(), "drafting issues with codex exec");
    let cmd = build_codex_init_command(&model, effort, repo);
    let r = run_headless(cmd, &prompt, timeout)
        .context("failed to spawn the `codex` CLI (is it installed and on PATH?)")?;
    let mut log = r.stdout;
    log.push_str(&r.stderr);
    let log_path = repo.join(".ralphy").join("init-issues.log");
    if let Some(parent) = log_path.parent() {
        fs::create_dir_all(parent).ok();
    }
    let _ = fs::write(&log_path, &log);

    if is_codex_auth_error(&log) {
        bail!("{} (see {})", CODEX_AUTH_ERROR_MSG, log_path.display());
    }
    if r.timed_out {
        bail!(
            "backlog → issues session hit the wall timeout (see {})",
            log_path.display()
        );
    }

    let raw = fs::read_to_string(out_path).with_context(|| {
        format!(
            "issues session left no draft at {} (see {})",
            out_path.display(),
            log_path.display()
        )
    })?;
    serde_json::from_str(&raw).with_context(|| {
        format!(
            "issues draft at {} did not match the schema",
            out_path.display()
        )
    })
}

/// The actionable message surfaced when a run hits a Codex authentication
/// failure — the account is signed out or its credentials were revoked.
const CODEX_AUTH_ERROR_MSG: &str =
    "Codex is not authenticated (401 Unauthorized) — run `codex login` and retry";

/// Return `true` when `text` shows a Codex authentication failure (account
/// signed out / credentials revoked). A logged-out `codex exec` prints a `401
/// Unauthorized` with `Missing bearer or basic authentication in header` and
/// writes no `-o` file, so without this the failure masquerades as a generic
/// "no plan" (planning) or `Outcome::Stuck` (execution) — both of which hide the
/// real cause. Either signal alone is auth-specific; matching either keeps the
/// detector robust to Codex reformatting one of the two lines.
fn is_codex_auth_error(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    lower.contains("401 unauthorized") || lower.contains("missing bearer or basic authentication")
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
    if let Some(reason) = ralphy_adapter_support::blocked_reason(out) {
        return Outcome::Blocked(reason);
    }
    if exited_cleanly && committed && ralphy_adapter_support::done_sentinel(out) {
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
        cmd: Command,
        prompt: &str,
        timeout: Duration,
    ) -> Result<(bool, bool, String)> {
        // Delegate the OS-level spawn/drain/poll/kill/collect plumbing to the
        // shared headless runner; Codex's `exited_cleanly` (a *successful* exit,
        // not merely "not timed out") is recovered from the returned exit status,
        // which is `None` exactly when the child was killed on the wall timeout.
        let r = run_headless(cmd, prompt, timeout)
            .context("failed to spawn the `codex` CLI (is it installed and on PATH?)")?;

        let mut text = r.stdout;
        text.push_str(&r.stderr);
        let _ = fs::write(self.run_dir.join("codex.log"), &text);

        let exited_cleanly = r.exit.map(|s| s.success()).unwrap_or(false);
        Ok((exited_cleanly, r.timed_out, text))
    }
}

/// Parse the cumulative token usage out of a Codex rollout JSONL (ADR-0008 D5).
///
/// Codex writes `event_msg` lines whose `payload.type == "token_count"` carry a
/// `payload.info.total_token_usage` snapshot that is **cumulative** for the
/// session; many `info` objects are `{}` or `null`, so the parser keeps the LAST
/// populated `total_token_usage` (the final cumulative figure for the phase).
///
/// Mapping (the load-bearing trap): Codex's `input_tokens` **includes** the
/// cached subset, so `input = input_tokens − cached_input_tokens` and
/// `cache_read = cached_input_tokens` — summing raw would double-count.
/// `cache_creation` is `0` (Codex reports no cache-write split) and
/// `reasoning_output_tokens` is NOT added: Codex's own `total_tokens` reconciles
/// as `input_tokens + output_tokens`, so reasoning already sits inside `output`.
/// `model` is the resolved invocation model (the rollout has no per-event model).
fn parse_codex_rollout_usage(jsonl: &str, model: Option<String>) -> Usage {
    let mut last: Option<serde_json::Value> = None;
    for line in jsonl.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        // opencode-style envelope: the real fields live under `payload`; fall
        // back to the value itself so a flat shape still parses.
        let payload = value.get("payload").unwrap_or(&value);
        if payload.get("type").and_then(|v| v.as_str()) != Some("token_count") {
            continue;
        }
        let Some(ttu) = payload
            .get("info")
            .and_then(|info| info.get("total_token_usage"))
        else {
            continue;
        };
        // Keep only populated snapshots; `{}`/null `info` must not clobber the
        // last good cumulative total.
        if ttu.is_object() {
            last = Some(ttu.clone());
        }
    }

    let mut usage = Usage {
        model,
        ..Usage::default()
    };
    if let Some(ttu) = last {
        let field = |k: &str| ttu.get(k).and_then(|v| v.as_u64()).unwrap_or(0);
        let cached = field("cached_input_tokens");
        usage.input = field("input_tokens").saturating_sub(cached);
        usage.output = field("output_tokens");
        usage.cache_read = cached;
        usage.cache_creation = 0;
    }
    usage
}

/// `$CODEX_HOME/sessions` when `CODEX_HOME` is set (matching Codex's own
/// resolution), else `<home>/.codex/sessions` (`USERPROFILE` on Windows, `HOME`
/// elsewhere) — the tree Codex writes `rollout-*.jsonl` session logs into.
/// `None` when no home is known. Mirrors the home logic in [`codex_config_path`].
fn codex_sessions_dir() -> Option<PathBuf> {
    if let Some(codex_home) = std::env::var_os("CODEX_HOME") {
        return Some(PathBuf::from(codex_home).join("sessions"));
    }
    let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME"))?;
    Some(PathBuf::from(home).join(".codex").join("sessions"))
}

/// Recursively collect `rollout-*.jsonl` files under `dir` (Codex nests them by
/// `<YYYY>/<MM>/<DD>/`). Empty when `dir` is missing or unreadable — best-effort,
/// never failing the run.
fn list_rollouts(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(list_rollouts(&path));
        } else if path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with("rollout-") && n.ends_with(".jsonl"))
        {
            out.push(path);
        }
    }
    out
}

/// Files present in `after` but not in `before` — the appeared-over-grew rule
/// (ADR-0008 D10), mirroring the Claude adapter's `appeared_files`. A file that
/// merely grew (present in both) is a concurrent pre-existing session and is
/// excluded; only a freshly *appeared* rollout is attributed to this run.
fn appeared(before: &[PathBuf], after: &[PathBuf]) -> Vec<PathBuf> {
    after
        .iter()
        .filter(|p| !before.contains(p))
        .cloned()
        .collect()
}

/// Snapshot-diff token capture: for each rollout that APPEARED between `before`
/// and `after`, parse its cumulative usage and fold into one `Usage` (the model
/// carried from the resolved invocation model). Mirrors `ClaudeAgent::execute`'s
/// before/after/appeared/fold.
fn fold_rollout_usage(before: &[PathBuf], after: &[PathBuf], model: Option<String>) -> Usage {
    let seed = Usage {
        model: model.clone(),
        ..Usage::default()
    };
    appeared(before, after)
        .iter()
        .filter_map(|p| fs::read_to_string(p).ok())
        .map(|t| parse_codex_rollout_usage(&t, model.clone()))
        .fold(seed, |mut acc, u| {
            acc.add_tokens(&u);
            acc
        })
}

impl Agent for CodexAgent {
    fn name(&self) -> &'static str {
        "codex"
    }

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

        // Snapshot the rollout tree around the call: a file that APPEARED is this
        // run's session, while one that merely grew is a concurrent pre-existing
        // session and is excluded (ADR-0008 D10, appeared-over-grew).
        let sessions_dir = codex_sessions_dir();
        let before = sessions_dir
            .as_deref()
            .map(list_rollouts)
            .unwrap_or_default();

        // Planning always runs at `high` effort (ADR-0004 D3).
        let cmd = build_codex_command(&model, "high", ws.repo_root(), &out_path);
        let timeout = self
            .issue_deadline()
            .saturating_duration_since(Instant::now());
        info!(model = %model, effort = "high", "planning with codex exec");
        let (_, _, log) = self.run_codex(cmd, PROMPT_PLAN_CODEX, timeout)?;
        let after = sessions_dir
            .as_deref()
            .map(list_rollouts)
            .unwrap_or_default();

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
            // An auth failure won't self-heal (unlike a usage limit), so stop the
            // run with an actionable message instead of a generic "no plan".
            if is_codex_auth_error(&log) {
                bail!(
                    "{CODEX_AUTH_ERROR_MSG} (see {})",
                    self.run_dir.join("codex.log").display()
                );
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
            usage: fold_rollout_usage(&before, &after, Some(model)),
        })
    }

    fn execute(&self, plan: &Plan, ws: &Workspace) -> Result<Execution> {
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
        // Snapshot the rollout tree around the call for appeared-over-grew token
        // capture (ADR-0008 D10), the same rule the Claude adapter uses.
        let sessions_dir = codex_sessions_dir();
        let before = sessions_dir
            .as_deref()
            .map(list_rollouts)
            .unwrap_or_default();
        let timeout = self
            .issue_deadline()
            .saturating_duration_since(Instant::now());
        let cmd = build_codex_command(&model, effort, ws.repo_root(), &out_path);
        info!(model = %model, effort, "executing with codex exec");
        let (exited_cleanly, timed_out, log) = self.run_codex(cmd, PROMPT_EXECUTE, timeout)?;
        let after = sessions_dir
            .as_deref()
            .map(list_rollouts)
            .unwrap_or_default();

        // A signed-out account never makes progress: stop the run with an
        // actionable message rather than letting it fall through to `Stuck`.
        if is_codex_auth_error(&log) {
            bail!(
                "{CODEX_AUTH_ERROR_MSG} (see {})",
                self.run_dir.join("codex.log").display()
            );
        }

        let after_sha = git::head_sha(ws.repo_root()).unwrap_or_default();
        let committed = before_sha != after_sha;
        let out = fs::read_to_string(&out_path).unwrap_or_default();

        let outcome = classify_codex_outcome(exited_cleanly, timed_out, committed, &out, &log);
        info!(
            ?outcome,
            exited_cleanly, timed_out, committed, "codex execution ended"
        );
        Ok(Execution {
            outcome,
            usage: fold_rollout_usage(&before, &after, Some(model)),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    // ── with_max_minutes_per_issue ──────────────────────────────────────────

    #[test]
    fn codex_honours_max_minutes_per_issue() {
        assert_eq!(
            CodexAgent::new(None, PathBuf::from("/run")).max_minutes_per_issue,
            90
        );
        let a = CodexAgent::new(None, PathBuf::from("/run")).with_max_minutes_per_issue(120);
        assert_eq!(a.max_minutes_per_issue, 120);
        let short = CodexAgent::new(None, PathBuf::from("/run")).with_max_minutes_per_issue(1);
        let long = CodexAgent::new(None, PathBuf::from("/run")).with_max_minutes_per_issue(1000);
        assert!(long.issue_deadline() > short.issue_deadline());
        let rd = Instant::now() + Duration::from_secs(1);
        let clamped = CodexAgent::new(None, PathBuf::from("/run"))
            .with_max_minutes_per_issue(1000)
            .with_run_deadline(Some(rd));
        assert!(clamped.issue_deadline() <= rd);
    }

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

        // The real skill content lives in the canonical `.ralphy/skills` store.
        let store = ws.ralphy_dir().join("skills");
        assert!(
            store.join("reviewer/SKILL.md").is_file(),
            "reviewer/SKILL.md must land in the .ralphy/skills store"
        );
        assert!(
            ws.ralphy_dir().join(".gitignore").is_file(),
            ".ralphy/.gitignore must be written"
        );

        // Codex's discovery path resolves each skill (through the symlink, or the
        // Windows copy fallback) to the same content.
        assert!(
            skills_dir.join("reviewer/SKILL.md").is_file(),
            "reviewer/SKILL.md must resolve under .agents/skills"
        );
        assert!(
            skills_dir.join("staged-plan/SKILL.md").is_file(),
            "staged-plan/SKILL.md must resolve under .agents/skills"
        );
        assert!(
            skills_dir.join("reviewer/scripts/audit.py").is_file(),
            "reviewer/scripts/audit.py must resolve under .agents/skills"
        );

        // The merged ignore lists our skills without a `*` that would swallow the
        // user's sibling skills in `.agents/skills`.
        let gi = fs::read_to_string(skills_dir.join(".gitignore")).expect("read .gitignore");
        assert!(
            gi.lines().any(|l| l.trim() == "/reviewer"),
            "gitignore: {gi:?}"
        );
        assert!(
            gi.lines().any(|l| l.trim() == "/staged-plan"),
            "gitignore: {gi:?}"
        );
        // The ignore self-ignores, so `.gitignore` is not the lone untracked file
        // that would surface `.agents/` in `git status` and dirty the tree.
        assert!(
            gi.lines().any(|l| l.trim() == "/.gitignore"),
            "gitignore must self-ignore: {gi:?}"
        );

        // Idempotent: a second call re-links cleanly and adds no duplicate entries.
        materialize_codex_skills(&ws).expect("re-materialize");
        assert!(skills_dir.join("reviewer/SKILL.md").is_file());
        let gi2 = fs::read_to_string(skills_dir.join(".gitignore")).expect("read .gitignore");
        assert_eq!(
            gi2.lines().filter(|l| l.trim() == "/reviewer").count(),
            1,
            "re-materialize must not duplicate ignore entries: {gi2:?}"
        );

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn materialize_codex_skills_preserves_user_skills() {
        // The defect this guards: materializing ralphy's skills must NOT wipe a
        // skill the user already keeps in the shared `.agents/skills` location, nor
        // overwrite their `.agents/.gitignore`. Only ralphy's own subdirs are touched.
        let base =
            std::env::temp_dir().join(format!("ralphy-codex-userskill-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        let ws = Workspace::new(&base);

        // A pre-existing user skill and a user-maintained .agents/skills/.gitignore.
        let user_skill = ws.repo_root().join(".agents/skills/my-skill");
        fs::create_dir_all(&user_skill).unwrap();
        fs::write(user_skill.join("SKILL.md"), b"user skill").unwrap();
        let user_gitignore = ws.repo_root().join(".agents/skills/.gitignore");
        fs::write(&user_gitignore, b"my-secret\n").unwrap();

        materialize_codex_skills(&ws).expect("materialize");

        // ralphy's skills landed...
        assert!(ws
            .repo_root()
            .join(".agents/skills/reviewer/SKILL.md")
            .is_file());
        // ...and the user's skill survived untouched.
        assert!(
            user_skill.join("SKILL.md").is_file(),
            "user skill must be preserved"
        );
        // The user's gitignore line is preserved and ours are merged in, not
        // overwritten.
        let gi = fs::read_to_string(&user_gitignore).unwrap();
        assert!(
            gi.lines().any(|l| l.trim() == "my-secret"),
            "gitignore: {gi:?}"
        );
        assert!(
            gi.lines().any(|l| l.trim() == "/reviewer"),
            "gitignore: {gi:?}"
        );

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

    // ── is_codex_auth_error ─────────────────────────────────────────────────

    #[test]
    fn is_codex_auth_error_matches_real_logged_out_log() {
        // The verbatim stderr a `codex exec` (v0.138.0) emitted with the account
        // signed out: a 401 with the missing-bearer body and reconnect attempts.
        let log = "ERROR codex_api::endpoint::responses_websocket: failed to connect \
                   to websocket: HTTP error: 401 Unauthorized, url: \
                   wss://api.openai.com/v1/responses\nERROR: Reconnecting... 5/5\n\
                   ERROR: unexpected status 401 Unauthorized: Missing bearer or basic \
                   authentication in header, url: https://api.openai.com/v1/responses";
        assert!(is_codex_auth_error(log));
    }

    #[test]
    fn is_codex_auth_error_matches_either_signal_alone() {
        assert!(is_codex_auth_error("HTTP error: 401 Unauthorized"));
        assert!(is_codex_auth_error(
            "Missing bearer or basic authentication in header"
        ));
        // Case-insensitive.
        assert!(is_codex_auth_error("401 UNAUTHORIZED"));
    }

    #[test]
    fn is_codex_auth_error_ignores_unrelated_and_limit_text() {
        assert!(!is_codex_auth_error("all steps green\nRALPHY_DONE_EXIT\n"));
        // A usage limit is a different failure, not an auth error.
        assert!(!is_codex_auth_error(
            "You've hit your usage limit. Try again at 2026-06-09T18:00:00Z."
        ));
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

    // ── parse_codex_rollout_usage ───────────────────────────────────────────

    #[test]
    fn parse_codex_rollout_usage_maps_cached_subtract_and_keeps_last() {
        // A real rollout shape: an empty `info`, then a populated cumulative
        // `total_token_usage`, then a trailing null `info` that must NOT clobber
        // the last good snapshot. `input_tokens` includes the cached subset, so
        // the mapped `input` is `841957 - 735616`; reasoning is already inside
        // `output` so it is not added.
        let jsonl = concat!(
            r#"{"type":"event_msg","payload":{"type":"token_count","info":{}}}"#,
            "\n",
            r#"{"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":841957,"cached_input_tokens":735616,"output_tokens":10466,"reasoning_output_tokens":4242,"total_tokens":852423},"last_token_usage":{"input_tokens":10}}}}"#,
            "\n",
            r#"{"type":"event_msg","payload":{"type":"token_count","info":null}}"#,
            "\n",
        );
        let usage = parse_codex_rollout_usage(jsonl, Some("gpt-5-codex".into()));
        assert_eq!(usage.input, 106341, "input = input_tokens - cached");
        assert_eq!(usage.output, 10466, "output excludes reasoning");
        assert_eq!(usage.cache_read, 735616);
        assert_eq!(usage.cache_creation, 0);
        assert_eq!(usage.model.as_deref(), Some("gpt-5-codex"));
        assert_eq!(usage.total(), 852423, "reconciles with Codex's own total");
    }

    #[test]
    fn parse_codex_rollout_usage_empty_keeps_model() {
        // No populated event → zeroed counts, but the model is preserved.
        let usage = parse_codex_rollout_usage("not json\n{}\n", Some("gpt-5-codex".into()));
        assert_eq!(usage.total(), 0);
        assert_eq!(usage.model.as_deref(), Some("gpt-5-codex"));
    }

    // ── appeared ────────────────────────────────────────────────────────────

    #[test]
    fn appeared_returns_new_not_grown() {
        let before = vec![PathBuf::from("/s/a.jsonl")];
        let after = vec![PathBuf::from("/s/a.jsonl"), PathBuf::from("/s/b.jsonl")];
        // `a.jsonl` was present before (it merely grew) → excluded; only the
        // freshly appeared `b.jsonl` is this run's rollout.
        assert_eq!(appeared(&before, &after), vec![PathBuf::from("/s/b.jsonl")]);
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
