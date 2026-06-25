//! `ralphy init`: deterministic environment gate (ADR-0012 stage 1), then a
//! read-only repo diagnosis from a neutral cwd (stage 2) and a diagnosis-seeded
//! console Q&A captured into a typed config (stage 3), the git-safety snapshot +
//! `ralphy/init` branch (stage 4) and the deterministic scaffold from the embedded
//! setup-pocock templates (stage 5). Stages 6–10 are stubbed.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use clap::Args;
use ralphy_adapter_support::find_program;
use ralphy_core::{git, DiagnosisReport, RepoKind, Workspace};

#[derive(Args)]
pub struct InitArgs {
    /// Any path inside the target repo; resolved to its git toplevel.
    #[arg(long, default_value = ".")]
    pub repo: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

fn agent_present(a: &Agent) -> bool {
    let path = std::env::var_os("PATH");
    let pathext = std::env::var_os("PATHEXT");
    find_program(a.cli_name(), path, pathext).is_some()
}

fn agent_logged_in(a: &Agent) -> bool {
    let hello = "hello";
    let mut cmd = match a {
        Agent::Claude => {
            let mut c = std::process::Command::new("claude");
            c.args(["-p", hello]);
            c
        }
        Agent::Codex => {
            let mut c = std::process::Command::new("codex");
            c.args(["exec", hello]);
            c.env_remove("OPENAI_API_KEY");
            c
        }
        Agent::Opencode => {
            let mut c = std::process::Command::new("opencode");
            c.args(["run", hello]);
            c
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

    println!("Environment gate passed. Diagnosing repo (read-only)…");

    // Diagnose from a neutral cwd OUTSIDE the repo, so the target's
    // CLAUDE.md/AGENTS.md are read as data, never auto-loaded as instructions.
    let stamp = format!("{}", std::process::id());
    let cwd = diagnosis_cwd(&repo, &stamp);
    let report = ralphy_agent_claude::diagnose_repo(
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
