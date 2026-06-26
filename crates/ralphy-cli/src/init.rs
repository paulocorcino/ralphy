//! `ralphy init`: deterministic environment gate (ADR-0012 stage 1), then a
//! read-only repo diagnosis from a neutral cwd (stage 2) and a diagnosis-seeded
//! console Q&A captured into a typed config (stage 3), the git-safety snapshot +
//! `ralphy/init` branch (stage 4), the deterministic scaffold from the embedded
//! setup-pocock templates (stage 5), the optional sparse-checkout download of
//! engineering skills pinned to `RALPHY_VERSION` (stage 6), the idempotent
//! GitHub label vocabulary creation (stage 7), and the conditional
//! backlog/milestone → issues judgment with a local preview the dev confirms
//! before any publish (stage 8). Stages 9–10 are stubbed.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use clap::{Args, ValueEnum};
use ralphy_adapter_support::{find_program, locate_program, resolve_program};
use ralphy_core::{
    git, github, DiagnosisReport, DraftRequest, IssuesDraft, IssuesMode, RepoKind, Workspace,
};

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

fn agent_logged_in(a: &Agent) -> bool {
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
#[derive(Debug, Clone, PartialEq, Eq)]
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

/// One seeded console question: the prompt label and the diagnosis-derived
/// default the dev confirms (empty input) or overrides.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Question {
    pub label: String,
    pub default: String,
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
            label: "Repo kind (empty/existing)".into(),
            default: display_kind(report.repo_kind),
        },
        Question {
            label: "Language / build".into(),
            default: display_opt(report.language_build.as_deref()),
        },
        Question {
            label: "Backlog location".into(),
            default: display_opt(report.backlog_location.as_deref()),
        },
        Question {
            label: "Milestone docs (comma-separated)".into(),
            default: display_list(&report.milestone_docs),
        },
        Question {
            label: "Skills directory".into(),
            default: display_opt(report.skills_dir.as_deref()),
        },
        Question {
            label: "Has CONTEXT.md or ADRs (yes/no)".into(),
            default: display_bool(report.has_context_or_adrs),
        },
        Question {
            label: "Remote host".into(),
            default: display_opt(report.remote_host.as_deref()),
        },
        Question {
            label: "Adopt PRD/roadmap track model (yes/no)".into(),
            default: display_bool(!report.milestone_docs.is_empty()),
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

/// Run the interactive, diagnosis-seeded Q&A on real stdin/stdout, resolving each
/// answer into an [`InitConfig`]. The pure resolvers do the work; this is the thin
/// impure shell (printing prompts, reading lines).
fn run_qa(report: &DiagnosisReport) -> Result<InitConfig> {
    let questions = seed_questions(report);
    let read_line = |label: &str, default: &str| -> Result<String> {
        print!("{label} [{default}]: ");
        std::io::stdout().flush().ok();
        let mut line = String::new();
        std::io::stdin()
            .read_line(&mut line)
            .context("reading answer from stdin")?;
        Ok(line)
    };

    // Indices match the order in `seed_questions`.
    let repo_kind = resolve_kind(
        report.repo_kind,
        &read_line(&questions[0].label, &questions[0].default)?,
    );
    let language_build = resolve_text(
        report.language_build.as_deref(),
        &read_line(&questions[1].label, &questions[1].default)?,
    );
    let backlog_location = resolve_text(
        report.backlog_location.as_deref(),
        &read_line(&questions[2].label, &questions[2].default)?,
    );
    let milestone_docs = resolve_list(
        &report.milestone_docs,
        &read_line(&questions[3].label, &questions[3].default)?,
    );
    let skills_dir = resolve_text(
        report.skills_dir.as_deref(),
        &read_line(&questions[4].label, &questions[4].default)?,
    );
    let has_context_or_adrs = resolve_bool(
        report.has_context_or_adrs,
        &read_line(&questions[5].label, &questions[5].default)?,
    );
    let remote_host = resolve_text(
        report.remote_host.as_deref(),
        &read_line(&questions[6].label, &questions[6].default)?,
    );
    let adopt_prd_roadmap = resolve_bool(
        !report.milestone_docs.is_empty(),
        &read_line(&questions[7].label, &questions[7].default)?,
    );

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

    // The block target: CLAUDE.md if present, else AGENTS.md if present, else a
    // fresh CLAUDE.md.
    let claude = repo.join("CLAUDE.md");
    let agents = repo.join("AGENTS.md");
    let target = if claude.exists() {
        claude
    } else if agents.exists() {
        agents
    } else {
        claude
    };
    let existing = std::fs::read_to_string(&target).unwrap_or_default();
    let updated = upsert_agent_skills_block(&existing, &agent_skills_block(cfg));
    std::fs::write(&target, updated).with_context(|| format!("writing {}", target.display()))?;

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
/// dirty tree commits on yes and aborts on anything else (no/empty/unknown), so a
/// refusal stops init before any branch or scaffold write.
fn commit_decision(is_clean: bool, answer: &str) -> CommitDecision {
    if is_clean {
        return CommitDecision::NothingToCommit;
    }
    match answer.trim().to_ascii_lowercase().as_str() {
        "y" | "yes" => CommitDecision::Commit,
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
fn resolve_triage_label(repo: &Path) -> String {
    let triage_doc = std::fs::read_to_string(repo.join("docs/agents/triage-labels.md")).ok();
    triage_doc
        .as_deref()
        .and_then(|d| github::parse_triage_mapping(d, "ready-for-agent"))
        .unwrap_or_else(|| "ready-for-agent".to_string())
}

/// Publish a confirmed draft to GitHub: create the milestone first (milestone
/// path), then each issue in array order, mapping each draft index to its created
/// number so a later issue's `blocked_by` indices resolve to real `#N` refs in its
/// body. Returns the created issue numbers in array order.
fn publish_draft(repo: &Path, draft: &IssuesDraft) -> Result<Vec<u64>> {
    // Create the milestone first so `gh issue create --milestone <name>` resolves;
    // each issue links to it by name (the number, returned here, is just logged).
    let milestone_name = match &draft.milestone {
        Some(ms) => {
            let number = github::create_milestone(repo, &ms.title, &ms.description)?;
            println!("  created milestone #{number}: {}", ms.title);
            Some(ms.title.as_str())
        }
        None => None,
    };
    let mut created: Vec<u64> = Vec::with_capacity(draft.issues.len());
    for issue in &draft.issues {
        // A blocker index must point at an earlier (already-created) issue; guard
        // against an out-of-range index rather than panicking on a bad draft.
        let blocked_numbers: Vec<u64> = issue
            .blocked_by
            .iter()
            .filter_map(|&idx| created.get(idx).copied())
            .collect();
        let body = patch_blocked_by(&issue.body, &blocked_numbers);
        let number =
            github::create_issue(repo, &issue.title, &body, &issue.labels, milestone_name)?;
        println!("  created #{number}: {}", issue.title);
        created.push(number);
    }
    Ok(created)
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

pub fn run(args: &InitArgs) -> Result<()> {
    let repo = git::resolve_toplevel(&args.repo)?;

    let agents_present: Vec<Agent> = Agent::ALL.iter().copied().filter(agent_present).collect();

    let agents_logged_in: Vec<Agent> = agents_present
        .iter()
        .copied()
        .filter(agent_logged_in)
        .collect();

    let findings = EnvFindings {
        python: python_present(),
        gh_authenticated: gh_authenticated(),
        github_remote: github_remote(&repo),
        agents_present,
        agents_logged_in,
    };

    let fails = evaluate_gate(&findings);
    print!("{}", format_report(&findings, &fails));

    if !fails.is_empty() {
        bail!(
            "ralphy init: environment gate failed ({} blocker(s)) — see report above",
            fails.len()
        );
    }

    // Pick the agent that drives diagnosis + issue drafting (explicit --agent, or
    // the first logged-in agent the gate found). The gate above guarantees ≥1.
    let selected_agent = select_agent(args.agent, &findings.agents_logged_in)?;
    println!(
        "Environment gate passed. Using agent: {}.",
        selected_agent.cli_name()
    );
    println!("Diagnosing repo (read-only)…");

    // Diagnose from a neutral cwd OUTSIDE the repo, so the target's
    // CLAUDE.md/AGENTS.md are read as data, never auto-loaded as instructions.
    let stamp = format!("{}", std::process::id());
    let cwd = diagnosis_cwd(&repo, &stamp);
    let report = diagnose_with_agent(
        selected_agent,
        &repo,
        &cwd,
        None,
        Some("medium"),
        Duration::from_secs(300),
    )?;

    let ws = Workspace::new(&repo);
    persist_report(&ws, &report)?;
    println!("Diagnosis written to {}", ws.diagnosis_path().display());

    // Console Q&A pre-filled by the diagnosis — the dev confirms/corrects.
    println!("\nConfirm or correct the findings (Enter keeps the default, 'none' clears):");
    let cfg = run_qa(&report)?;

    println!("\nCaptured config:");
    print!("{}", format_config_echo(&cfg));

    // ── stage 4: git safety (snapshot commit) ──────────────────────────────
    let prompt = |label: &str| -> Result<String> {
        print!("{label}");
        std::io::stdout().flush().ok();
        let mut line = String::new();
        std::io::stdin()
            .read_line(&mut line)
            .context("reading answer from stdin")?;
        Ok(line)
    };

    let is_clean = git::is_clean_ignoring_ralphy(&repo)?;
    if !is_clean {
        println!("\nWorking tree is dirty:");
        print!("{}", git::git(&repo, &["status", "--short"])?);
        let answer = prompt("\nCommit a snapshot before init (git add -A && git commit)? [Y/n]: ")?;
        match commit_decision(is_clean, &answer) {
            CommitDecision::Abort(msg) => {
                // INVARIANT: a refusal stops here — before any branch or write.
                bail!("{msg}");
            }
            CommitDecision::Commit => {
                git::commit_all_snapshot(&repo)?;
                println!("Snapshot committed.");
            }
            CommitDecision::NothingToCommit => {}
        }
    }

    // ── stage 4b: branch (before any scaffold write) ────────────────────────
    let current = git::current_branch(&repo)?;
    let answer = prompt("\nCreate branch `ralphy/init` for init's changes? [Y/n]: ")?;
    match branch_decision(&current, &answer) {
        BranchDecision::Create(branch) => {
            if git::commitish_exists(&repo, &branch) {
                git::checkout(&repo, &branch)?;
            } else {
                git::checkout_new_branch(&repo, &branch, &current)?;
            }
            println!("On branch {branch}.");
        }
        BranchDecision::Stay => {
            println!("Staying on branch {current}.");
        }
    }

    // ── stage 5: deterministic scaffold (onto the branch) ───────────────────
    write_scaffold(&repo, &cfg)?;
    println!("\nScaffold written:");
    println!("  docs/agents/issue-tracker.md");
    println!("  docs/agents/triage-labels.md");
    println!("  docs/agents/domain.md");
    if cfg.adopt_prd_roadmap {
        println!("  docs/roadmap.md");
        println!("  docs/prd/README.md");
        println!("  docs/prd/_template.md");
    }

    // ── stage 6: download engineering skills ────────────────────────────────
    let names = skill_names();
    let skills_dst = repo.join(skills_target(cfg.skills_dir.as_deref()));
    // NOTE: displayed list is from the build-time tree; downloaded set is from
    // the pinned commit (see resolve_fetch_ref) and may differ across builds.
    println!("\nEngineering skills available: {}", names.join(", "));
    println!("Target: {}", skills_dst.display());
    let answer = prompt(&format!(
        "Download these engineering skills into {}? [y/N]: ",
        skills_dst.display()
    ))?;
    if !download_decision(&answer) {
        println!("Skipping skills download.");
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
        match install_skills_step(&skills_dst, fetch)? {
            Outcome::Installed(n) => {
                println!("Installed {n} skill(s) into {}.", skills_dst.display())
            }
            Outcome::Skipped => println!("Skills already up to date."),
            Outcome::Failed(msg) => {
                println!("warning: skills download failed ({msg}); continuing");
            }
        }
    }

    // ── stage 7: create GitHub label vocabulary ──────────────────────────────
    let triage_doc = std::fs::read_to_string(repo.join("docs/agents/triage-labels.md")).ok();
    let desired = github::ralphy_label_specs(triage_doc.as_deref());
    let existing = github::list_repo_labels(&repo)?;
    let actions = github::plan_label_actions(&desired, &existing);
    print!(
        "\nGitHub label plan:\n{}",
        github::format_label_plan(&actions)
    );
    let answer = prompt("\nCreate/update these labels on GitHub? [Y/n]: ")?;
    if labels_decision(&answer) {
        github::apply_label_actions(&actions, &repo)?;
        println!("Labels created/updated.");
    } else {
        println!("Skipping label creation.");
    }

    // ── stage 8: backlog/milestone → issues (preview, confirm, publish) ───────
    match decide_issues_path(&cfg) {
        IssuesPath::Skip => {
            println!("\nNo backlog or milestone found — skipping issue creation.");
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
            let triage_label = resolve_triage_label(&repo);
            let draft_path = ws.issues_draft_path();
            println!("\nDrafting issues from the backlog/milestone (read-only preview)…");
            let req = DraftRequest {
                mode,
                source_docs: &source_docs,
                triage_label: &triage_label,
            };
            let draft = draft_with_agent(
                selected_agent,
                &repo,
                &draft_path,
                &req,
                None,
                Some("medium"),
                Duration::from_secs(600),
            )?;
            println!("Draft written to {}", draft_path.display());

            println!("\nPreview:\n{}", format_draft_summary(&draft));
            let answer = prompt("Publish these issues to GitHub? [y/N]: ")?;
            if publish_decision(&answer) {
                let created = publish_draft(&repo, &draft)?;
                println!("Published {} issue(s).", created.len());
            } else {
                println!("Skipping publish; draft kept at {}.", draft_path.display());
            }
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
        match commit_decision(false, "no") {
            CommitDecision::Abort(msg) => assert!(!msg.is_empty()),
            other => panic!("expected Abort, got {other:?}"),
        }
        // An empty answer is a refusal too — never commit on silence.
        assert!(matches!(
            commit_decision(false, ""),
            CommitDecision::Abort(_)
        ));
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
        let claude = std::fs::read_to_string(dir.join("CLAUDE.md")).unwrap();
        assert_eq!(claude.matches("## Agent skills").count(), 1);
        // PRD opt-out: none of the PRD docs exist.
        assert!(!dir.join("docs/prd").exists());
        assert!(!dir.join("docs/roadmap.md").exists());

        // Idempotency: a second scaffold leaves a single block.
        write_scaffold(&dir, &cfg).unwrap();
        let claude2 = std::fs::read_to_string(dir.join("CLAUDE.md")).unwrap();
        assert_eq!(claude2.matches("## Agent skills").count(), 1);

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
}
