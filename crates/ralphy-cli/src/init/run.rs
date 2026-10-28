use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use clap::Args;
use ralphy_core::{git, github, gitignore, DiagnosisReport, Workspace};

use super::gate::Agent;
use super::render::{ask_yes_no, print_note, print_ok, print_section, with_spinner};
use super::verify::finalize;
use super::wizard::InitState;
use crate::doctor::gh_signed_in;

mod decisions;
mod stages;

pub use decisions::{
    create_repo_decision, labels_decision, private_visibility_decision, repo_name_from_path,
};
use decisions::{init_model_for, select_agent};

#[derive(Args)]
pub struct InitArgs {
    /// Any folder inside the repo.
    #[arg(long, default_value = ".")]
    pub repo: PathBuf,

    /// The agent that studies the repo and writes the first issues. It must be
    /// logged in. Default: the first logged-in agent found (claude, then codex,
    /// then opencode).
    #[arg(long, value_enum)]
    pub agent: Option<Agent>,
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
        Agent::Claude => ralphy_agent_claude::diagnose_repo(
            repo,
            neutral_cwd,
            model,
            effort,
            timeout,
            crate::config::claude_guard_proof_waiver(repo),
        ),

        Agent::Codex => {
            ralphy_agent_codex::diagnose_repo(repo, neutral_cwd, model, effort, timeout)
        }

        Agent::Copilot => {
            ralphy_agent_copilot::diagnose_repo(repo, neutral_cwd, model, effort, timeout)
        }
        Agent::Gemini => {
            ralphy_agent_gemini::diagnose_repo(repo, neutral_cwd, model, effort, timeout)
        }
        Agent::Cursor => {
            ralphy_agent_cursor::diagnose_repo(repo, neutral_cwd, model, effort, timeout)
        }

        Agent::Kimi => ralphy_agent_kimi::diagnose_repo(repo, neutral_cwd, model, effort, timeout),

        Agent::Opencode => {
            ralphy_agent_opencode::diagnose_repo(repo, neutral_cwd, model, effort, timeout)
        }
    }
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
    if !gh_signed_in() {
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
    crate::daemon::register_repo(&repo);

    stages::show_banner(&repo);

    // Ignore `.ralphy/` before any snapshot commit, so the checkpoint
    // (`init-state.json`) and every other scratch artifact stay out of commits.
    gitignore::ensure_ralphy_ignored(&repo)?;

    let findings = stages::check_environment(&repo)?;

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

    let cfg = stages::capture_config(&repo, &ws, &mut state, selected_agent, init_model)?;
    stages::git_stage(&repo, &ws, &mut state)?;
    stages::scaffold_stage(&repo, &ws, &mut state, &cfg)?;
    stages::skills_stage(&repo, &ws, &mut state, &cfg)?;
    stages::labels_stage(&repo, &ws, &mut state)?;
    stages::issues_stage(&repo, &ws, &mut state, &cfg, selected_agent, init_model)?;

    finalize(&repo, &cfg, &findings.agents_logged_in, selected_agent)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::init::scaffold::write_scaffold;

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

    fn block_cfg() -> crate::init::InitConfig {
        crate::init::InitConfig {
            repo_kind: ralphy_core::RepoKind::Existing,
            language_build: Some("Rust / cargo".into()),
            backlog_location: Some("docs/backlog.md".into()),
            milestone_docs: vec!["docs/roadmap.md".into(), "docs/prd/0001.md".into()],
            skills_dir: Some(".claude".into()),
            has_context_or_adrs: true,
            remote_host: Some("github.com".into()),
            adopt_prd_roadmap: true,
        }
    }

    #[test]
    fn write_scaffold_writes_the_agent_docs_and_no_instruction_file() {
        let dir = std::env::temp_dir().join(format!("ralphy-init-scaffold-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

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

        // Idempotency: a second scaffold still writes no agent-instruction file.
        write_scaffold(&dir, &cfg).unwrap();
        assert!(!dir.join("CLAUDE.md").exists());
        assert!(!dir.join("AGENTS.md").exists());

        let _ = std::fs::remove_dir_all(&dir);
    }
}
