//! `ralphy init`: deterministic environment gate (ADR-0012 stage 1), then a
//! read-only repo diagnosis from a neutral cwd (stage 2) and a diagnosis-seeded
//! console Q&A captured into a typed config (stage 3). Stages 4–10 are stubbed.

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
