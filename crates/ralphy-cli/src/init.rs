//! `ralphy init`: deterministic environment gate (ADR-0012 stage 1), then a
//! read-only repo diagnosis from a neutral cwd (stage 2) and a diagnosis-seeded
//! console Q&A captured into a typed config (stage 3), the git-safety snapshot +
//! `ralphy/init` branch (stage 4), the deterministic scaffold from the embedded
//! setup-pocock templates (stage 5), the optional sparse-checkout download of
//! engineering skills pinned to `RALPHY_VERSION` (stage 6), the idempotent
//! GitHub label vocabulary creation (stage 7), the conditional
//! backlog/milestone → issues judgment with a local preview the dev confirms
//! before any publish (stage 8), the `init-state.json` checkpoint (stage 9),
//! and the static verification + final report with an optional dry-run smoke
//! test (stage 10).

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use clap::{Args, ValueEnum};
use console::Style;
use ralphy_adapter_support::{find_program, locate_program, resolve_program};
use ralphy_core::{
    git, github, gitignore, DiagnosisReport, DraftRequest, IssuesDraft, IssuesMode, RepoKind,
    Workspace,
};
use serde::{Deserialize, Serialize};

#[derive(Args)]
pub struct InitArgs {
    /// Any path inside the target repo; resolved to its git toplevel.
    #[arg(long, default_value = ".")]
    pub repo: PathBuf,

    /// Which agent CLI drives the AI judgment steps (repo diagnosis + issue
    /// drafting). Must be logged in. Defaults to the first logged-in agent the
    /// environment gate detects (claude, then codex, then opencode).
    #[arg(long, value_enum)]
    pub agent: Option<Agent>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Agent {
    Claude,
    Codex,
    Opencode,
}

impl Agent {
    pub const ALL: [Agent; 3] = [Agent::Claude, Agent::Codex, Agent::Opencode];

    pub fn cli_name(&self) -> &'static str {
        match self {
            Agent::Claude => "claude",
            Agent::Codex => "codex",
            Agent::Opencode => "opencode",
        }
    }
}

pub struct EnvFindings {
    pub python: bool,
    pub gh_authenticated: bool,
    pub github_remote: bool,
    pub agents_present: Vec<Agent>,
    pub agents_logged_in: Vec<Agent>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HardFail {
    MissingPython,
    GhNotAuthenticated,
    NoGithubRemote,
    NoAgentCli,
    NoAgentLoggedIn,
}

/// Pure gate evaluation: returns all hard failures given the environment findings.
/// The agent-login rule fires only when ≥1 agent is present.
pub fn evaluate_gate(f: &EnvFindings) -> Vec<HardFail> {
    let mut fails = Vec::new();
    if !f.python {
        fails.push(HardFail::MissingPython);
    }
    if !f.gh_authenticated {
        fails.push(HardFail::GhNotAuthenticated);
    }
    if !f.github_remote {
        fails.push(HardFail::NoGithubRemote);
    }
    if f.agents_present.is_empty() {
        fails.push(HardFail::NoAgentCli);
    } else if f.agents_logged_in.is_empty() {
        fails.push(HardFail::NoAgentLoggedIn);
    }
    fails
}

/// Pure report formatter. Produces a human-readable string with one line per
/// prerequisite and a summary. The substrings `"<name>: logged in"` and
/// `"<name>: not logged in"` are guaranteed for present agents so tests can
/// assert them literally.
pub fn format_report(f: &EnvFindings, fails: &[HardFail]) -> String {
    let mut out = String::new();

    let py = if f.python { "ok" } else { "MISSING" };
    out.push_str(&format!("python:        {py}\n"));

    let gh = if f.gh_authenticated {
        "ok"
    } else {
        "NOT AUTHENTICATED"
    };
    out.push_str(&format!("gh auth:       {gh}\n"));

    let remote = if f.github_remote {
        "ok"
    } else {
        "NO GITHUB REMOTE"
    };
    out.push_str(&format!("github remote: {remote}\n"));

    out.push_str("agents:\n");
    for agent in &Agent::ALL {
        let name = agent.cli_name();
        let present = f.agents_present.contains(agent);
        if present {
            let logged_in = f.agents_logged_in.contains(agent);
            if logged_in {
                out.push_str(&format!("  {name}: logged in\n"));
            } else {
                out.push_str(&format!("  {name}: not logged in\n"));
            }
        } else {
            out.push_str(&format!("  {name}: absent\n"));
        }
    }

    let blocker_count = fails.len();
    if blocker_count == 0 {
        out.push_str("result: all checks passed\n");
    } else {
        out.push_str(&format!("result: {blocker_count} blocker(s)\n"));
    }
    out
}

// ── impure probes ──────────────────────────────────────────────────────────

fn python_present() -> bool {
    let path = std::env::var_os("PATH");
    let pathext = std::env::var_os("PATHEXT");
    find_program("python", path.clone(), pathext.clone()).is_some()
        || find_program("python3", path, pathext).is_some()
}

fn gh_authenticated() -> bool {
    std::process::Command::new("gh")
        .args(["auth", "status"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn github_remote(repo: &Path) -> bool {
    git::origin_url(repo)
        .map(|url| url.contains("github.com"))
        .unwrap_or(false)
}

// The gate's presence/login probes resolve each CLI through the SAME locator the
// adapters spawn through (`locate_program`/`resolve_program`), so detection and
// execution agree — a `claude` under `~/.local/bin` but off `PATH` is reported
// present and is the binary actually run, rather than being falsely called absent.
fn agent_present(a: &Agent) -> bool {
    locate_program(a.cli_name()).is_some()
}

pub(crate) fn agent_logged_in(a: &Agent) -> bool {
    let hello = "hello";
    let bin = resolve_program(a.cli_name());
    let mut cmd = std::process::Command::new(&bin);
    match a {
        Agent::Claude => {
            cmd.args(["-p", hello]);
        }
        Agent::Codex => {
            cmd.args(["exec", hello]);
            cmd.env_remove("OPENAI_API_KEY");
        }
        Agent::Opencode => {
            cmd.args(["run", hello]);
        }
    };
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

// ── diagnosis → Q&A → typed config (ADR-0012 stages 2–3) ────────────────────

/// The typed config the interactive Q&A captures — the dev's confirmed/corrected
/// view of the [`DiagnosisReport`]. Each field mirrors a report field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InitConfig {
    pub repo_kind: RepoKind,
    pub language_build: Option<String>,
    pub backlog_location: Option<String>,
    pub milestone_docs: Vec<String>,
    pub skills_dir: Option<String>,
    pub has_context_or_adrs: bool,
    pub remote_host: Option<String>,
    pub adopt_prd_roadmap: bool,
}

/// One onboarding stage of [`run`] that the checkpoint tracks as completed
/// (ADR-0012 stage 9). Recorded so a re-run skips a stage already done.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    Diagnose,
    Git,
    Scaffold,
    Skills,
    Labels,
    Issues,
}

/// The `ralphy init` checkpoint persisted to `.ralphy/init-state.json`
/// (ADR-0012 stage 9): which stages completed, the captured config (so a resume
/// skips the costly diagnosis + Q&A), and — crucially — the milestone and issue
/// numbers already published, so a re-run NEVER recreates them. `#[serde(default)]`
/// on every field keeps an older checkpoint loadable as the schema grows.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct InitState {
    #[serde(default)]
    pub completed: Vec<Stage>,
    #[serde(default)]
    pub config: Option<InitConfig>,
    #[serde(default)]
    pub milestone_created: Option<String>,
    #[serde(default)]
    pub created_issues: Vec<u64>,
}

impl InitState {
    /// Load the checkpoint from `<repo>/.ralphy/init-state.json`, or a fresh
    /// default when the file does not exist (a first run).
    pub fn load(ws: &Workspace) -> Result<Self> {
        let path = ws.init_state_path();
        if !path.exists() {
            return Ok(Self::default());
        }
        let raw = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        serde_json::from_str(&raw).with_context(|| format!("parsing {}", path.display()))
    }

    /// Persist the checkpoint under `.ralphy/`, mirroring [`persist_report`].
    pub fn save(&self, ws: &Workspace) -> Result<()> {
        std::fs::create_dir_all(ws.ralphy_dir()).context("creating .ralphy dir")?;
        let json = serde_json::to_string_pretty(self).context("serializing init state")?;
        std::fs::write(ws.init_state_path(), json).context("writing .ralphy/init-state.json")?;
        Ok(())
    }

    /// Whether `stage` has already completed.
    pub fn is_done(&self, s: Stage) -> bool {
        self.completed.contains(&s)
    }

    /// Record `stage` as completed (idempotent — never duplicated).
    pub fn mark(&mut self, s: Stage) {
        if !self.completed.contains(&s) {
            self.completed.push(s);
        }
    }
}

/// One seeded console question: a short label, a one-line explanation of what
/// the field means and how to answer it, and the diagnosis-derived default the
/// dev confirms (empty input) or overrides. `clearable` is true for optional
/// fields that `none` can blank — it tailors the per-question keep/clear hint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Question {
    pub label: String,
    pub help: String,
    pub default: String,
    pub clearable: bool,
}

/// The display form of an optional text field: the value, or the literal `none`
/// when absent — so a seeded default round-trips through [`resolve_text`].
fn display_opt(value: Option<&str>) -> String {
    value.unwrap_or("none").to_string()
}

/// The display form of a [`RepoKind`] — the same token [`resolve_kind`] parses.
fn display_kind(kind: RepoKind) -> String {
    match kind {
        RepoKind::Empty => "empty",
        RepoKind::Existing => "existing",
    }
    .to_string()
}

/// The display form of a bool field — the same token [`resolve_bool`] parses.
fn display_bool(value: bool) -> String {
    if value { "yes" } else { "no" }.to_string()
}

/// The display form of the milestone-docs list: comma-joined, or `none` when
/// empty.
fn display_list(items: &[String]) -> String {
    if items.is_empty() {
        "none".to_string()
    } else {
        items.join(", ")
    }
}

/// Resolve a free-text answer against a seeded default. Empty input keeps the
/// default; the literal `none` clears it to `None`; anything else is the trimmed
/// override.
fn resolve_text(default: Option<&str>, raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return default.map(str::to_string);
    }
    if trimmed.eq_ignore_ascii_case("none") {
        return None;
    }
    Some(trimmed.to_string())
}

/// Resolve a yes/no answer against a seeded default. Empty input keeps the
/// default; `y`/`yes`/`true` → `true`, `n`/`no`/`false` → `false`; an
/// unrecognized answer keeps the default.
fn resolve_bool(default: bool, raw: &str) -> bool {
    match raw.trim().to_ascii_lowercase().as_str() {
        "" => default,
        "y" | "yes" | "true" => true,
        "n" | "no" | "false" => false,
        _ => default,
    }
}

/// Resolve a repo-kind answer against a seeded default. Empty input keeps the
/// default; `empty`/`existing` set it; an unrecognized answer keeps the default.
fn resolve_kind(default: RepoKind, raw: &str) -> RepoKind {
    match raw.trim().to_ascii_lowercase().as_str() {
        "empty" => RepoKind::Empty,
        "existing" => RepoKind::Existing,
        _ => default,
    }
}

/// Resolve a comma-separated list answer against a seeded default. Empty input
/// keeps the default; the literal `none` clears it to an empty list; otherwise
/// the comma-split, trimmed, non-empty entries replace it.
fn resolve_list(default: &[String], raw: &str) -> Vec<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return default.to_vec();
    }
    if trimmed.eq_ignore_ascii_case("none") {
        return Vec::new();
    }
    trimmed
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

/// Build the seeded console questions from a diagnosis report — each default is
/// the report field's display form, so the dev confirms findings rather than
/// answering blind (ADR-0012 stage 3).
fn seed_questions(report: &DiagnosisReport) -> Vec<Question> {
    vec![
        Question {
            label: "Repository type".into(),
            help: "A new empty repo to set up, or an existing project to work in? \
                   (empty / existing)"
                .into(),
            default: display_kind(report.repo_kind),
            clearable: false,
        },
        Question {
            label: "Language & build".into(),
            help: "Main language and build/test command for this project. (e.g. cargo, npm)".into(),
            default: display_opt(report.language_build.as_deref()),
            clearable: true,
        },
        Question {
            label: "Backlog".into(),
            help: "Where your list of work to do lives, if any. (a link, a label, or a file path)"
                .into(),
            default: display_opt(report.backlog_location.as_deref()),
            clearable: true,
        },
        Question {
            label: "Planning docs".into(),
            help: "Roadmap or spec files to turn into tasks, if any. (file paths, comma-separated)"
                .into(),
            default: display_list(&report.milestone_docs),
            clearable: true,
        },
        Question {
            label: "Skills folder".into(),
            help: "Folder holding the agent's skill files, if any. (e.g. .claude/skills)".into(),
            default: display_opt(report.skills_dir.as_deref()),
            clearable: true,
        },
        Question {
            label: "Architecture docs".into(),
            help: "Do you already have notes about how the project is built? (yes / no)".into(),
            default: display_bool(report.has_context_or_adrs),
            clearable: false,
        },
        Question {
            label: "Code host".into(),
            help: "Where your code is hosted. (e.g. github.com)".into(),
            default: display_opt(report.remote_host.as_deref()),
            clearable: true,
        },
        Question {
            label: "Plan work from the docs above".into(),
            help: "Use the planning docs above to draft your first tasks? (yes / no)".into(),
            default: display_bool(!report.milestone_docs.is_empty()),
            clearable: false,
        },
    ]
}

/// Persist a validated diagnosis report under the workspace's `.ralphy/` so the
/// later init stages (and a re-run) can read it back (ADR-0012 stage 2).
fn persist_report(ws: &Workspace, report: &DiagnosisReport) -> Result<()> {
    std::fs::create_dir_all(ws.ralphy_dir()).context("creating .ralphy dir")?;
    let json = serde_json::to_string_pretty(report).context("serializing diagnosis report")?;
    std::fs::write(ws.diagnosis_path(), json).context("writing .ralphy/diagnosis.json")?;
    Ok(())
}

/// A human-readable echo of the captured config, for the dev to confirm. Every
/// resolved field appears so the confirmation is complete.
fn format_config_echo(cfg: &InitConfig) -> String {
    let mut out = String::new();
    out.push_str(&format!("repo kind:     {}\n", display_kind(cfg.repo_kind)));
    out.push_str(&format!(
        "language/build: {}\n",
        display_opt(cfg.language_build.as_deref())
    ));
    out.push_str(&format!(
        "backlog:        {}\n",
        display_opt(cfg.backlog_location.as_deref())
    ));
    out.push_str(&format!(
        "milestone docs: {}\n",
        display_list(&cfg.milestone_docs)
    ));
    out.push_str(&format!(
        "skills dir:     {}\n",
        display_opt(cfg.skills_dir.as_deref())
    ));
    out.push_str(&format!(
        "context/ADRs:   {}\n",
        display_bool(cfg.has_context_or_adrs)
    ));
    out.push_str(&format!(
        "remote host:    {}\n",
        display_opt(cfg.remote_host.as_deref())
    ));
    out.push_str(&format!(
        "PRD/roadmap:    {}\n",
        display_bool(cfg.adopt_prd_roadmap)
    ));
    out
}

/// Echo the captured config as a styled key/value list (dim labels, green values)
/// on a colour TTY, or the plain [`format_config_echo`] text otherwise. Labels
/// mirror the friendly Q&A wording so the summary reads as a recap of the answers.
fn print_captured_config(cfg: &InitConfig) {
    if !qa_color() {
        print!("{}", format_config_echo(cfg));
        return;
    }
    let row = |label: &str, value: String| {
        println!(
            "  {} {}",
            forced(Style::new().dim()).apply_to(format!("{label:<18}")),
            forced(Style::new().green()).apply_to(value)
        );
    };
    row("Repository type", display_kind(cfg.repo_kind));
    row(
        "Language & build",
        display_opt(cfg.language_build.as_deref()),
    );
    row("Backlog", display_opt(cfg.backlog_location.as_deref()));
    row("Planning docs", display_list(&cfg.milestone_docs));
    row("Skills folder", display_opt(cfg.skills_dir.as_deref()));
    row("Architecture docs", display_bool(cfg.has_context_or_adrs));
    row("Code host", display_opt(cfg.remote_host.as_deref()));
    row("Plan from docs", display_bool(cfg.adopt_prd_roadmap));
}

/// The neutral working directory for the diagnosis session: a fresh dir under the
/// system temp root, OUTSIDE the target repo, so the agent CLI cannot auto-load
/// the target's `CLAUDE.md`/`AGENTS.md` as system instructions (ADR-0012
/// "Considered options"). The `stamp` keeps concurrent runs from colliding.
fn diagnosis_cwd(repo: &Path, stamp: &str) -> PathBuf {
    neutral_cwd_from(&std::env::temp_dir(), repo, stamp)
}

/// Pure core of [`diagnosis_cwd`]: a dir under `base` named for `stamp`. The
/// whole point is that the cwd is OUTSIDE `repo`; if the temp `base` itself lives
/// inside the repo (a repo-local `TMPDIR`/`TEMP`), the candidate would land in
/// the target and both break the read-only invariant and let the CLI walk up into
/// the target's `CLAUDE.md`/`AGENTS.md`. In that case fall back to the repo's
/// parent so the cwd is guaranteed outside the target. Pure over its inputs so it
/// unit-tests the fallback the happy-path test can't reach.
fn neutral_cwd_from(base: &Path, repo: &Path, stamp: &str) -> PathBuf {
    let name = format!("ralphy-diagnose-{stamp}");
    let candidate = base.join(&name);
    if candidate.starts_with(repo) {
        if let Some(parent) = repo.parent() {
            return parent.join(name);
        }
    }
    candidate
}

/// Whether the console Q&A should emit ANSI styling: an attended stdout TTY
/// without `NO_COLOR`, mirroring the presenter's detection in `ui.rs` so init and
/// the run queue agree on when to colour.
fn qa_color() -> bool {
    console::Term::stdout().is_term() && std::env::var_os("NO_COLOR").is_none()
}

/// The content width the Q&A wraps to: the terminal's columns, clamped so help
/// stays readable on a narrow pane and doesn't sprawl across an ultra-wide one.
fn qa_width() -> usize {
    (console::Term::stdout().size().1 as usize).clamp(48, 92)
}

/// `force_styling` overrides console's own TTY probe: the caller's `color`
/// decision (from [`qa_color`]) is already authoritative, so honour it — this is
/// what keeps the styled path testable off a TTY.
fn forced(style: Style) -> Style {
    style.force_styling(true)
}

/// Greedy word-wrap `text` into lines no wider than `width`, breaking only on
/// whitespace (a word longer than `width` overflows its own line rather than
/// splitting mid-word — the bug the old single-line prompt showed as `em\npty`).
fn wrap_text(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut cur = String::new();
    for word in text.split_whitespace() {
        if cur.is_empty() {
            cur.push_str(word);
        } else if cur.chars().count() + 1 + word.chars().count() <= width {
            cur.push(' ');
            cur.push_str(word);
        } else {
            lines.push(std::mem::take(&mut cur));
            cur.push_str(word);
        }
    }
    if !cur.is_empty() {
        lines.push(cur);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

/// Render one question's prompt block as a small wizard step: a `[idx/total]`
/// counter and bold-cyan label, the explanation word-wrapped under it with a
/// hanging indent, then the seeded default and cursor arrow the answer is typed
/// after. Pure over `color`/`width` so both the styled and the plain (non-TTY /
/// `NO_COLOR`) forms are unit-testable.
fn render_question(q: &Question, idx: usize, total: usize, color: bool, width: usize) -> String {
    // 8 columns aligns the help and value lines under the label, past "  [n/n] ".
    const INDENT: &str = "        ";
    let help_width = width.saturating_sub(INDENT.len()).max(24);
    let help = wrap_text(&q.help, help_width).join(&format!("\n{INDENT}"));
    if color {
        let counter = forced(Style::new().dim()).apply_to(format!("[{idx}/{total}]"));
        let label = forced(Style::new().cyan().bold()).apply_to(&q.label);
        let help = forced(Style::new().dim()).apply_to(help);
        let default = forced(Style::new().green()).apply_to(&q.default);
        let arrow = forced(Style::new().cyan().bold()).apply_to("›");
        let opt = if q.clearable {
            forced(Style::new().dim())
                .apply_to(" · optional")
                .to_string()
        } else {
            String::new()
        };
        format!("\n  {counter} {label}\n{INDENT}{help}\n{INDENT}{default}{opt} {arrow} ")
    } else {
        let opt = if q.clearable { " (optional)" } else { "" };
        format!(
            "\n  [{idx}/{total}] {}\n{INDENT}{}\n{INDENT}{}{opt} > ",
            q.label, help, q.default
        )
    }
}

/// Print the environment-gate findings as a styled checklist (green ✓ / red ✗,
/// dim agent status) on a colour TTY, falling back to the plain [`format_report`]
/// text the unit tests and non-TTY consumers depend on.
fn print_gate_report(f: &EnvFindings, fails: &[HardFail]) {
    if !qa_color() {
        print!("{}", format_report(f, fails));
        return;
    }
    let mark = |good: bool| {
        if good {
            forced(Style::new().green().bold())
                .apply_to("✓")
                .to_string()
        } else {
            forced(Style::new().red().bold()).apply_to("✗").to_string()
        }
    };
    let dim = |s: &str| {
        forced(Style::new().dim())
            .apply_to(s.to_string())
            .to_string()
    };

    println!(
        "\n{}",
        forced(Style::new().cyan().bold()).apply_to("Environment")
    );
    println!("  {} python", mark(f.python));
    println!("  {} gh auth", mark(f.gh_authenticated));
    println!("  {} GitHub remote", mark(f.github_remote));
    println!("  {}", dim("agents"));
    for agent in &Agent::ALL {
        let name = agent.cli_name();
        let present = f.agents_present.contains(agent);
        let logged = present && f.agents_logged_in.contains(agent);
        let (glyph, status) = if logged {
            (
                forced(Style::new().green().bold())
                    .apply_to("✓")
                    .to_string(),
                "logged in",
            )
        } else if present {
            (dim("·"), "not logged in")
        } else {
            (dim("·"), "absent")
        };
        println!("    {glyph} {name:<9} {}", dim(status));
    }
    if fails.is_empty() {
        println!(
            "  {} {}",
            mark(true),
            forced(Style::new().green()).apply_to("all checks passed")
        );
    } else {
        println!("  {} {} blocker(s)", mark(false), fails.len());
    }
}

/// Print a secondary status line dimmed on a colour TTY (plain otherwise), so the
/// running commentary recedes behind the headers and prompts.
fn print_note(text: &str) {
    if qa_color() {
        println!("{}", forced(Style::new().dim()).apply_to(text));
    } else {
        println!("{text}");
    }
}

/// Print a success line: a green ✓ and the message, so finished steps read at a
/// glance the same way the gate's checklist does. Plain (`✓ text`) off a TTY.
fn print_ok(text: &str) {
    if qa_color() {
        println!(
            "  {} {text}",
            forced(Style::new().green().bold()).apply_to("✓")
        );
    } else {
        println!("  {text}");
    }
}

/// Print a list row under a section — a dim bullet and the item — for the file
/// lists and plans the stages emit (scaffold files, label actions, …).
fn print_bullet(text: &str) {
    if qa_color() {
        println!("  {} {text}", forced(Style::new().dim()).apply_to("·"));
    } else {
        println!("  - {text}");
    }
}

/// Ask a yes/no question with a styled, single-line prompt (a cyan `›`, the
/// question, a dim `[Y/n]`/`[y/N]` reflecting `default_yes`) and return the raw
/// answer for the stage's decision fn to resolve. Centralises every confirmation
/// so they all look alike instead of bare `print!("… [Y/n]: ")`.
fn ask_yes_no(question: &str, default_yes: bool) -> Result<String> {
    let hint = if default_yes { "[Y/n]" } else { "[y/N]" };
    if qa_color() {
        print!(
            "\n  {} {question} {} ",
            forced(Style::new().cyan().bold()).apply_to("›"),
            forced(Style::new().dim()).apply_to(hint)
        );
    } else {
        print!("\n  > {question} {hint} ");
    }
    std::io::stdout().flush().ok();
    let mut line = String::new();
    std::io::stdin()
        .read_line(&mut line)
        .context("reading answer from stdin")?;
    Ok(line)
}

/// Run `f` while a spinner animates next to `message` on a colour TTY, so the
/// multi-second agent calls (diagnosis, issue drafting) show life instead of a
/// frozen cursor. Off a TTY it just prints the message and runs `f` — no ANSI,
/// no animation. The spinner is cleared when `f` returns; the caller prints the
/// outcome line.
fn with_spinner<T>(message: &str, f: impl FnOnce() -> T) -> T {
    if !qa_color() {
        print_note(message);
        return f();
    }
    let pb = indicatif::ProgressBar::new_spinner();
    // `unwrap` is safe: the template is a compile-time constant that always parses.
    let style = indicatif::ProgressStyle::with_template("  {spinner:.cyan} {msg}")
        .unwrap()
        .tick_strings(&["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏", " "]);
    pb.set_style(style);
    pb.set_message(message.to_string());
    pb.enable_steady_tick(Duration::from_millis(90));
    let out = f();
    pb.finish_and_clear();
    out
}

/// Print a styled section header — a bold-cyan title and an optional dim subtitle
/// — so init's stages read like the run queue's branded output rather than bare
/// `println!`s.
fn print_section(title: &str, subtitle: Option<&str>) {
    if qa_color() {
        println!("\n{}", forced(Style::new().cyan().bold()).apply_to(title));
        if let Some(s) = subtitle {
            println!("{}", forced(Style::new().dim()).apply_to(s));
        }
    } else {
        println!("\n{title}");
        if let Some(s) = subtitle {
            println!("{s}");
        }
    }
}

/// Run the interactive, diagnosis-seeded Q&A on real stdin/stdout, resolving each
/// answer into an [`InitConfig`]. The pure resolvers do the work; this is the thin
/// impure shell (printing prompts, reading lines).
fn run_qa(report: &DiagnosisReport) -> Result<InitConfig> {
    let questions = seed_questions(report);
    let color = qa_color();
    let width = qa_width();
    let total = questions.len();
    let read_line = |i: usize| -> Result<String> {
        print!(
            "{}",
            render_question(&questions[i], i + 1, total, color, width)
        );
        std::io::stdout().flush().ok();
        let mut line = String::new();
        std::io::stdin()
            .read_line(&mut line)
            .context("reading answer from stdin")?;
        Ok(line)
    };

    // Indices match the order in `seed_questions`.
    let repo_kind = resolve_kind(report.repo_kind, &read_line(0)?);
    let language_build = resolve_text(report.language_build.as_deref(), &read_line(1)?);
    let backlog_location = resolve_text(report.backlog_location.as_deref(), &read_line(2)?);
    let milestone_docs = resolve_list(&report.milestone_docs, &read_line(3)?);
    let skills_dir = resolve_text(report.skills_dir.as_deref(), &read_line(4)?);
    let has_context_or_adrs = resolve_bool(report.has_context_or_adrs, &read_line(5)?);
    let remote_host = resolve_text(report.remote_host.as_deref(), &read_line(6)?);
    let adopt_prd_roadmap = resolve_bool(!report.milestone_docs.is_empty(), &read_line(7)?);

    Ok(InitConfig {
        repo_kind,
        language_build,
        backlog_location,
        milestone_docs,
        skills_dir,
        has_context_or_adrs,
        remote_host,
        adopt_prd_roadmap,
    })
}

// ── scaffold from setup-pocock templates (ADR-0012 stages 4–5) ──────────────

// The setup-pocock templates ship embedded in the binary so init has zero
// runtime dependency on the on-disk skills dir. Paths mirror the depth
// `ralphy-agent-claude/src/lib.rs` uses for `../../../assets/prompts/...`.
const TPL_ISSUE_GITHUB: &str =
    include_str!("../../../assets/plugin/skills/setup-pocock/issue-tracker-github.md");
const TPL_ISSUE_GITLAB: &str =
    include_str!("../../../assets/plugin/skills/setup-pocock/issue-tracker-gitlab.md");
const TPL_ISSUE_LOCAL: &str =
    include_str!("../../../assets/plugin/skills/setup-pocock/issue-tracker-local.md");
const TPL_TRIAGE: &str =
    include_str!("../../../assets/plugin/skills/setup-pocock/triage-labels.md");
const TPL_DOMAIN: &str = include_str!("../../../assets/plugin/skills/setup-pocock/domain.md");
const TPL_ROADMAP: &str = include_str!("../../../assets/plugin/skills/setup-pocock/roadmap.md");
const TPL_PRD_README: &str =
    include_str!("../../../assets/plugin/skills/setup-pocock/prd-readme.md");
const TPL_PRD_TEMPLATE: &str =
    include_str!("../../../assets/plugin/skills/setup-pocock/prd-template.md");

// SUSPENDED (see `write_scaffold`): the `## Agent skills` block write is parked
// under evaluation. These helpers are kept and unit-tested for re-enabling, so
// they read as dead in a non-test build — allow it rather than delete the work.
#[allow(dead_code)]
const AGENT_SKILLS_HEADING: &str = "## Agent skills";

/// Select the issue-tracker template by the remote host. A host containing
/// `gitlab` → the GitLab template, `github` → the GitHub template, anything else
/// (including `None`) → the local-markdown template. Returns the on-disk filename
/// to write (always `issue-tracker.md`) and the chosen template body. Pure over
/// its input.
fn select_issue_tracker(remote_host: Option<&str>) -> (&'static str, &'static str) {
    let host = remote_host.unwrap_or("").to_ascii_lowercase();
    let body = if host.contains("gitlab") {
        TPL_ISSUE_GITLAB
    } else if host.contains("github") {
        TPL_ISSUE_GITHUB
    } else {
        TPL_ISSUE_LOCAL
    };
    ("issue-tracker.md", body)
}

/// Render the `## Agent skills` block from the captured config: three one-line
/// summaries (issue tracker, triage labels, domain docs), each pointing at the
/// `docs/agents/*.md` file written alongside it.
#[allow(dead_code)] // SUSPENDED — see AGENT_SKILLS_HEADING.
fn agent_skills_block(cfg: &InitConfig) -> String {
    let tracker = match select_issue_tracker(cfg.remote_host.as_deref()).1 {
        b if b == TPL_ISSUE_GITHUB => "GitHub issues (use the `gh` CLI)",
        b if b == TPL_ISSUE_GITLAB => "GitLab issues (use the `glab` CLI)",
        _ => "local markdown files",
    };
    let mut out = String::new();
    out.push_str(AGENT_SKILLS_HEADING);
    out.push('\n');
    out.push_str(&format!(
        "\nThe engineering skills onboard from the docs below.\n\n\
         - Issue tracker: {tracker}. See `docs/agents/issue-tracker.md`.\n\
         - Triage labels: this repo's canonical triage roles. See `docs/agents/triage-labels.md`.\n\
         - Domain docs: single-context. See `docs/agents/domain.md`.\n"
    ));
    out
}

/// Replace an existing `## Agent skills` section in `doc` (from that heading up to
/// the next top-level `## ` heading or EOF) with `block`, or append `block` when
/// no such section exists. The result contains the heading exactly once. Pure.
#[allow(dead_code)] // SUSPENDED — see AGENT_SKILLS_HEADING.
fn upsert_agent_skills_block(doc: &str, block: &str) -> String {
    let block = block.trim_end();
    let Some(start) = find_heading(doc, AGENT_SKILLS_HEADING) else {
        // Append, separated by a blank line from any existing content.
        let mut out = doc.trim_end().to_string();
        if out.is_empty() {
            return format!("{block}\n");
        }
        out.push_str("\n\n");
        out.push_str(block);
        out.push('\n');
        return out;
    };

    // Find the end: the next h1/h2 heading after the section's body. Using any
    // top-level heading (not just `## `) as the boundary means a following `# `
    // or `## ` sibling section is preserved rather than silently clobbered; a
    // deeper `### ` nests under our section and is replaced with it.
    let after = &doc[start..];
    let body_offset = after.find('\n').map(|n| n + 1).unwrap_or(after.len());
    let end_rel = next_top_heading(&after[body_offset..]).map(|p| body_offset + p);

    let mut out = String::new();
    out.push_str(&doc[..start]);
    out.push_str(block);
    out.push('\n');
    if let Some(end_rel) = end_rel {
        out.push('\n');
        out.push_str(after[end_rel..].trim_start_matches('\n'));
    }
    out
}

/// Byte offset of the first line that starts with `needle` (at column 0), or
/// `None`. `needle` is matched as a line prefix so `## Agent skills` does not
/// match `### Agent skills sub`.
#[allow(dead_code)] // SUSPENDED — see AGENT_SKILLS_HEADING.
fn find_heading(doc: &str, needle: &str) -> Option<usize> {
    if doc.starts_with(needle) {
        return Some(0);
    }
    let pat = format!("\n{needle}");
    doc.find(&pat).map(|p| p + 1)
}

/// Byte offset of the first line that opens a top-level (h1 or h2) markdown
/// heading, or `None`. Used to bound an `## ` section: deeper `### ` headings
/// nest within it, so only `# ` / `## ` ends it.
#[allow(dead_code)] // SUSPENDED — see AGENT_SKILLS_HEADING.
fn next_top_heading(s: &str) -> Option<usize> {
    let mut offset = 0;
    for line in s.split_inclusive('\n') {
        let hashes = line.chars().take_while(|&c| c == '#').count();
        if (hashes == 1 || hashes == 2) && line[hashes..].starts_with(' ') {
            return Some(offset);
        }
        offset += line.len();
    }
    None
}

/// Write the deterministic scaffold onto the repo (ADR-0012 stage 5): the
/// `docs/agents/*` docs, the `## Agent skills` block in `CLAUDE.md`/`AGENTS.md`,
/// and — only when the dev opted in — the PRD/roadmap track docs. Idempotent:
/// every write overwrites in place and the block upsert never duplicates.
fn write_scaffold(repo: &Path, cfg: &InitConfig) -> Result<()> {
    let agents_dir = repo.join("docs").join("agents");
    std::fs::create_dir_all(&agents_dir).context("creating docs/agents")?;

    let (tracker_name, tracker_body) = select_issue_tracker(cfg.remote_host.as_deref());
    std::fs::write(agents_dir.join(tracker_name), tracker_body)
        .context("writing docs/agents/issue-tracker.md")?;
    std::fs::write(agents_dir.join("triage-labels.md"), TPL_TRIAGE)
        .context("writing docs/agents/triage-labels.md")?;
    std::fs::write(agents_dir.join("domain.md"), TPL_DOMAIN)
        .context("writing docs/agents/domain.md")?;

    // SUSPENDED (under evaluation): do not create or modify the repo's
    // CLAUDE.md/AGENTS.md. The `## Agent skills` block write is intentionally
    // disabled while we decide whether injecting it into the target repo is
    // necessary. The helpers (`agent_skills_block`/`upsert_agent_skills_block`)
    // are kept and still covered by unit tests; only the on-disk write here is
    // turned off. Re-enable by uncommenting the block below.
    //
    // // The block target: CLAUDE.md if present, else AGENTS.md if present, else a
    // // fresh CLAUDE.md.
    // let claude = repo.join("CLAUDE.md");
    // let agents = repo.join("AGENTS.md");
    // let target = if claude.exists() {
    //     claude
    // } else if agents.exists() {
    //     agents
    // } else {
    //     claude
    // };
    // let existing = std::fs::read_to_string(&target).unwrap_or_default();
    // let updated = upsert_agent_skills_block(&existing, &agent_skills_block(cfg));
    // std::fs::write(&target, updated).with_context(|| format!("writing {}", target.display()))?;

    if cfg.adopt_prd_roadmap {
        let prd_dir = repo.join("docs").join("prd");
        std::fs::create_dir_all(&prd_dir).context("creating docs/prd")?;
        std::fs::write(repo.join("docs").join("roadmap.md"), TPL_ROADMAP)
            .context("writing docs/roadmap.md")?;
        std::fs::write(prd_dir.join("README.md"), TPL_PRD_README)
            .context("writing docs/prd/README.md")?;
        std::fs::write(prd_dir.join("_template.md"), TPL_PRD_TEMPLATE)
            .context("writing docs/prd/_template.md")?;
    }

    Ok(())
}

// ── stage 6: download engineering skills (ADR-0012) ─────────────────────────

const RALPHY_REPO_URL: &str = "https://github.com/paulocorcino/ralphy.git";
const SKILLS_SUBTREE: &str = "assets/agents_template/skills";

/// Split the compile-time `RALPHY_SKILLS` env into skill names. The displayed list
/// reflects the BUILD-TIME tree; the downloaded set comes from the pinned tag and
/// may differ. A comment in the caller acknowledges this.
pub fn skill_names() -> Vec<&'static str> {
    env!("RALPHY_SKILLS")
        .split(',')
        .filter(|s| !s.is_empty())
        .collect()
}

/// Resolve the skills installation target from the configured agent skills dir.
/// - `None` → `.agents/skills`
/// - A value already ending in `skills` → used as-is (idempotent)
/// - `.codex` → `.agents/skills` (codex discovers there per ADR-0004)
/// - Anything else → `<dir>/skills`
pub fn skills_target(skills_dir: Option<&str>) -> PathBuf {
    match skills_dir {
        None => PathBuf::from(".agents/skills"),
        Some(d) if d == ".codex" || d.ends_with("/.codex") => PathBuf::from(".agents/skills"),
        Some(d) if d.ends_with("/skills") || d == "skills" => PathBuf::from(d),
        Some(d) if d.ends_with("skills") => PathBuf::from(d),
        Some(d) => PathBuf::from(d).join("skills"),
    }
}

/// Return `true` only when the dev explicitly authorizes the download with
/// `y` or `yes` (case-insensitive, trimmed). Any other answer — including silence
/// — declines. The prompt defaults to `[y/N]`, so silence is a network-safe no.
pub fn download_decision(answer: &str) -> bool {
    matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes")
}

/// The bootstrap decision when the target directory is not yet a git repository.
/// The prompt shows `[Y/n]`, so the recommended default (empty/`y`/`yes`) creates
/// the repo (`git init` + `gh repo create`); any other answer declines and init
/// keeps the original "not a git repository" error. Pure, mirrors [`labels_decision`].
pub fn create_repo_decision(answer: &str) -> bool {
    matches!(
        answer.trim().to_ascii_lowercase().as_str(),
        "" | "y" | "yes"
    )
}

/// Resolve the repo-visibility answer to whether the new GitHub repo is private.
/// The prompt shows `[Y/n]`, so the default (empty/`y`/`yes`) is private — the
/// safer default for a freshly created repo; an explicit `n`/`no` makes it public.
/// Pure.
pub fn private_visibility_decision(answer: &str) -> bool {
    !matches!(answer.trim().to_ascii_lowercase().as_str(), "n" | "no")
}

/// Derive the GitHub repo name from the (absolute) target directory: its final
/// path segment, falling back to `repo` when the path has no usable base name
/// (e.g. a drive/filesystem root). Pure over its input.
pub fn repo_name_from_path(path: &Path) -> String {
    path.file_name()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("repo")
        .to_string()
}

/// The label-creation decision: empty / `y` / `yes` → proceed (the default is
/// recommended since stage 7 is idempotent); `n` / anything else → skip.
pub fn labels_decision(answer: &str) -> bool {
    matches!(
        answer.trim().to_ascii_lowercase().as_str(),
        "" | "y" | "yes"
    )
}

/// Resolve the git ref the skills sparse-fetch should pin to. `RALPHY_VERSION` is
/// a `git describe` string (e.g. `v0.1.0-rc6-19-g27adb48`) for any build past a
/// tag, which `git fetch origin <ref>` cannot resolve — so prefer the exact commit
/// SHA (emitted by `build.rs` when built from a git checkout; GitHub resolves a
/// reachable SHA in a `want`). Fall back to `version` for a clean release build
/// (where `RALPHY_VERSION` is the bare tag) or a no-git source build. Pure over its
/// inputs so the fallback unit-tests without a build env.
fn resolve_fetch_ref(git_sha: Option<&str>, version: &str) -> String {
    match git_sha {
        Some(sha) if !sha.trim().is_empty() => sha.trim().to_string(),
        _ => version.to_string(),
    }
}

/// Build the exact git argv sequence for a sparse, pinned fetch of `subtree` from
/// the Ralphy repo at `version`. Pure: the impure shell feeds these to `git::git`.
/// Order: init → remote add → sparse-checkout init --cone →
///        sparse-checkout set <subtree> → fetch --depth 1 origin <version> →
///        checkout FETCH_HEAD.
pub fn sparse_fetch_commands(version: &str, subtree: &str) -> Vec<Vec<String>> {
    vec![
        vec!["init".into()],
        vec![
            "remote".into(),
            "add".into(),
            "origin".into(),
            RALPHY_REPO_URL.into(),
        ],
        vec!["sparse-checkout".into(), "init".into(), "--cone".into()],
        vec!["sparse-checkout".into(), "set".into(), subtree.into()],
        vec![
            "fetch".into(),
            "--depth".into(),
            "1".into(),
            "origin".into(),
            version.into(),
        ],
        vec!["checkout".into(), "FETCH_HEAD".into()],
    ]
}

/// Recursively copy `src` into `dst`, mirroring the directory structure.
fn copy_dir_all(src: &Path, dst: &Path) -> Result<()> {
    std::fs::create_dir_all(dst).with_context(|| format!("creating {}", dst.display()))?;
    for entry in std::fs::read_dir(src).with_context(|| format!("reading dir {}", src.display()))? {
        let entry = entry.with_context(|| format!("reading entry in {}", src.display()))?;
        let ty = entry
            .file_type()
            .with_context(|| format!("file type for {}", entry.path().display()))?;
        let dst_path = dst.join(entry.file_name());
        if ty.is_dir() {
            copy_dir_all(&entry.path(), &dst_path)?;
        } else {
            std::fs::copy(entry.path(), &dst_path).with_context(|| {
                format!(
                    "copying {} → {}",
                    entry.path().display(),
                    dst_path.display()
                )
            })?;
        }
    }
    Ok(())
}

/// Install skills from `src` into `dst` by replacing each managed skill subdir.
/// INVARIANT: only immediate subdirs of `src` are removed and replaced — unrelated
/// sibling dirs already in `dst` (the user's own skills) are never touched
/// (ADR-0004). Returns the count of installed skills.
pub fn install_skills_from(src: &Path, dst: &Path) -> Result<usize> {
    std::fs::create_dir_all(dst)
        .with_context(|| format!("creating skills dir {}", dst.display()))?;
    let mut count = 0usize;
    for entry in std::fs::read_dir(src)
        .with_context(|| format!("reading source skills {}", src.display()))?
    {
        let entry = entry.with_context(|| format!("reading entry in {}", src.display()))?;
        if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        let name = entry.file_name();
        let dst_skill = dst.join(&name);
        // Remove only this managed skill; sibling user dirs are untouched.
        let _ = std::fs::remove_dir_all(&dst_skill);
        copy_dir_all(&entry.path(), &dst_skill)?;
        count += 1;
    }
    Ok(count)
}

/// The result of the skills download step.
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    Installed(usize),
    #[allow(dead_code)]
    Skipped,
    Failed(String),
}

/// Run the skills install step.  `fetch` materialises a pinned subtree into a
/// scratch dir and returns the path to it.  EVERY failure — creating the scratch
/// dir, the fetch closure, or the subsequent copy — is absorbed and returned as
/// `Ok(Outcome::Failed(_))`.  This function NEVER propagates an error; the caller
/// (init's `run`) logs a warning and continues (warn-and-continue boundary).
pub fn install_skills_step(
    dst: &Path,
    fetch: impl FnOnce(&Path) -> Result<PathBuf>,
) -> Result<Outcome> {
    let scratch = std::env::temp_dir().join(format!("ralphy-skills-fetch-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&scratch);
    if let Err(e) = std::fs::create_dir_all(&scratch) {
        return Ok(Outcome::Failed(format!("creating scratch dir: {e}")));
    }
    let src = match fetch(&scratch) {
        Ok(p) => p,
        Err(e) => {
            let _ = std::fs::remove_dir_all(&scratch);
            return Ok(Outcome::Failed(e.to_string()));
        }
    };
    let outcome = match install_skills_from(&src, dst) {
        Ok(n) => Outcome::Installed(n),
        Err(e) => Outcome::Failed(e.to_string()),
    };
    let _ = std::fs::remove_dir_all(&scratch);
    Ok(outcome)
}

/// The git-safety decision for a (clean?, answer) pair. Pure: the impure shell in
/// [`run`] probes the tree and reads the answer, then acts on this verdict.
#[derive(Debug, Clone, PartialEq, Eq)]
enum CommitDecision {
    NothingToCommit,
    Commit,
    Abort(String),
}

/// Map (is_clean, answer) to a [`CommitDecision`]. A clean tree never commits; a
/// dirty tree commits on the recommended default (empty/yes/y — the prompt shows
/// `[Y/n]`, so accepting it commits the snapshot) and aborts only on an explicit
/// decline, which stops init before any branch or scaffold write.
fn commit_decision(is_clean: bool, answer: &str) -> CommitDecision {
    if is_clean {
        return CommitDecision::NothingToCommit;
    }
    match answer.trim().to_ascii_lowercase().as_str() {
        "" | "y" | "yes" => CommitDecision::Commit,
        _ => CommitDecision::Abort(
            "ralphy init aborted: a snapshot commit is required to isolate init's changes".into(),
        ),
    }
}

/// The branch decision for a (current, answer) pair. Pure.
#[derive(Debug, Clone, PartialEq, Eq)]
enum BranchDecision {
    Create(String),
    Stay,
}

/// Map an answer to a [`BranchDecision`]. Empty/yes/y (the recommended default) →
/// create `ralphy/init`; no/n → stay on the current branch.
fn branch_decision(_current: &str, answer: &str) -> BranchDecision {
    match answer.trim().to_ascii_lowercase().as_str() {
        "" | "y" | "yes" => BranchDecision::Create("ralphy/init".into()),
        _ => BranchDecision::Stay,
    }
}

// ── stage 8: backlog/milestone → issues (preview, confirm, publish) ──────────

/// Which judgment path stage 8 takes for the captured config — or `Skip` when the
/// diagnosis/Q&A found no backlog or milestone (ADR-0012 stage 8 "skipped cleanly"
/// criterion). Pure: the impure shell acts on this verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IssuesPath {
    Milestone,
    LooseBacklog,
    Skip,
}

/// Decide the stage-8 path from the captured config. The milestone path wins when
/// the dev adopted the PRD/roadmap model AND milestone docs exist; otherwise a
/// loose backlog is reshaped when one was found; otherwise stage 8 is skipped.
fn decide_issues_path(cfg: &InitConfig) -> IssuesPath {
    if cfg.adopt_prd_roadmap && !cfg.milestone_docs.is_empty() {
        IssuesPath::Milestone
    } else if cfg.backlog_location.is_some() {
        IssuesPath::LooseBacklog
    } else {
        IssuesPath::Skip
    }
}

/// The draft decision for the task-drafting step. The prompt shows `[Y/n]`, so the
/// recommended default (empty/yes/y) drafts a preview — nothing is published, this
/// is a read-only agent call — and only an explicit decline skips it. Pure, mirrors
/// [`labels_decision`].
fn draft_decision(answer: &str) -> bool {
    matches!(
        answer.trim().to_ascii_lowercase().as_str(),
        "" | "y" | "yes"
    )
}

/// The publish decision for the drafted preview. Default is **No** (`[y/N]`): a
/// bulk external write is never the silent default — only an explicit `y`/`yes`
/// proceeds. Pure, mirrors [`download_decision`].
fn publish_decision(answer: &str) -> bool {
    matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes")
}

/// A human-readable summary of the draft for the dev to confirm before any
/// external write: the headline count (and milestone, when present) followed by
/// one line per issue with its labels and blocked-by indices. Pure, mirrors
/// [`github::format_label_plan`].
fn format_draft_summary(draft: &IssuesDraft) -> String {
    let mut out = String::new();
    match &draft.milestone {
        Some(ms) => out.push_str(&format!(
            "will create {} issue(s), all in milestone \"{}\"\n",
            draft.issue_count(),
            ms.title
        )),
        None => out.push_str(&format!("will create {} issue(s)\n", draft.issue_count())),
    }
    if let Some(prd) = &draft.prd_path {
        out.push_str(&format!("PRD written: {prd}\n"));
    }
    for (i, issue) in draft.issues.iter().enumerate() {
        let labels = if issue.labels.is_empty() {
            String::new()
        } else {
            format!("  [{}]", issue.labels.join(", "))
        };
        let blocked = if issue.blocked_by.is_empty() {
            String::new()
        } else {
            let refs: Vec<String> = issue
                .blocked_by
                .iter()
                .map(|n| format!("#{}", n + 1))
                .collect();
            format!("  (blocked by {})", refs.join(", "))
        };
        out.push_str(&format!(
            "  {}. {}{}{}\n",
            i + 1,
            issue.title,
            labels,
            blocked
        ));
    }
    out
}

/// Rewrite a drafted body's `## Blocked by` placeholder with the resolved issue
/// numbers (or the "can start immediately" line when none). The charter emits the
/// literal `BLOCKED_BY_PLACEHOLDER`; if it is absent (a body that didn't follow
/// the template) the body is returned unchanged. Pure.
fn patch_blocked_by(body: &str, blocked_numbers: &[u64]) -> String {
    const PLACEHOLDER: &str = "BLOCKED_BY_PLACEHOLDER";
    if !body.contains(PLACEHOLDER) {
        return body.to_string();
    }
    let replacement = if blocked_numbers.is_empty() {
        "None - can start immediately".to_string()
    } else {
        blocked_numbers
            .iter()
            .map(|n| format!("- #{n}"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    body.replace(PLACEHOLDER, &replacement)
}

/// Resolve the agent-ready triage label from the scaffolded
/// `docs/agents/triage-labels.md` (the mapping for `ready-for-agent`), falling
/// back to the canonical `ready-for-agent` when no mapping is configured.
pub(crate) fn resolve_triage_label(repo: &Path) -> String {
    let triage_doc = std::fs::read_to_string(repo.join("docs/agents/triage-labels.md")).ok();
    triage_doc
        .as_deref()
        .and_then(|d| github::parse_triage_mapping(d, "ready-for-agent"))
        .unwrap_or_else(|| "ready-for-agent".to_string())
}

/// Resolve the human-return label an `escalate` verdict swaps in (ADR-0018 §3)
/// from the scaffolded `docs/agents/triage-labels.md` mapping for
/// `ready-for-human`, falling back to the canonical `ready-for-human` when no
/// mapping is configured. The `HITL` alias is honored downstream by the runner's
/// human-gate classification, so no alias handling is needed at swap time.
pub(crate) fn resolve_human_label(repo: &Path) -> String {
    let triage_doc = std::fs::read_to_string(repo.join("docs/agents/triage-labels.md")).ok();
    triage_doc
        .as_deref()
        .and_then(|d| github::parse_triage_mapping(d, "ready-for-human"))
        .unwrap_or_else(|| "ready-for-human".to_string())
}

/// Publish a confirmed draft, threading the [`InitState`] checkpoint so a crash
/// mid-publish never recreates an already-published issue or milestone on resume
/// (ADR-0012 stage 9). The closures inject the external writes (`create_milestone`,
/// `create_issue`) and the persist step (`save`), mirroring `install_skills_step`.
///
/// INVARIANT (held at every step): each issue/milestone is created AT MOST ONCE
/// across runs. The milestone is created only when `state.milestone_created` is
/// `None`; issues are created only beyond `state.created_issues.len()` (the
/// persisted prefix). `save` runs after EVERY create, so a crash leaves a prefix
/// the next run resumes PAST, never before — `created_issues[i]` is the published
/// number of `draft.issues[i]`, so a later issue's `blocked_by` index resolves
/// against that prefix.
fn publish_draft_with(
    draft: &IssuesDraft,
    state: &mut InitState,
    mut save: impl FnMut(&InitState) -> Result<()>,
    mut create_milestone: impl FnMut(&str, &str) -> Result<u64>,
    mut create_issue: impl FnMut(&str, &str, &[String], Option<&str>) -> Result<u64>,
) -> Result<()> {
    // Create the milestone first (so `gh issue create --milestone <name>` resolves)
    // — but only once across runs. Each issue links to it by name.
    if let Some(ms) = &draft.milestone {
        if state.milestone_created.is_none() {
            let number = create_milestone(&ms.title, &ms.description)?;
            println!("  created milestone #{number}: {}", ms.title);
            state.milestone_created = Some(ms.title.clone());
            save(state)?;
        }
    }
    let milestone_name = draft.milestone.as_ref().map(|ms| ms.title.as_str());

    // Resume past the persisted prefix: the first `created_issues.len()` draft
    // entries are already on GitHub.
    for issue in draft.issues.iter().skip(state.created_issues.len()) {
        // A blocker index must point at an earlier (already-created) issue; the
        // persisted prefix makes this resolve on resume. Guard an out-of-range
        // index rather than panicking on a bad draft.
        let blocked_numbers: Vec<u64> = issue
            .blocked_by
            .iter()
            .filter_map(|&idx| state.created_issues.get(idx).copied())
            .collect();
        let body = patch_blocked_by(&issue.body, &blocked_numbers);
        let number = create_issue(&issue.title, &body, &issue.labels, milestone_name)?;
        println!("  created #{number}: {}", issue.title);
        state.created_issues.push(number);
        save(state)?;
    }
    Ok(())
}

/// Reload a persisted [`IssuesDraft`] from `issues-draft.json` — the draft a
/// prior run's `created_issues` prefix corresponds to. Used on a partial-publish
/// resume so the remainder publishes against the SAME draft, never a regenerated
/// one (which could reorder the prefix and duplicate a published issue).
fn load_issues_draft(path: &Path) -> Result<IssuesDraft> {
    let raw =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    serde_json::from_str(&raw).with_context(|| format!("parsing {}", path.display()))
}

/// The impure wrapper: wire [`publish_draft_with`]'s closures to the real
/// `github::` POSTs and persist the checkpoint to `.ralphy/init-state.json` after
/// each create.
fn publish_draft(
    repo: &Path,
    draft: &IssuesDraft,
    state: &mut InitState,
    ws: &Workspace,
) -> Result<()> {
    publish_draft_with(
        draft,
        state,
        |s| s.save(ws),
        |title, description| github::create_milestone(repo, title, description),
        |title, body, labels, milestone| github::create_issue(repo, title, body, labels, milestone),
    )
}

/// Dispatch the read-only repo-diagnosis session to the selected agent's adapter.
/// Each adapter drives the same core charter ([`ralphy_core::build_diagnose_prompt`]);
/// only the CLI invocation differs.
fn diagnose_with_agent(
    agent: Agent,
    repo: &Path,
    neutral_cwd: &Path,
    model: Option<&str>,
    effort: Option<&str>,
    timeout: Duration,
) -> Result<DiagnosisReport> {
    match agent {
        Agent::Claude => {
            ralphy_agent_claude::diagnose_repo(repo, neutral_cwd, model, effort, timeout)
        }
        Agent::Codex => {
            ralphy_agent_codex::diagnose_repo(repo, neutral_cwd, model, effort, timeout)
        }
        Agent::Opencode => {
            ralphy_agent_opencode::diagnose_repo(repo, neutral_cwd, model, effort, timeout)
        }
    }
}

/// Dispatch the backlog/milestone → issues draft session to the selected agent's
/// adapter. Like [`diagnose_with_agent`], the charter is shared
/// ([`ralphy_core::build_init_issues_prompt`]) and only the invocation differs.
fn draft_with_agent(
    agent: Agent,
    repo: &Path,
    out_path: &Path,
    req: &DraftRequest,
    model: Option<&str>,
    effort: Option<&str>,
    timeout: Duration,
) -> Result<IssuesDraft> {
    match agent {
        Agent::Claude => {
            ralphy_agent_claude::draft_issues(repo, out_path, req, model, effort, timeout)
        }
        Agent::Codex => {
            ralphy_agent_codex::draft_issues(repo, out_path, req, model, effort, timeout)
        }
        Agent::Opencode => {
            ralphy_agent_opencode::draft_issues(repo, out_path, req, model, effort, timeout)
        }
    }
}

/// Choose which agent drives the AI judgment steps. An explicit `--agent` must be
/// logged in (else a hard error names the logged-in set); with no flag, the first
/// logged-in agent in gate order (claude → codex → opencode) is used. The gate has
/// already guaranteed `logged_in` is non-empty before this is called.
fn select_agent(requested: Option<Agent>, logged_in: &[Agent]) -> Result<Agent> {
    match requested {
        Some(a) if logged_in.contains(&a) => Ok(a),
        Some(a) => bail!(
            "ralphy init: --agent {} is not logged in (logged in: {})",
            a.cli_name(),
            if logged_in.is_empty() {
                "none".to_string()
            } else {
                logged_in
                    .iter()
                    .map(|x| x.cli_name())
                    .collect::<Vec<_>>()
                    .join(", ")
            }
        ),
        None => logged_in
            .first()
            .copied()
            .context("no logged-in agent available (the environment gate should have caught this)"),
    }
}

/// The model init pins for the AI judgment steps (diagnosis + issue drafting).
/// Claude gets `sonnet` (these steps don't warrant opus, and pinning keeps init
/// off the dev's personal `claude` default); other agents keep their CLI default
/// (`None`). Pure so the mapping unit-tests.
fn init_model_for(agent: Agent) -> Option<&'static str> {
    match agent {
        Agent::Claude => Some("sonnet"),
        Agent::Codex | Agent::Opencode => None,
    }
}

/// Resolve the target to its git toplevel, or — when it is not yet a git
/// repository — offer to bootstrap one before the environment gate (which assumes
/// a repo: it probes the `origin` remote). Creating a GitHub repo is only useful
/// if init can reach GitHub, so this first requires an authenticated `gh` (a hard
/// error names the fix otherwise); then, on the dev's confirmation, it runs
/// `git init` + an initial commit + `gh repo create` (visibility asked), wiring
/// `origin` so the gate's GitHub-remote check passes. A decline keeps the original
/// "not a git repository" error.
fn resolve_or_bootstrap_repo(target: &Path) -> Result<PathBuf> {
    if git::is_repo(target) {
        return git::resolve_toplevel(target);
    }

    print_section(
        "No git repository",
        Some("This directory isn't a git repository yet."),
    );

    // Creating a GitHub repo needs an authenticated `gh`; check it up front so the
    // dev fixes auth before we offer to create anything.
    if !gh_authenticated() {
        bail!(
            "ralphy init: this directory is not a git repository and `gh` is not authenticated, \
             so a repo can't be created — run `gh auth login`, then re-run `ralphy init` \
             (or `git init` and add a GitHub remote yourself)"
        );
    }

    let answer = ask_yes_no("Create a git repository and a GitHub repo here?", true)?;
    if !create_repo_decision(&answer) {
        bail!(
            "not a git repository: {} (pass --repo <repo>, or re-run and accept repo creation)",
            target.display()
        );
    }

    // Resolve to an absolute path so the repo name comes from the real directory (a
    // bare `.` has no file name) and the git/gh calls below have a stable cwd. The
    // dir may not exist yet — create it, then canonicalize.
    let abs = match std::fs::canonicalize(target) {
        Ok(p) => p,
        Err(_) => {
            std::fs::create_dir_all(target)
                .with_context(|| format!("creating {}", target.display()))?;
            std::fs::canonicalize(target)
                .with_context(|| format!("resolving {}", target.display()))?
        }
    };
    let name = repo_name_from_path(&abs);

    let private =
        private_visibility_decision(&ask_yes_no("Make the new GitHub repo private?", true)?);

    git::init(&abs)?;
    git::initial_commit(&abs)?;
    print_ok("Initialized git repository.");

    let visibility = if private { "private" } else { "public" };
    with_spinner("Creating the GitHub repository…", || {
        github::create_repo(&abs, &name, private)
    })?;
    print_ok(&format!("Created {visibility} GitHub repo {name}."));

    // Return git's own toplevel (a clean, forward-slash path) rather than the
    // canonicalized — possibly extended-length — `abs`.
    git::resolve_toplevel(&abs)
}

pub fn run(args: &InitArgs) -> Result<()> {
    let repo = resolve_or_bootstrap_repo(&args.repo)?;

    // Reuse the run command's branding banner so `init` opens with the same face:
    // the `🦊 Ralphy - vX` header + `📦 project · 🌿 branch · 🔗 url` info line. Seed
    // the face with the repo name so it's stable for this repo; the info-line
    // segments are best-effort (a detached HEAD or local-only repo drops a part).
    let repo_name = repo.file_name().and_then(|s| s.to_str()).unwrap_or("repo");
    let banner = crate::ui::Presenter::new().handle();
    banner.print_header(repo_name);
    let branch = git::current_branch(&repo).ok();
    let url = git::origin_url(&repo).map(|u| crate::ui::normalize_remote_url(&u));
    banner.print_info_line(repo_name, branch.as_deref(), url.as_deref());
    // Tear down the banner's live region now: init has no progress bars, and
    // leaving it active swallows the blank line the gate section prints next.
    banner.finalize();

    // Ignore `.ralphy/` before any snapshot commit, so the checkpoint
    // (`init-state.json`) and every other scratch artifact stay out of commits.
    gitignore::ensure_ralphy_ignored(&repo)?;

    // Run the (subprocess-backed: `gh auth status`, agent `whoami`/login probes)
    // environment checks behind a spinner so the multi-second wait shows life.
    let findings = with_spinner("Analyzing the environment…", || {
        let agents_present: Vec<Agent> = Agent::ALL.iter().copied().filter(agent_present).collect();
        let agents_logged_in: Vec<Agent> = agents_present
            .iter()
            .copied()
            .filter(agent_logged_in)
            .collect();
        EnvFindings {
            python: python_present(),
            gh_authenticated: gh_authenticated(),
            github_remote: github_remote(&repo),
            agents_present,
            agents_logged_in,
        }
    });

    let fails = evaluate_gate(&findings);
    print_gate_report(&findings, &fails);

    if !fails.is_empty() {
        bail!(
            "ralphy init: environment gate failed ({} blocker(s)) — see report above",
            fails.len()
        );
    }

    // Pick the agent that drives diagnosis + issue drafting (explicit --agent, or
    // the first logged-in agent the gate found). The gate above guarantees ≥1.
    let selected_agent = select_agent(args.agent, &findings.agents_logged_in)?;
    print_note(&format!(
        "Gate passed — using agent: {}.",
        selected_agent.cli_name()
    ));
    // The model for init's AI judgment steps (diagnosis + issue drafting). For
    // claude, pin sonnet: the read-only diagnosis and the issue drafting are
    // well-scoped tasks that don't warrant opus, and pinning here keeps init off
    // whatever the dev's `claude` default happens to be. Other agents keep their
    // CLI default (`None`).
    let init_model = init_model_for(selected_agent);
    // Load the checkpoint: a re-run resumes from it (ADR-0012 stage 9).
    let ws = Workspace::new(&repo);
    let mut state = InitState::load(&ws)?;

    // The captured config is the resume key for stages 2–3: present ⇒ the costly
    // agent diagnosis and the interactive Q&A already ran, so skip both.
    let cfg = if let Some(cfg) = state.config.clone() {
        print_note("Resuming: diagnosis + Q&A already captured — skipping.");
        cfg
    } else {
        // Reload a persisted report when one exists (a crash during the
        // interactive Q&A left `diagnosis.json` but no config), otherwise run the
        // diagnosis from a neutral cwd OUTSIDE the repo — so the target's
        // CLAUDE.md/AGENTS.md are read as data, never auto-loaded as instructions.
        let report = if ws.diagnosis_path().exists() {
            print_note(&format!(
                "Reusing persisted diagnosis at {}.",
                ws.diagnosis_path().display()
            ));
            let raw = std::fs::read_to_string(ws.diagnosis_path())
                .with_context(|| format!("reading {}", ws.diagnosis_path().display()))?;
            serde_json::from_str(&raw)
                .with_context(|| format!("parsing {}", ws.diagnosis_path().display()))?
        } else {
            let stamp = format!("{}", std::process::id());
            let cwd = diagnosis_cwd(&repo, &stamp);
            let report = with_spinner("Scanning your repo (read-only)…", || {
                diagnose_with_agent(
                    selected_agent,
                    &repo,
                    &cwd,
                    init_model,
                    Some("medium"),
                    Duration::from_secs(300),
                )
            })?;
            persist_report(&ws, &report)?;
            print_note(&format!(
                "Diagnosis written to {}",
                ws.diagnosis_path().display()
            ));
            report
        };

        // Console Q&A pre-filled by the diagnosis — the dev confirms/corrects.
        print_section(
            "Confirm the diagnosis",
            Some(
                "Ralphy pre-filled these 8 fields from its read-only scan. For each one: press \
                 Enter to keep the value shown, type a new value to change it, or 'none' to clear \
                 an optional field.",
            ),
        );
        let cfg = run_qa(&report)?;
        state.config = Some(cfg.clone());
        state.mark(Stage::Diagnose);
        state.save(&ws)?;
        cfg
    };

    print_section("Captured config", None);
    print_captured_config(&cfg);

    // ── stage 4: git safety (snapshot commit) ──────────────────────────────
    if !state.is_done(Stage::Git) {
        let is_clean = git::is_clean_ignoring_ralphy(&repo)?;
        if !is_clean {
            print_section(
                "Git safety",
                Some("You have uncommitted changes. Ralphy can save them in a commit first."),
            );
            for l in git::git(&repo, &["status", "--short"])?.lines() {
                print_bullet(l.trim_end());
            }
            let answer = ask_yes_no("Save your current changes in a commit first?", true)?;
            match commit_decision(is_clean, &answer) {
                CommitDecision::Abort(msg) => {
                    // INVARIANT: a refusal stops here — before any branch or write.
                    bail!("{msg}");
                }
                CommitDecision::Commit => {
                    git::commit_all_snapshot(&repo)?;
                    print_ok("Changes committed.");
                }
                CommitDecision::NothingToCommit => {}
            }
        }

        // ── stage 4b: branch (before any scaffold write) ────────────────────
        let current = git::current_branch(&repo)?;
        let answer = ask_yes_no("Do Ralphy's setup on a new branch `ralphy/init`?", true)?;
        match branch_decision(&current, &answer) {
            BranchDecision::Create(branch) => {
                if git::commitish_exists(&repo, &branch) {
                    git::checkout(&repo, &branch)?;
                } else {
                    git::checkout_new_branch(&repo, &branch, &current)?;
                }
                print_ok(&format!("Working on branch {branch}."));
            }
            BranchDecision::Stay => {
                print_ok(&format!("Staying on branch {current}."));
            }
        }
        state.mark(Stage::Git);
        state.save(&ws)?;
    } else {
        print_note("Resuming: git safety + branch already done — skipping.");
    }

    // ── stage 5: deterministic scaffold (onto the branch) ───────────────────
    if !state.is_done(Stage::Scaffold) {
        write_scaffold(&repo, &cfg)?;
        print_section(
            "Project files",
            Some("Created starter docs for the agent to use:"),
        );
        print_bullet("docs/agents/issue-tracker.md");
        print_bullet("docs/agents/triage-labels.md");
        print_bullet("docs/agents/domain.md");
        if cfg.adopt_prd_roadmap {
            print_bullet("docs/roadmap.md");
            print_bullet("docs/prd/README.md");
            print_bullet("docs/prd/_template.md");
        }
        state.mark(Stage::Scaffold);
        state.save(&ws)?;
    } else {
        print_note("Resuming: scaffold already written — skipping.");
    }

    // ── stage 6: download engineering skills ────────────────────────────────
    if state.is_done(Stage::Skills) {
        print_note("Resuming: skills step already done — skipping.");
    } else {
        let names = skill_names();
        let skills_dst = repo.join(skills_target(cfg.skills_dir.as_deref()));
        // NOTE: displayed list is from the build-time tree; downloaded set is from
        // the pinned commit (see resolve_fetch_ref) and may differ across builds.
        print_section(
            "Agent skills",
            Some(&format!(
                "Optional ready-made skills, installed into {}:",
                skills_dst.display()
            )),
        );
        for name in &names {
            print_bullet(name);
        }
        let answer = ask_yes_no("Install these skills?", false)?;
        if !download_decision(&answer) {
            print_note("Skipped — no skills installed.");
        } else {
            let version = env!("RALPHY_VERSION").to_string();
            let fetch_ref = resolve_fetch_ref(option_env!("RALPHY_GIT_SHA"), &version);
            let subtree = SKILLS_SUBTREE.to_string();
            let fetch = |scratch: &Path| -> Result<PathBuf> {
                for argv in sparse_fetch_commands(&fetch_ref, &subtree) {
                    let args: Vec<&str> = argv.iter().map(String::as_str).collect();
                    git::git(scratch, &args)?;
                }
                Ok(scratch.join(&subtree))
            };
            let outcome = with_spinner("Installing skills…", || {
                install_skills_step(&skills_dst, fetch)
            })?;
            match outcome {
                Outcome::Installed(n) => print_ok(&format!("Installed {n} skill(s).")),
                Outcome::Skipped => print_ok("Skills already up to date."),
                Outcome::Failed(msg) => {
                    print_note(&format!(
                        "warning: skills download failed ({msg}); continuing"
                    ));
                }
            }
        }
        state.mark(Stage::Skills);
        state.save(&ws)?;
    }

    // ── stage 7: create GitHub label vocabulary ──────────────────────────────
    if state.is_done(Stage::Labels) {
        print_note("Resuming: labels already done — skipping.");
    } else {
        let triage_doc = std::fs::read_to_string(repo.join("docs/agents/triage-labels.md")).ok();
        let desired = github::ralphy_label_specs(triage_doc.as_deref());
        let existing = github::list_repo_labels(&repo)?;
        let actions = github::plan_label_actions(&desired, &existing);
        print_section(
            "GitHub labels",
            Some("Labels Ralphy uses to track and triage issues:"),
        );
        for l in github::format_label_plan(&actions).lines() {
            print_bullet(l.trim());
        }
        let answer = ask_yes_no("Create/update these labels on GitHub?", true)?;
        if labels_decision(&answer) {
            with_spinner("Applying labels on GitHub…", || {
                github::apply_label_actions(&actions, &repo)
            })?;
            print_ok("Labels created/updated.");
        } else {
            print_note("Skipped — labels unchanged.");
        }
        state.mark(Stage::Labels);
        state.save(&ws)?;
    }

    // ── stage 8: backlog/milestone → issues (preview, confirm, publish) ───────
    if state.is_done(Stage::Issues) {
        if state.created_issues.is_empty() {
            print_note("Resuming: issue stage already done — nothing was published.");
        } else {
            let nums: Vec<String> = state
                .created_issues
                .iter()
                .map(|n| format!("#{n}"))
                .collect();
            print_note(&format!(
                "Resuming: issues already published ({}).",
                nums.join(", ")
            ));
        }
        return finalize(&repo, &cfg, &findings.agents_logged_in);
    }

    // Partial-publish resume: a prior run already created the milestone and/or
    // some issues but did not finish (a transient `gh` error mid-loop, or a
    // crash). The persisted `issues-draft.json` is the draft those numbers — and
    // the created milestone title — correspond to, so RELOAD it and publish only
    // the remainder. We must NOT re-draft here: a regenerated draft could reorder
    // the prefix (making `skip(created_issues.len())` recreate an already-
    // published issue) or carry a different milestone title than the one already
    // on GitHub. A clean re-draft happens only below, when nothing was published
    // yet (no milestone created and no issues created).
    if !state.created_issues.is_empty() || state.milestone_created.is_some() {
        let draft_path = ws.issues_draft_path();
        if !draft_path.exists() {
            bail!(
                "ralphy init: resume recorded {} published issue(s) but the draft at {} is gone, \
                 so the remaining issues can't be published safely — delete \
                 .ralphy/init-state.json to restart issue creation from scratch",
                state.created_issues.len(),
                draft_path.display()
            );
        }
        let draft = load_issues_draft(&draft_path)?;
        // Guard against a tampered/truncated draft: it must hold at least the
        // already-published prefix, else `skip` would silently drop the remainder
        // and we'd mark the stage done having published nothing more.
        if draft.issues.len() < state.created_issues.len() {
            bail!(
                "ralphy init: the draft at {} has {} issue(s) but {} were already published — \
                 it no longer matches the checkpoint; delete .ralphy/init-state.json to restart \
                 issue creation from scratch",
                draft_path.display(),
                draft.issues.len(),
                state.created_issues.len()
            );
        }
        print_note(&format!(
            "Resuming publish: {} issue(s) already created; publishing the rest from {}…",
            state.created_issues.len(),
            draft_path.display()
        ));
        with_spinner("Publishing remaining issues…", || {
            publish_draft(&repo, &draft, &mut state, &ws)
        })?;
        print_ok(&format!(
            "Published {} issue(s) total.",
            state.created_issues.len()
        ));
        state.mark(Stage::Issues);
        state.save(&ws)?;
        return finalize(&repo, &cfg, &findings.agents_logged_in);
    }

    match decide_issues_path(&cfg) {
        IssuesPath::Skip => {
            print_section(
                "First tasks",
                Some("No backlog or planning docs found — skipping task creation."),
            );
            // Nothing to publish — the stage completed; record it so a re-run
            // doesn't reconsider an empty backlog.
            state.mark(Stage::Issues);
            state.save(&ws)?;
        }
        path => {
            let (mode, source_docs) = match path {
                IssuesPath::Milestone => (IssuesMode::Milestone, cfg.milestone_docs.clone()),
                IssuesPath::LooseBacklog => (
                    IssuesMode::LooseBacklog,
                    cfg.backlog_location.iter().cloned().collect(),
                ),
                IssuesPath::Skip => unreachable!("Skip handled above"),
            };
            print_section(
                "First tasks",
                Some("Ralphy can read your docs to draft a first set of tasks (nothing is published yet)."),
            );
            let answer = ask_yes_no("Draft a first set of tasks from your docs?", true)?;
            if !draft_decision(&answer) {
                // Declined — don't run the agent. Leave Stage::Issues unmarked so a
                // re-run offers drafting again (mirrors a declined publish).
                print_note("Skipped — no tasks drafted.");
                return finalize(&repo, &cfg, &findings.agents_logged_in);
            }
            let triage_label = resolve_triage_label(&repo);
            let draft_path = ws.issues_draft_path();
            let req = DraftRequest {
                mode,
                source_docs: &source_docs,
                triage_label: &triage_label,
            };
            let draft = with_spinner("Drafting tasks…", || {
                draft_with_agent(
                    selected_agent,
                    &repo,
                    &draft_path,
                    &req,
                    init_model,
                    Some("medium"),
                    Duration::from_secs(600),
                )
            })?;
            print_note(&format!("Draft written to {}", draft_path.display()));

            println!();
            for l in format_draft_summary(&draft).lines() {
                print_bullet(l);
            }
            let answer = ask_yes_no("Publish these tasks as issues on GitHub?", false)?;
            if publish_decision(&answer) {
                with_spinner("Publishing issues…", || {
                    publish_draft(&repo, &draft, &mut state, &ws)
                })?;
                print_ok(&format!(
                    "Published {} issue(s).",
                    state.created_issues.len()
                ));
                state.mark(Stage::Issues);
                state.save(&ws)?;
            } else {
                // A declined publish leaves the draft on disk; do NOT mark Issues
                // done, so a re-run still offers to publish it (per Decisions).
                print_note(&format!(
                    "Skipped — draft kept at {}.",
                    draft_path.display()
                ));
            }
        }
    }

    finalize(&repo, &cfg, &findings.agents_logged_in)
}

// ── stage 10: static verification + final report + optional smoke test ────────

/// A gathered snapshot of what `ralphy init` produced: which artifacts exist,
/// how many labels and skills were installed, what issues are queued, and who is
/// logged in.
pub struct VerifyReport {
    pub ralphy_present: bool,
    pub docs: Vec<(&'static str, bool)>,
    pub ralphy_label_count: usize,
    pub skill_count: usize,
    pub queue: Vec<u64>,
    pub branch: String,
    pub logged_in: Vec<String>,
}

/// The lowest-numbered queue issue is the suggested next run target (the queue
/// from `github::build_queue` is ascending by number). Returns `None` when the
/// queue is empty.
pub fn suggested_issue(queue: &[u64]) -> Option<u64> {
    queue.first().copied()
}

/// The exact `ralphy run` command the dev should run next for issue `n`.
fn next_step_command(n: u64) -> String {
    format!("ralphy run --only-issue {n} --dry-run")
}

/// Returns `true` only for an explicit `y`/`yes` (case-insensitive, trimmed).
/// Any other answer — including silence — declines, making the smoke test
/// opt-in rather than automatic.
pub fn smoke_test_decision(answer: &str) -> bool {
    matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes")
}

/// Returns the relative paths of required artifacts that are missing from the
/// report. The order is stable: `.ralphy/` first, then the doc files in order.
pub fn required_artifacts_missing(r: &VerifyReport) -> Vec<String> {
    let mut missing = Vec::new();
    if !r.ralphy_present {
        missing.push(".ralphy/".to_string());
    }
    for (path, present) in &r.docs {
        if !present {
            missing.push(path.to_string());
        }
    }
    missing
}

/// Render a human-readable final report from a gathered [`VerifyReport`]. When
/// the queue is empty the literal `warning: no queue-labeled issue` is emitted
/// and no next-step line is included; otherwise the `next step:` line names the
/// lowest queue number.
pub fn format_final_report(r: &VerifyReport) -> String {
    let mut out = String::new();
    out.push_str(&format!("agents logged in: {}\n", r.logged_in.join(", ")));
    out.push_str(&format!("branch: {}\n", r.branch));

    let ralphy_status = if r.ralphy_present {
        "present"
    } else {
        "MISSING"
    };
    out.push_str(&format!(".ralphy/:              {ralphy_status}\n"));
    for (path, present) in &r.docs {
        let status = if *present { "present" } else { "MISSING" };
        out.push_str(&format!("{path}:  {status}\n"));
    }

    out.push_str(&format!(
        "labels: {}  skills: {}  queue issues: {}\n",
        r.ralphy_label_count,
        r.skill_count,
        r.queue.len()
    ));

    if r.queue.is_empty() {
        out.push_str("warning: no queue-labeled issue\n");
    } else {
        let n = suggested_issue(&r.queue).unwrap();
        out.push_str(&format!("next step: {}\n", next_step_command(n)));
    }
    out
}

/// Print the final verify report as a styled checklist (green ✓ / red ✗ for the
/// required artifacts, dim summary lines, a highlighted next step) on a colour
/// TTY, or the plain [`format_final_report`] text otherwise.
fn print_final_report(r: &VerifyReport) {
    if !qa_color() {
        print!("{}", format_final_report(r));
        return;
    }
    let mark = |ok: bool| {
        if ok {
            forced(Style::new().green().bold())
                .apply_to("✓")
                .to_string()
        } else {
            forced(Style::new().red().bold()).apply_to("✗").to_string()
        }
    };
    print_section("Setup complete", None);
    println!("  {} .ralphy/", mark(r.ralphy_present));
    for (path, present) in &r.docs {
        println!("  {} {path}", mark(*present));
    }
    print_note(&format!("branch: {}", r.branch));
    print_note(&format!("agents: {}", r.logged_in.join(", ")));
    print_note(&format!(
        "labels: {} · skills: {} · queued issues: {}",
        r.ralphy_label_count,
        r.skill_count,
        r.queue.len()
    ));
    if r.queue.is_empty() {
        print_note("note: no issue is queued for the agent yet");
    } else {
        let n = suggested_issue(&r.queue).unwrap();
        print_ok(&format!("Next: {}", next_step_command(n)));
    }
}

/// Spawn the current binary as `ralphy run --repo <repo> --only-issue <n>
/// --dry-run`, inheriting stdio. A non-zero exit is surfaced as a warning line
/// but does NOT fail `finalize` — the smoke test is diagnostic.
fn run_smoke_test(repo: &Path, n: u64) -> Result<()> {
    let exe = std::env::current_exe().context("resolving current exe for smoke test")?;
    let repo_str = repo.display().to_string();
    let status = std::process::Command::new(&exe)
        .args([
            "run",
            "--repo",
            &repo_str,
            "--only-issue",
            &n.to_string(),
            "--dry-run",
        ])
        .status()
        .with_context(|| format!("spawning smoke test: {}", exe.display()))?;
    if !status.success() {
        println!("warning: smoke test exited with status {status} — inspect the output above");
    }
    Ok(())
}

/// Stage 10: gather a [`VerifyReport`], print the final report, bail when any
/// required artifact is missing, and — when the queue is non-empty — offer an
/// optional `--dry-run` smoke test. Called from every completion point in
/// [`run`] so the report always appears regardless of which path the dev took.
fn finalize(repo: &Path, cfg: &InitConfig, logged_in: &[Agent]) -> Result<()> {
    let triage_doc = std::fs::read_to_string(repo.join("docs/agents/triage-labels.md")).ok();

    let ralphy_present = repo.join(".ralphy").is_dir();
    let docs: Vec<(&'static str, bool)> = vec![
        (
            "docs/agents/issue-tracker.md",
            repo.join("docs/agents/issue-tracker.md").exists(),
        ),
        (
            "docs/agents/triage-labels.md",
            repo.join("docs/agents/triage-labels.md").exists(),
        ),
        (
            "docs/agents/domain.md",
            repo.join("docs/agents/domain.md").exists(),
        ),
    ];

    let desired_labels = github::ralphy_label_specs(triage_doc.as_deref());
    let existing_labels = github::list_repo_labels(repo)?;
    let existing_names: std::collections::HashSet<&str> =
        existing_labels.iter().map(|(n, _)| n.as_str()).collect();
    let ralphy_label_count = desired_labels
        .iter()
        .filter(|s| existing_names.contains(s.name.as_str()))
        .count();

    let skills_path = repo.join(skills_target(cfg.skills_dir.as_deref()));
    let skill_count = std::fs::read_dir(&skills_path)
        .ok()
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
                .count()
        })
        .unwrap_or(0);

    let queue_labels = github::resolve_queue_labels(&[], repo);
    // Whole-repo housekeeping — never assignee-scoped (ADR-0021).
    let queue_issues = github::list_queue(&queue_labels, None, repo)?;
    let mut queue: Vec<u64> = queue_issues.iter().map(|i| i.number).collect();
    queue.sort_unstable();

    let branch = git::current_branch(repo)?;
    let logged_in_names: Vec<String> = logged_in.iter().map(|a| a.cli_name().to_string()).collect();

    let r = VerifyReport {
        ralphy_present,
        docs,
        ralphy_label_count,
        skill_count,
        queue,
        branch,
        logged_in: logged_in_names,
    };

    print_final_report(&r);

    let missing = required_artifacts_missing(&r);
    if !missing.is_empty() {
        bail!(
            "ralphy init: repo is not ready — missing {}",
            missing.join(", ")
        );
    }

    if !r.queue.is_empty() {
        let n = suggested_issue(&r.queue).unwrap();
        let answer = ask_yes_no("Try a safe practice run now (no changes made)?", false)?;
        if smoke_test_decision(&answer) {
            run_smoke_test(repo, n)?;
        } else {
            print_note(&format!(
                "Skipped. Run it yourself anytime: {}",
                next_step_command(n)
            ));
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all_green() -> EnvFindings {
        EnvFindings {
            python: true,
            gh_authenticated: true,
            github_remote: true,
            agents_present: vec![Agent::Claude, Agent::Codex],
            agents_logged_in: vec![Agent::Claude],
        }
    }

    // (a) All-green: evaluate_gate returns empty vec when ≥1 agent is logged in.
    #[test]
    fn evaluate_gate_all_green_returns_empty() {
        assert!(evaluate_gate(&all_green()).is_empty());
    }

    // ── agent selection (ADR-0012: --agent dispatch) ────────────────────────

    #[test]
    fn select_agent_defaults_to_first_logged_in() {
        let logged_in = vec![Agent::Codex, Agent::Opencode];
        assert_eq!(select_agent(None, &logged_in).unwrap(), Agent::Codex);
    }

    #[test]
    fn init_model_pins_sonnet_for_claude_only() {
        assert_eq!(init_model_for(Agent::Claude), Some("sonnet"));
        assert_eq!(init_model_for(Agent::Codex), None);
        assert_eq!(init_model_for(Agent::Opencode), None);
    }

    #[test]
    fn select_agent_honours_explicit_logged_in_choice() {
        let logged_in = vec![Agent::Claude, Agent::Codex];
        assert_eq!(
            select_agent(Some(Agent::Codex), &logged_in).unwrap(),
            Agent::Codex
        );
    }

    #[test]
    fn select_agent_rejects_explicit_not_logged_in() {
        // A present-but-not-logged-in (or absent) agent is a hard error naming the
        // logged-in set, never a silent fallback to another agent.
        let logged_in = vec![Agent::Claude];
        let err = select_agent(Some(Agent::Opencode), &logged_in).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("opencode"), "names the rejected agent:\n{msg}");
        assert!(msg.contains("claude"), "names the logged-in set:\n{msg}");
    }

    // ── pre-gate bootstrap (not-a-git-repo) decisions ───────────────────────

    #[test]
    fn create_repo_decision_defaults_to_yes() {
        // Empty (Enter on a [Y/n] prompt) and explicit yes proceed; anything else
        // declines, keeping the original "not a git repository" error.
        assert!(create_repo_decision(""));
        assert!(create_repo_decision("y"));
        assert!(create_repo_decision("YES"));
        assert!(!create_repo_decision("n"));
        assert!(!create_repo_decision("no"));
        assert!(!create_repo_decision("huh"));
    }

    #[test]
    fn private_visibility_defaults_to_private() {
        // The default and yes mean private; only an explicit no makes it public.
        assert!(private_visibility_decision(""));
        assert!(private_visibility_decision("y"));
        assert!(private_visibility_decision("anything"));
        assert!(!private_visibility_decision("n"));
        assert!(!private_visibility_decision("NO"));
    }

    #[test]
    fn repo_name_from_path_uses_final_segment() {
        assert_eq!(
            repo_name_from_path(Path::new("/home/dev/subtitle-downloader")),
            "subtitle-downloader"
        );
        // A root with no usable base name falls back to `repo`.
        assert_eq!(repo_name_from_path(Path::new("/")), "repo");
    }

    // (b) Missing python.
    #[test]
    fn evaluate_gate_missing_python() {
        let f = EnvFindings {
            python: false,
            ..all_green()
        };
        let fails = evaluate_gate(&f);
        assert!(fails.contains(&HardFail::MissingPython));
    }

    // (c) gh not authenticated.
    #[test]
    fn evaluate_gate_gh_not_authenticated() {
        let f = EnvFindings {
            gh_authenticated: false,
            ..all_green()
        };
        let fails = evaluate_gate(&f);
        assert!(fails.contains(&HardFail::GhNotAuthenticated));
    }

    // (d) No github remote.
    #[test]
    fn evaluate_gate_no_github_remote() {
        let f = EnvFindings {
            github_remote: false,
            ..all_green()
        };
        let fails = evaluate_gate(&f);
        assert!(fails.contains(&HardFail::NoGithubRemote));
    }

    // (e) No agent CLI present.
    #[test]
    fn evaluate_gate_no_agent_cli() {
        let f = EnvFindings {
            agents_present: vec![],
            agents_logged_in: vec![],
            ..all_green()
        };
        let fails = evaluate_gate(&f);
        assert!(fails.contains(&HardFail::NoAgentCli));
    }

    // (f) Two agents present, none logged in → NoAgentLoggedIn.
    #[test]
    fn evaluate_gate_agents_present_none_logged_in() {
        let f = EnvFindings {
            agents_present: vec![Agent::Claude, Agent::Codex],
            agents_logged_in: vec![],
            ..all_green()
        };
        let fails = evaluate_gate(&f);
        assert!(fails.contains(&HardFail::NoAgentLoggedIn));
        assert!(!fails.contains(&HardFail::NoAgentCli));
    }

    // (g) Two present, one logged in → empty vec (≥1 passes rule).
    #[test]
    fn evaluate_gate_one_of_two_logged_in_passes() {
        let f = EnvFindings {
            agents_present: vec![Agent::Claude, Agent::Codex],
            agents_logged_in: vec![Agent::Codex],
            ..all_green()
        };
        assert!(evaluate_gate(&f).is_empty());
    }

    // (h) format_report literal substring assertions.
    #[test]
    fn format_report_logged_in_and_not_logged_in_substrings() {
        let f = EnvFindings {
            python: true,
            gh_authenticated: true,
            github_remote: true,
            agents_present: vec![Agent::Claude, Agent::Codex],
            agents_logged_in: vec![Agent::Claude],
        };
        let fails = evaluate_gate(&f);
        let report = format_report(&f, &fails);
        assert!(
            report.contains("claude: logged in"),
            "expected 'claude: logged in' in:\n{report}"
        );
        assert!(
            report.contains("codex: not logged in"),
            "expected 'codex: not logged in' in:\n{report}"
        );
    }

    // ── diagnosis → Q&A → config (ADR-0012 stages 2–3) ──────────────────────

    fn sample_report() -> DiagnosisReport {
        DiagnosisReport {
            repo_kind: RepoKind::Existing,
            language_build: Some("Rust / cargo".into()),
            backlog_location: Some("docs/backlog.md".into()),
            milestone_docs: vec!["docs/roadmap.md".into(), "docs/prd/0001.md".into()],
            skills_dir: Some(".claude".into()),
            has_context_or_adrs: true,
            remote_host: Some("github.com".into()),
        }
    }

    fn config_of(report: &DiagnosisReport) -> InitConfig {
        InitConfig {
            repo_kind: report.repo_kind,
            language_build: report.language_build.clone(),
            backlog_location: report.backlog_location.clone(),
            milestone_docs: report.milestone_docs.clone(),
            skills_dir: report.skills_dir.clone(),
            has_context_or_adrs: report.has_context_or_adrs,
            remote_host: report.remote_host.clone(),
            adopt_prd_roadmap: !report.milestone_docs.is_empty(),
        }
    }

    #[test]
    fn resolve_text_empty_keeps_default_typed_overrides_none_clears() {
        // Empty input keeps the seeded default.
        assert_eq!(resolve_text(Some("Rust"), "  "), Some("Rust".to_string()));
        // A typed value overrides it.
        assert_eq!(resolve_text(Some("Rust"), "Go"), Some("Go".to_string()));
        // The literal `none` clears it.
        assert_eq!(resolve_text(Some("Rust"), "none"), None);
    }

    #[test]
    fn resolve_bool_empty_keeps_default_typed_overrides() {
        assert!(resolve_bool(true, ""));
        assert!(!resolve_bool(true, "no"));
        assert!(resolve_bool(false, "yes"));
        // Unrecognized → keep the default.
        assert!(!resolve_bool(false, "maybe"));
    }

    #[test]
    fn resolve_kind_empty_keeps_default_typed_overrides() {
        assert_eq!(resolve_kind(RepoKind::Existing, ""), RepoKind::Existing);
        assert_eq!(resolve_kind(RepoKind::Existing, "empty"), RepoKind::Empty);
        assert_eq!(
            resolve_kind(RepoKind::Empty, "existing"),
            RepoKind::Existing
        );
    }

    #[test]
    fn seed_questions_defaults_match_report_fields() {
        let report = sample_report();
        let qs = seed_questions(&report);
        assert_eq!(qs[0].default, display_kind(report.repo_kind));
        assert_eq!(qs[1].default, report.language_build.clone().unwrap());
        assert_eq!(qs[2].default, report.backlog_location.clone().unwrap());
        assert_eq!(qs[3].default, report.milestone_docs.join(", "));
        assert_eq!(qs[4].default, report.skills_dir.clone().unwrap());
        assert_eq!(qs[5].default, display_bool(report.has_context_or_adrs));
        assert_eq!(qs[6].default, report.remote_host.clone().unwrap());
        assert_eq!(
            qs[7].default,
            display_bool(!report.milestone_docs.is_empty())
        );
    }

    #[test]
    fn render_question_plain_shows_counter_label_help_and_default() {
        let q = Question {
            label: "Repo kind".into(),
            help: "Empty repo to scaffold, or existing codebase to adopt?".into(),
            default: "existing".into(),
            clearable: false,
        };
        let out = render_question(&q, 1, 8, false, 80);
        // No ANSI escapes on the plain path, and every part is present.
        assert!(!out.contains('\u{1b}'));
        assert!(out.contains("[1/8]"));
        assert!(out.contains("Repo kind"));
        assert!(out.contains("existing codebase to adopt"));
        assert!(out.contains("existing"));
        // A non-clearable field is not marked optional.
        assert!(!out.contains("optional"));
    }

    #[test]
    fn render_question_clearable_marks_optional_and_color_emits_ansi() {
        let q = Question {
            label: "Backlog location".into(),
            help: "Where the backlog lives.".into(),
            default: "none".into(),
            clearable: true,
        };
        assert!(render_question(&q, 3, 8, false, 80).contains("optional"));
        // The styled path wraps content in ANSI escapes.
        assert!(render_question(&q, 3, 8, true, 80).contains('\u{1b}'));
    }

    #[test]
    fn wrap_text_breaks_on_whitespace_within_width() {
        let lines = wrap_text("the quick brown fox jumps", 9);
        assert!(lines.iter().all(|l| l.chars().count() <= 9), "{lines:?}");
        // Joining with spaces round-trips the words in order.
        assert_eq!(lines.join(" "), "the quick brown fox jumps");
    }

    #[test]
    fn wrap_text_overflows_a_word_longer_than_width() {
        // A single word wider than `width` lands on its own line, never split.
        let lines = wrap_text("short superlongunbreakableword end", 8);
        assert!(
            lines.contains(&"superlongunbreakableword".to_string()),
            "{lines:?}"
        );
    }

    #[test]
    fn persist_report_round_trips_through_ralphy_dir() {
        // Mirror gitignore.rs/queue.rs: no tempfile dep, manual temp dir.
        let dir = std::env::temp_dir().join(format!("ralphy-init-persist-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let ws = Workspace::new(&dir);
        let report = sample_report();

        persist_report(&ws, &report).unwrap();
        let raw = std::fs::read_to_string(ws.diagnosis_path()).unwrap();
        let back: DiagnosisReport = serde_json::from_str(&raw).unwrap();
        assert_eq!(back, report);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn init_state_round_trips_through_ralphy_dir() {
        // Mirror persist_report_round_trips: no tempfile dep, manual temp dir.
        let dir = std::env::temp_dir().join(format!("ralphy-init-state-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let ws = Workspace::new(&dir);

        let state = InitState {
            completed: vec![Stage::Diagnose, Stage::Git],
            config: Some(config_of(&sample_report())),
            milestone_created: Some("M1".into()),
            created_issues: vec![101, 102],
        };

        state.save(&ws).unwrap();
        let back = InitState::load(&ws).unwrap();
        assert_eq!(back, state);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn init_state_load_defaults_when_absent() {
        let dir =
            std::env::temp_dir().join(format!("ralphy-init-state-absent-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let ws = Workspace::new(&dir);
        // No file on disk → a fresh default, not an error.
        assert_eq!(InitState::load(&ws).unwrap(), InitState::default());
    }

    #[test]
    fn init_state_mark_is_idempotent() {
        let mut state = InitState::default();
        state.mark(Stage::Git);
        state.mark(Stage::Git);
        assert_eq!(state.completed, vec![Stage::Git]);
        assert!(state.is_done(Stage::Git));
        assert!(!state.is_done(Stage::Labels));
    }

    #[test]
    fn init_state_path_is_under_gitignored_ralphy_dir() {
        let d = std::env::temp_dir().join("ralphy-init-state-path-check");
        assert!(Workspace::new(&d)
            .init_state_path()
            .starts_with(Workspace::new(&d).ralphy_dir()));
    }

    #[test]
    fn format_config_echo_contains_each_field() {
        let cfg = config_of(&sample_report());
        let echo = format_config_echo(&cfg);
        assert!(echo.contains("existing"), "repo kind missing:\n{echo}");
        assert!(echo.contains("Rust / cargo"), "language missing:\n{echo}");
        assert!(echo.contains("docs/backlog.md"), "backlog missing:\n{echo}");
        assert!(
            echo.contains("docs/roadmap.md"),
            "milestone missing:\n{echo}"
        );
        assert!(echo.contains(".claude"), "skills dir missing:\n{echo}");
        assert!(echo.contains("github.com"), "remote missing:\n{echo}");
        assert!(echo.contains("PRD/roadmap:"), "PRD opt-in missing:\n{echo}");
    }

    #[test]
    fn resolve_list_empty_keeps_default_none_clears_csv_splits() {
        let default = vec!["a.md".to_string(), "b.md".to_string()];
        // Empty input keeps the default.
        assert_eq!(resolve_list(&default, "  "), default);
        // The literal `none` clears it.
        assert!(resolve_list(&default, "none").is_empty());
        // A CSV override splits, trims, and drops blanks.
        assert_eq!(
            resolve_list(&default, " x.md , , y.md "),
            vec!["x.md".to_string(), "y.md".to_string()]
        );
    }

    #[test]
    fn diagnosis_cwd_is_outside_repo() {
        let repo = std::env::temp_dir().join("ralphy-some-repo");
        let cwd = diagnosis_cwd(&repo, "stamp123");
        assert_ne!(cwd, repo, "neutral cwd must not be the repo root");
        assert!(
            !cwd.starts_with(&repo),
            "neutral cwd {} must not be inside the repo {}",
            cwd.display(),
            repo.display()
        );
    }

    // ── scaffold: template selection, block upsert, decisions (#54) ─────────

    #[test]
    fn select_issue_tracker_picks_body_by_host() {
        assert!(select_issue_tracker(Some("github.com"))
            .1
            .contains("# Issue tracker: GitHub"));
        assert!(select_issue_tracker(Some("gitlab.com"))
            .1
            .contains("# Issue tracker: GitLab"));
        assert!(select_issue_tracker(None)
            .1
            .contains("# Issue tracker: Local Markdown"));
        // The on-disk filename is always issue-tracker.md regardless of host.
        assert_eq!(
            select_issue_tracker(Some("github.com")).0,
            "issue-tracker.md"
        );
    }

    fn block_cfg() -> InitConfig {
        config_of(&sample_report())
    }

    #[test]
    fn upsert_appends_block_when_absent() {
        let doc = "# Project\n\nSome intro.\n";
        let block = agent_skills_block(&block_cfg());
        let out = upsert_agent_skills_block(doc, &block);
        assert_eq!(
            out.matches("## Agent skills").count(),
            1,
            "exactly one heading:\n{out}"
        );
        assert!(
            out.contains("# Project"),
            "original content preserved:\n{out}"
        );
        assert!(
            out.trim_end().ends_with("docs/agents/domain.md`."),
            "block appended at end:\n{out}"
        );
    }

    #[test]
    fn upsert_replaces_existing_block_in_place() {
        let doc = "# Project\n\n## Agent skills\n\nOLD STALE BODY.\n\n## Other\n\nkeep me.\n";
        let block = agent_skills_block(&block_cfg());
        let out = upsert_agent_skills_block(doc, &block);
        assert_eq!(
            out.matches("## Agent skills").count(),
            1,
            "still exactly one heading:\n{out}"
        );
        assert!(!out.contains("OLD STALE BODY"), "old body gone:\n{out}");
        assert!(
            out.contains("docs/agents/issue-tracker.md"),
            "new summary present:\n{out}"
        );
        assert!(
            out.contains("## Other"),
            "trailing section preserved:\n{out}"
        );
        assert!(out.contains("keep me."), "trailing body preserved:\n{out}");
    }

    #[test]
    fn upsert_preserves_following_h1_sibling_section() {
        // Regression: a `# `/`## ` section after Agent skills must survive the
        // replace — only the section's own body (and any `### ` subsection) goes.
        let doc =
            "## Agent skills\n\nOLD BODY.\n\n### old sub\n\nnested old.\n\n# Top Level\n\nkeep me.\n";
        let block = agent_skills_block(&block_cfg());
        let out = upsert_agent_skills_block(doc, &block);
        assert_eq!(out.matches("## Agent skills").count(), 1);
        assert!(!out.contains("OLD BODY"), "old body gone:\n{out}");
        assert!(!out.contains("nested old"), "nested old sub gone:\n{out}");
        assert!(out.contains("# Top Level"), "h1 sibling preserved:\n{out}");
        assert!(out.contains("keep me."), "h1 body preserved:\n{out}");
    }

    #[test]
    fn commit_decision_maps_clean_dirty_yes_and_refusal() {
        assert_eq!(
            commit_decision(true, "anything"),
            CommitDecision::NothingToCommit
        );
        assert_eq!(commit_decision(false, "yes"), CommitDecision::Commit);
        assert_eq!(commit_decision(false, "y"), CommitDecision::Commit);
        // Empty input accepts the `[Y/n]` default and commits the snapshot.
        assert_eq!(commit_decision(false, ""), CommitDecision::Commit);
        match commit_decision(false, "no") {
            CommitDecision::Abort(msg) => assert!(!msg.is_empty()),
            other => panic!("expected Abort, got {other:?}"),
        }
    }

    #[test]
    fn branch_decision_maps_default_and_decline() {
        assert_eq!(
            branch_decision("main", ""),
            BranchDecision::Create("ralphy/init".into())
        );
        assert_eq!(
            branch_decision("main", "yes"),
            BranchDecision::Create("ralphy/init".into())
        );
        assert_eq!(branch_decision("main", "no"), BranchDecision::Stay);
        assert_eq!(branch_decision("main", "n"), BranchDecision::Stay);
    }

    #[test]
    fn write_scaffold_prd_opt_in_controls_prd_docs() {
        let dir = std::env::temp_dir().join(format!("ralphy-scaffold-prd-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let mut cfg = block_cfg();
        cfg.adopt_prd_roadmap = true;
        write_scaffold(&dir, &cfg).unwrap();
        assert!(dir.join("docs/roadmap.md").exists());
        assert!(dir.join("docs/prd/README.md").exists());
        assert!(dir.join("docs/prd/_template.md").exists());

        let dir2 =
            std::env::temp_dir().join(format!("ralphy-scaffold-noprd-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir2);
        std::fs::create_dir_all(&dir2).unwrap();
        let mut cfg2 = block_cfg();
        cfg2.adopt_prd_roadmap = false;
        write_scaffold(&dir2, &cfg2).unwrap();
        assert!(!dir2.join("docs/roadmap.md").exists());
        assert!(!dir2.join("docs/prd").exists());

        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&dir2);
    }

    #[test]
    fn init_git_safety_branch_and_scaffold_end_to_end() {
        use ralphy_core::git;

        let dir = std::env::temp_dir().join(format!("ralphy-init-e2e-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        git::git(&dir, &["init", "-q", "-b", "main"]).unwrap();
        git::git(&dir, &["config", "user.email", "t@example.com"]).unwrap();
        git::git(&dir, &["config", "user.name", "Test"]).unwrap();
        std::fs::write(dir.join("README.md"), "hello\n").unwrap();
        git::git(&dir, &["add", "."]).unwrap();
        git::git(&dir, &["commit", "-q", "-m", "init"]).unwrap();
        // Dirty the tree so the commit decision has work to do.
        std::fs::write(dir.join("README.md"), "changed\n").unwrap();

        // Drive the decision functions with literal "yes" answers, then the real
        // git/scaffold helpers — no stdin blocking.
        let is_clean = git::is_clean_ignoring_ralphy(&dir).unwrap();
        assert!(!is_clean, "tree should be dirty");
        match commit_decision(is_clean, "yes") {
            CommitDecision::Commit => git::commit_all_snapshot(&dir).unwrap(),
            other => panic!("expected Commit, got {other:?}"),
        }
        assert!(
            git::is_clean_ignoring_ralphy(&dir).unwrap(),
            "clean after snapshot"
        );

        let current = git::current_branch(&dir).unwrap();
        match branch_decision(&current, "") {
            BranchDecision::Create(branch) => {
                git::checkout_new_branch(&dir, &branch, &current).unwrap();
            }
            other => panic!("expected Create, got {other:?}"),
        }
        assert_eq!(git::current_branch(&dir).unwrap(), "ralphy/init");

        let mut cfg = block_cfg();
        cfg.adopt_prd_roadmap = false;
        write_scaffold(&dir, &cfg).unwrap();

        assert!(dir.join("docs/agents/issue-tracker.md").exists());
        assert!(dir.join("docs/agents/triage-labels.md").exists());
        assert!(dir.join("docs/agents/domain.md").exists());
        // SUSPENDED (under evaluation): the scaffold no longer writes the
        // `## Agent skills` block, so neither CLAUDE.md nor AGENTS.md is created.
        assert!(!dir.join("CLAUDE.md").exists());
        assert!(!dir.join("AGENTS.md").exists());
        // PRD opt-out: none of the PRD docs exist.
        assert!(!dir.join("docs/prd").exists());
        assert!(!dir.join("docs/roadmap.md").exists());

        // Idempotency: a second scaffold still writes no agent-instruction file.
        write_scaffold(&dir, &cfg).unwrap();
        assert!(!dir.join("CLAUDE.md").exists());
        assert!(!dir.join("AGENTS.md").exists());

        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── stage 6: skills download helpers ────────────────────────────────────

    #[test]
    fn skill_names_is_non_empty_and_contains_to_issues() {
        let names = skill_names();
        assert!(!names.is_empty(), "RALPHY_SKILLS must be non-empty");
        assert!(
            names.contains(&"to-issues"),
            "expected 'to-issues' in {:?}",
            names
        );
    }

    #[test]
    fn skills_target_maps_correctly() {
        assert_eq!(skills_target(None), PathBuf::from(".agents/skills"));
        assert_eq!(
            skills_target(Some(".codex")),
            PathBuf::from(".agents/skills")
        );
        assert_eq!(
            skills_target(Some(".claude")),
            PathBuf::from(".claude/skills")
        );
        assert_eq!(
            skills_target(Some(".cursor")),
            PathBuf::from(".cursor/skills")
        );
        // Already ends in "skills" → used as-is.
        assert_eq!(
            skills_target(Some(".agents/skills")),
            PathBuf::from(".agents/skills")
        );
    }

    #[test]
    fn download_decision_yes_true_others_false() {
        assert!(download_decision("yes"));
        assert!(download_decision("y"));
        assert!(download_decision("  Y  "));
        assert!(download_decision("YES"));
        assert!(!download_decision(""));
        assert!(!download_decision("n"));
        assert!(!download_decision("no"));
        assert!(!download_decision("maybe"));
    }

    #[test]
    fn labels_decision_empty_and_yes_proceed_no_declines() {
        assert!(labels_decision(""));
        assert!(labels_decision("y"));
        assert!(labels_decision("Y"));
        assert!(labels_decision("yes"));
        assert!(labels_decision("  YES  "));
        assert!(!labels_decision("n"));
        assert!(!labels_decision("no"));
        assert!(!labels_decision("maybe"));
    }

    #[test]
    fn resolve_fetch_ref_prefers_sha_falls_back_to_version() {
        // A real SHA wins over the (unresolvable) describe string.
        assert_eq!(
            resolve_fetch_ref(Some("27adb48abc"), "v0.1.0-rc6-19-g27adb48"),
            "27adb48abc"
        );
        // No SHA (no-git build) → the version is used as-is.
        assert_eq!(resolve_fetch_ref(None, "v0.1.0-rc6"), "v0.1.0-rc6");
        // An empty SHA is treated as absent.
        assert_eq!(resolve_fetch_ref(Some("  "), "v0.1.0-rc6"), "v0.1.0-rc6");
    }

    #[test]
    fn sparse_fetch_commands_contains_expected_argv() {
        let version = "v0.1.0";
        let subtree = "assets/agents_template/skills";
        let cmds = sparse_fetch_commands(version, subtree);
        let fetch_argv: Vec<String> = vec!["fetch", "--depth", "1", "origin", version]
            .into_iter()
            .map(str::to_string)
            .collect();
        let sc_set_argv: Vec<String> = vec!["sparse-checkout", "set", subtree]
            .into_iter()
            .map(str::to_string)
            .collect();
        assert!(cmds.contains(&fetch_argv), "missing fetch argv in {cmds:?}");
        assert!(
            cmds.contains(&sc_set_argv),
            "missing sparse-checkout set argv in {cmds:?}"
        );
        // No argv should reference a local path (token starts with `.` or is a
        // plain `name/name` without `://`) other than `subtree`.
        for argv in &cmds {
            for token in argv {
                let is_url = token.contains("://");
                if !is_url && token.contains('/') && token.as_str() != subtree {
                    panic!("unexpected path token {token:?} in {argv:?}");
                }
            }
        }
    }

    #[test]
    fn install_skills_from_idempotent_and_preserves_sibling() {
        let base = std::env::temp_dir().join(format!("ralphy-skills-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);

        // Build source fixture with two skill dirs.
        let src = base.join("src");
        std::fs::create_dir_all(src.join("skill-a")).unwrap();
        std::fs::write(src.join("skill-a").join("skill.md"), "skill-a content").unwrap();
        std::fs::create_dir_all(src.join("skill-b")).unwrap();
        std::fs::write(src.join("skill-b").join("skill.md"), "skill-b content").unwrap();

        let dst = base.join("dst");
        // Pre-create a stale file in managed skill and an unrelated sibling.
        std::fs::create_dir_all(dst.join("skill-a")).unwrap();
        std::fs::write(dst.join("skill-a").join("STALE.md"), "stale").unwrap();
        std::fs::create_dir_all(dst.join("user-skill")).unwrap();
        std::fs::write(dst.join("user-skill").join("keep.md"), "keep me").unwrap();

        // First install.
        let n = install_skills_from(&src, &dst).unwrap();
        assert_eq!(n, 2);
        // Stale file gone.
        assert!(
            !dst.join("skill-a").join("STALE.md").exists(),
            "STALE.md must be gone after install"
        );
        // Real skill file present.
        assert!(dst.join("skill-a").join("skill.md").exists());
        // Sibling preserved.
        assert!(
            dst.join("user-skill").join("keep.md").exists(),
            "user-skill sibling must survive"
        );

        // Second install (idempotency).
        let n2 = install_skills_from(&src, &dst).unwrap();
        assert_eq!(n2, 2);
        assert!(dst.join("skill-a").join("skill.md").exists());
        assert!(dst.join("user-skill").join("keep.md").exists());

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn install_skills_step_returns_failed_on_fetch_error() {
        let dst =
            std::env::temp_dir().join(format!("ralphy-skills-step-err-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dst);
        let result = install_skills_step(&dst, |_scratch| Err(anyhow::anyhow!("boom")));
        match result.unwrap() {
            Outcome::Failed(msg) => assert!(msg.contains("boom"), "expected 'boom' in {msg}"),
            other => panic!("expected Failed, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&dst);
    }

    #[test]
    fn install_skills_step_returns_installed_on_success() {
        let base =
            std::env::temp_dir().join(format!("ralphy-skills-step-ok-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);

        // Build a fixture source with one skill.
        let src = base.join("src");
        std::fs::create_dir_all(src.join("my-skill")).unwrap();
        std::fs::write(src.join("my-skill").join("skill.md"), "content").unwrap();

        let dst = base.join("dst");
        let src_clone = src.clone();
        let result = install_skills_step(&dst, move |_scratch| Ok(src_clone));
        match result.unwrap() {
            Outcome::Installed(n) => assert_eq!(n, 1, "expected 1 skill installed"),
            other => panic!("expected Installed, got {other:?}"),
        }
        assert!(dst.join("my-skill").join("skill.md").exists());

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn install_skills_step_returns_failed_when_install_fails() {
        // Prove that a post-fetch copy error is also absorbed (not propagated).
        let base = std::env::temp_dir().join(format!(
            "ralphy-skills-step-installfail-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&base);
        // Build a fixture source — but point the fetch closure at a *file*, not a dir,
        // so install_skills_from's read_dir fails.
        let bad_src = base.join("not-a-dir.txt");
        std::fs::create_dir_all(&base).unwrap();
        std::fs::write(&bad_src, "oops").unwrap();
        let dst = base.join("dst");
        let bad_src_clone = bad_src.clone();
        let result = install_skills_step(&dst, move |_scratch| Ok(bad_src_clone));
        match result.unwrap() {
            Outcome::Failed(_) => {} // expected
            other => panic!("expected Failed, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&base);
    }

    // ── stage 8: backlog/milestone → issues helpers (#57) ───────────────────

    #[test]
    fn decide_issues_path_milestone_backlog_and_skip() {
        let mut cfg = config_of(&sample_report());
        // Milestone path: opted in AND milestone docs present.
        cfg.adopt_prd_roadmap = true;
        assert_eq!(decide_issues_path(&cfg), IssuesPath::Milestone);

        // Not opted in but a backlog exists → loose-backlog path.
        cfg.adopt_prd_roadmap = false;
        cfg.backlog_location = Some("docs/backlog.md".into());
        assert_eq!(decide_issues_path(&cfg), IssuesPath::LooseBacklog);

        // Neither milestone (opt-in off) nor backlog → skip cleanly.
        cfg.backlog_location = None;
        assert_eq!(decide_issues_path(&cfg), IssuesPath::Skip);

        // Opted in but NO milestone docs, with a backlog → loose-backlog, not milestone.
        cfg.adopt_prd_roadmap = true;
        cfg.milestone_docs = vec![];
        cfg.backlog_location = Some("BACKLOG.md".into());
        assert_eq!(decide_issues_path(&cfg), IssuesPath::LooseBacklog);
    }

    #[test]
    fn draft_decision_empty_and_yes_proceed_no_declines() {
        // Default-Yes: silence accepts the `[Y/n]` default and drafts.
        assert!(draft_decision(""));
        assert!(draft_decision("y"));
        assert!(draft_decision("  YES "));
        assert!(!draft_decision("n"));
        assert!(!draft_decision("no"));
        assert!(!draft_decision("nah"));
    }

    #[test]
    fn publish_decision_only_yes_proceeds() {
        assert!(publish_decision("y"));
        assert!(publish_decision("yes"));
        assert!(publish_decision("  YES "));
        // Default-No: silence and anything else declines.
        assert!(!publish_decision(""));
        assert!(!publish_decision("n"));
        assert!(!publish_decision("maybe"));
    }

    fn sample_draft() -> IssuesDraft {
        IssuesDraft {
            milestone: Some(ralphy_core::MilestoneDraft {
                title: "v1".into(),
                description: "first".into(),
            }),
            prd_path: Some("docs/prd/0001.md".into()),
            issues: vec![
                ralphy_core::IssueDraft {
                    title: "slice one".into(),
                    body: "## Blocked by\n\nBLOCKED_BY_PLACEHOLDER\n".into(),
                    labels: vec!["ready-for-agent".into()],
                    blocked_by: vec![],
                },
                ralphy_core::IssueDraft {
                    title: "slice two".into(),
                    body: "## Blocked by\n\nBLOCKED_BY_PLACEHOLDER\n".into(),
                    labels: vec!["ready-for-agent".into()],
                    blocked_by: vec![0],
                },
            ],
        }
    }

    #[test]
    fn format_draft_summary_reports_count_milestone_and_blocked_by() {
        let summary = format_draft_summary(&sample_draft());
        assert!(summary.contains("2 issue(s)"), "count:\n{summary}");
        assert!(
            summary.contains("milestone \"v1\""),
            "milestone:\n{summary}"
        );
        assert!(summary.contains("docs/prd/0001.md"), "prd:\n{summary}");
        assert!(summary.contains("slice two"), "issue title:\n{summary}");
        // blocked_by index 0 → 1-based "#1" in the human summary.
        assert!(
            summary.contains("blocked by #1"),
            "blocked-by ref:\n{summary}"
        );
    }

    #[test]
    fn patch_blocked_by_replaces_placeholder_and_handles_empty() {
        let body = "## Blocked by\n\nBLOCKED_BY_PLACEHOLDER\n";
        // With blockers: real refs.
        let patched = patch_blocked_by(body, &[7, 9]);
        assert!(patched.contains("- #7"));
        assert!(patched.contains("- #9"));
        assert!(!patched.contains("BLOCKED_BY_PLACEHOLDER"));
        // No blockers: the "can start immediately" line.
        let none = patch_blocked_by(body, &[]);
        assert!(none.contains("None - can start immediately"));
        assert!(!none.contains("BLOCKED_BY_PLACEHOLDER"));
        // Absent placeholder: returned unchanged.
        let plain = "no placeholder here";
        assert_eq!(patch_blocked_by(plain, &[1]), plain);
    }

    fn three_issue_draft() -> IssuesDraft {
        let issue = |title: &str, blocked_by: Vec<usize>| ralphy_core::IssueDraft {
            title: title.into(),
            body: "## Blocked by\n\nBLOCKED_BY_PLACEHOLDER\n".into(),
            labels: vec!["ready-for-agent".into()],
            blocked_by,
        };
        IssuesDraft {
            milestone: Some(ralphy_core::MilestoneDraft {
                title: "v1".into(),
                description: "first".into(),
            }),
            prd_path: None,
            issues: vec![
                issue("slice one", vec![]),
                issue("slice two", vec![0]),
                issue("slice three", vec![1]),
            ],
        }
    }

    #[test]
    fn publish_draft_with_never_recreates_persisted_prefix() {
        // Resume case: two issues + the milestone already published. Only the 3rd
        // issue is created; the milestone is NOT recreated.
        let draft = three_issue_draft();
        let mut state = InitState {
            created_issues: vec![101, 102],
            milestone_created: Some("v1".into()),
            ..InitState::default()
        };

        let mut ms_calls = 0;
        let mut issue_titles: Vec<String> = Vec::new();
        let mut save_calls = 0;
        publish_draft_with(
            &draft,
            &mut state,
            |_s| {
                save_calls += 1;
                Ok(())
            },
            |_t, _d| {
                ms_calls += 1;
                Ok(999)
            },
            |title, body, _labels, milestone| {
                issue_titles.push(title.to_string());
                // The 3rd issue is blocked_by index 1 → must resolve to the
                // persisted #102, proving blocked-by resolves against the prefix.
                assert!(body.contains("- #102"), "blocked-by resolved:\n{body}");
                assert_eq!(milestone, Some("v1"));
                Ok(103)
            },
        )
        .unwrap();

        assert_eq!(issue_titles, vec!["slice three".to_string()]);
        assert_eq!(ms_calls, 0, "milestone must NOT be recreated");
        assert_eq!(state.created_issues, vec![101, 102, 103]);
        assert!(save_calls >= 1, "save must fire after the create");

        // Fresh case: nothing published yet. Milestone created once, all 3 issues
        // created in order, numbers accumulate.
        let draft = three_issue_draft();
        let mut state = InitState::default();
        let mut ms_calls = 0;
        let mut next = 200u64;
        let mut issue_titles: Vec<String> = Vec::new();
        let mut save_calls = 0;
        publish_draft_with(
            &draft,
            &mut state,
            |_s| {
                save_calls += 1;
                Ok(())
            },
            |_t, _d| {
                ms_calls += 1;
                Ok(1)
            },
            |title, _body, _labels, milestone| {
                issue_titles.push(title.to_string());
                assert_eq!(milestone, Some("v1"));
                let n = next;
                next += 1;
                Ok(n)
            },
        )
        .unwrap();

        assert_eq!(ms_calls, 1, "milestone created exactly once");
        assert_eq!(
            issue_titles,
            vec![
                "slice one".to_string(),
                "slice two".to_string(),
                "slice three".to_string()
            ]
        );
        assert_eq!(state.created_issues, vec![200, 201, 202]);
        assert_eq!(state.milestone_created.as_deref(), Some("v1"));
        // save after milestone + after each of 3 issues.
        assert_eq!(save_calls, 4);
    }

    #[test]
    fn load_issues_draft_round_trips_persisted_draft() {
        // The partial-publish resume path reloads this exact file instead of
        // regenerating, so it must parse what publish writes.
        let dir = std::env::temp_dir().join(format!("ralphy-draft-reload-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let ws = Workspace::new(&dir);
        std::fs::create_dir_all(ws.ralphy_dir()).unwrap();

        let draft = three_issue_draft();
        let path = ws.issues_draft_path();
        std::fs::write(&path, serde_json::to_string_pretty(&draft).unwrap()).unwrap();

        let back = load_issues_draft(&path).unwrap();
        assert_eq!(back, draft);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn neutral_cwd_falls_back_when_temp_base_is_inside_repo() {
        // A repo-local temp base would put the "neutral" cwd inside the target,
        // breaking the read-only invariant — the fallback must move it outside.
        let repo = Path::new("/some/target/repo");
        let base_inside = repo.join("tmp");
        let cwd = neutral_cwd_from(&base_inside, repo, "s1");
        assert!(
            !cwd.starts_with(repo),
            "fallback cwd {} must be outside the repo {}",
            cwd.display(),
            repo.display()
        );
    }

    // ── stage 10: verify report helpers (#59) ────────────────────────────────

    fn report_all_present(queue: Vec<u64>) -> VerifyReport {
        VerifyReport {
            ralphy_present: true,
            docs: vec![
                ("docs/agents/issue-tracker.md", true),
                ("docs/agents/triage-labels.md", true),
                ("docs/agents/domain.md", true),
            ],
            ralphy_label_count: 9,
            skill_count: 5,
            queue,
            branch: "main".into(),
            logged_in: vec!["claude".into()],
        }
    }

    #[test]
    fn format_final_report_nonempty_queue_has_next_step() {
        let r = report_all_present(vec![7, 12]);
        let output = format_final_report(&r);
        assert!(
            output.contains("ralphy run --only-issue 7 --dry-run"),
            "expected next-step command in:\n{output}"
        );
        assert!(
            !output.contains("warning:"),
            "expected no warning in:\n{output}"
        );
    }

    #[test]
    fn format_final_report_empty_queue_warns() {
        let r = report_all_present(vec![]);
        let output = format_final_report(&r);
        assert!(
            output.contains("warning: no queue-labeled issue"),
            "expected warning in:\n{output}"
        );
        assert!(
            !output.contains("ralphy run --only-issue"),
            "expected no next-step line in:\n{output}"
        );
    }

    #[test]
    fn format_final_report_marks_missing_artifact() {
        let r = VerifyReport {
            ralphy_present: true,
            docs: vec![
                ("docs/agents/issue-tracker.md", true),
                ("docs/agents/triage-labels.md", true),
                ("docs/agents/domain.md", false),
            ],
            ralphy_label_count: 0,
            skill_count: 0,
            queue: vec![],
            branch: "main".into(),
            logged_in: vec![],
        };
        let output = format_final_report(&r);
        // Find the line containing docs/agents/domain.md and assert it has MISSING.
        let domain_line = output
            .lines()
            .find(|l| l.contains("docs/agents/domain.md"))
            .expect("expected a line for docs/agents/domain.md");
        assert!(
            domain_line.contains("MISSING"),
            "expected MISSING on domain.md line:\n{domain_line}"
        );

        assert_eq!(
            required_artifacts_missing(&r),
            vec!["docs/agents/domain.md".to_string()]
        );
    }

    #[test]
    fn smoke_test_decision_default_declines() {
        assert!(!smoke_test_decision(""), "empty should decline");
        assert!(!smoke_test_decision("n"), "n should decline");
        assert!(smoke_test_decision("y"), "y should accept");
        assert!(smoke_test_decision("yes"), "yes should accept");
    }

    #[test]
    fn suggested_issue_picks_lowest() {
        assert_eq!(suggested_issue(&[7, 12]), Some(7));
        assert_eq!(suggested_issue(&[]), None);
    }
}
