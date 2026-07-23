use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use clap::Args;
use ralphy_core::{git, github, gitignore, DiagnosisReport, DraftRequest, IssuesMode, Workspace};

use super::gate::{
    agent_logged_in, agent_present, evaluate_gate, gh_authenticated, git_present, github_remote,
    python_present, Agent, EnvFindings,
};
use super::issues::{
    decide_issues_path, draft_decision, draft_with_agent, format_draft_summary, load_issues_draft,
    publish_decision, publish_draft, resolve_triage_label, IssuesPath,
};
use super::render::{
    ask_yes_no, print_bullet, print_captured_config, print_gate_report, print_note, print_ok,
    print_section, run_qa, with_spinner,
};
use super::scaffold::write_scaffold;
use super::skills::{
    download_decision, install_skills_step, skill_names, skills_target, sparse_fetch_commands,
    Outcome, SKILLS_REF, SKILLS_SUBTREE,
};
use super::verify::finalize;
use super::wizard::{persist_report, InitState, Stage};

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
        Agent::Codex
        | Agent::Copilot
        | Agent::Cursor
        | Agent::Gemini
        | Agent::Opencode
        | Agent::Kimi => None,
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
    crate::daemon::register_repo(&repo);

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
            git: git_present(),
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
        let report: DiagnosisReport = if ws.diagnosis_path().exists() {
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
        // NOTE: displayed list is the static INSTALLABLE_SKILLS; the downloaded set
        // is whatever `main` of the skills repo holds (see SKILLS_REF) and may drift.
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
            let fetch_ref = SKILLS_REF.to_string();
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

        return finalize(&repo, &cfg, &findings.agents_logged_in, selected_agent);
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
        return finalize(&repo, &cfg, &findings.agents_logged_in, selected_agent);
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
                return finalize(&repo, &cfg, &findings.agents_logged_in, selected_agent);
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

    finalize(&repo, &cfg, &findings.agents_logged_in, selected_agent)
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn init_model_pins_sonnet_for_claude_only() {
        assert_eq!(init_model_for(Agent::Claude), Some("sonnet"));
        assert_eq!(init_model_for(Agent::Codex), None);
        assert_eq!(init_model_for(Agent::Kimi), None);
        assert_eq!(init_model_for(Agent::Opencode), None);
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
}
