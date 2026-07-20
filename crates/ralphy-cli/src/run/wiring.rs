//! Run-config wiring: the vendor-neutral plumbing `run_cmd` uses to turn resolved
//! flags into a ready-to-drive run — queue construction, adapter selection, agent
//! preflight, per-phase limit stance, the env scrub, the operating-branch derivation,
//! and the tracing/presenter install. All pure of the final-panel reporting (that
//! lives in [`super::report`]); none of it owns lifecycle ordering — the orchestrator
//! keeps the ordering-sensitive side effects in `run_cmd`.

use std::path::PathBuf;

use anyhow::Result;
use ralphy_agent_claude::ClaudeAgent;
use ralphy_agent_codex::CodexAgent;
use ralphy_agent_copilot::CopilotAgent;
use ralphy_agent_kimi::KimiAgent;
use ralphy_agent_opencode::OpenCodeAgent;
use ralphy_core::{github, Agent, BranchMode};
use tracing::warn;

use crate::cli::{CliAgent, RunArgs};
use crate::non_empty;
use crate::{config, delivery, events, ui};

/// The five Claude-only run knobs resolved once (flag > settings.json >
/// hardcoded default, ADR-0010) so the executor and an optional split planner
/// share one value. Strings are guaranteed non-empty by the resolvers.
pub(crate) struct ResolvedClaude {
    pub(crate) plan_model: String,
    pub(crate) plan_effort: String,
    pub(crate) exec_effort: String,
    pub(crate) default_exec_model: String,
    pub(crate) max_minutes_per_issue: u64,
    pub(crate) remote_control: bool,
}

/// The two Copilot per-phase model overrides resolved once (flag, then
/// settings.json, then `None`, ADR-0010/ADR-0041 D4) so the executor and an
/// optional split planner share one value. `None` on either field omits
/// `--model` for that phase.
pub(crate) struct ResolvedCopilot {
    pub(crate) plan_model: Option<String>,
    pub(crate) exec_model: Option<String>,
}

/// Resolve the two Copilot per-phase model overrides (ADR-0041 D4). Each phase
/// resolves independently through [`config::resolve_optional_model`]: the
/// phase's own flag, then the persisted `copilot.plan_model`/`copilot.exec_model`,
/// then `None` (omit `--model`, the account's own default).
pub(crate) fn resolve_copilot(
    plan_flag: Option<String>,
    exec_flag: Option<String>,
    persisted: &ralphy_agent_copilot::CopilotSettings,
) -> ResolvedCopilot {
    ResolvedCopilot {
        plan_model: config::resolve_optional_model(plan_flag, persisted.plan_model.clone()),
        exec_model: config::resolve_optional_model(exec_flag, persisted.exec_model.clone()),
    }
}

/// Build the run's issue queue and the explicitly-named ("forced") issue set. Two
/// paths:
///   `--issues`: an explicit, ordered selection — fetch each number directly
///     (label-agnostic, no dependency re-ordering) and run the list AS GIVEN, so
///     the run drains it as a sequence. This bypasses the label question entirely.
///   default: the label-built queue, optionally narrowed by `--only-issue`, then
///     ordered by dependency.
/// The forced set is `--issues 5,3,9` verbatim, or the single `--only-issue N`
/// folded into a one-element list, or empty for the ordinary label queue — handed
/// to the core so `stop-before` is ignored on exactly these issues (parity with the
/// ps1 `$OnlyIssue` guard, generalized to a set). No lifecycle side effects
/// (env/threads), so it lives outside the orchestrator's ordering.
pub(crate) fn build_run_queue(
    args: &RunArgs,
    assignee: Option<&str>,
    effective_labels: &[String],
    repo_root: &std::path::Path,
) -> Result<(Vec<ralphy_core::Issue>, Vec<u64>)> {
    let forced_issues: Vec<u64> = if !args.issues.is_empty() {
        args.issues.clone()
    } else {
        args.only_issue.into_iter().collect()
    };

    let queue = if !args.issues.is_empty() {
        let mut selected = Vec::with_capacity(args.issues.len());
        for number in &args.issues {
            selected.push(github::fetch_issue(*number, repo_root)?);
        }
        selected
    } else {
        // `--only-issue` fetches its single target unfiltered (criterion 5), so drop
        // the assignee filter on that path; a bare label queue applies it.
        let queue_assignee = if args.only_issue.is_some() {
            None
        } else {
            assignee
        };
        let mut queue = github::list_queue(effective_labels, queue_assignee, repo_root)?;
        if let Some(only) = args.only_issue {
            queue.retain(|i| i.number == only);
        }
        // Order by dependency (Blocked-by edges + split-bundle children), ascending
        // number as tie-break — the pending list shown to the user IS the sequence
        // run_queue will work, and a dependency-consistent order lets one run drain
        // a graph whose numbering disagrees with its edges.
        //
        // Fetch the full open-issue set so a blocker that sits OUTSIDE the queue but is
        // itself open (a partially-labelled chain) still orders the queue: edges are
        // walked transitively through those out-of-queue nodes. Best-effort — on a `gh`
        // failure fall back to in-queue-only ordering rather than abort the run. Skip
        // the extra call when ordering can't matter (0 or 1 issue).
        if queue.len() > 1 {
            match github::list_open_issues(repo_root) {
                Ok(open) => ralphy_core::blocked::sort_queue_in_graph(queue, &open),
                Err(e) => {
                    warn!(error = %e, "could not list open issues for dependency ordering; using in-queue edges only");
                    ralphy_core::blocked::sort_queue(queue)
                }
            }
        } else {
            ralphy_core::blocked::sort_queue(queue)
        }
    };

    Ok((queue, forced_issues))
}

/// Build a fully-configured adapter for one `CliAgent`, boxed as `&dyn Agent`.
/// Centralizes the per-vendor construction the composition root needs once for
/// the executor and (only in a split run) once for the planner — so `--plan-agent`
/// can wire two adapters without duplicating the match. The `String`/`Option`
/// config values are cloned per call so the same `RunArgs` can back both builds.
#[allow(clippy::too_many_arguments)]
pub(crate) fn build_agent(
    which: CliAgent,
    args: &RunArgs,
    run_dir: PathBuf,
    run_deadline: Option<std::time::Instant>,
    persisted_opencode_model: Option<String>,
    claude: &ResolvedClaude,
    copilot: &ResolvedCopilot,
    idle_minutes: Option<u64>,
) -> Box<dyn Agent> {
    // The headless adapters drive one child shape, so they resolve the idle
    // window here; the Claude adapter picks per execution path (its interactive
    // session has a coarser progress signal) and so keeps the `Option`.
    let headless_idle = idle_minutes.unwrap_or(ralphy_core::DEFAULT_IDLE_MINUTES);
    match which {
        CliAgent::Claude => Box::new(
            ClaudeAgent::new(
                non_empty(claude.plan_model.clone()),
                non_empty(claude.plan_effort.clone()),
                run_dir,
            )
            .with_exec_config(
                non_empty(args.exec_model.clone().unwrap_or_default()),
                non_empty(claude.exec_effort.clone()),
                claude.default_exec_model.clone(),
                claude.max_minutes_per_issue,
                claude.remote_control,
                args.headless_exec,
                args.max_exec_calls,
            )
            .with_run_deadline(run_deadline)
            .with_idle_minutes(idle_minutes),
        ),
        CliAgent::Codex => Box::new(
            CodexAgent::new(
                non_empty(args.exec_model.clone().unwrap_or_default()),
                run_dir,
            )
            .with_run_deadline(run_deadline)
            .with_max_minutes_per_issue(claude.max_minutes_per_issue)
            .with_idle_minutes(headless_idle),
        ),
        CliAgent::Copilot => Box::new(
            CopilotAgent::new(copilot.exec_model.clone(), run_dir)
                .with_plan_model(copilot.plan_model.clone())
                .with_run_deadline(run_deadline)
                .with_max_minutes_per_issue(claude.max_minutes_per_issue)
                .with_idle_minutes(headless_idle),
        ),
        CliAgent::Kimi => Box::new(
            KimiAgent::new(
                non_empty(args.exec_model.clone().unwrap_or_default()),
                run_dir,
            )
            .with_run_deadline(run_deadline)
            .with_max_minutes_per_issue(claude.max_minutes_per_issue)
            .with_idle_minutes(headless_idle),
        ),
        CliAgent::OpenCode => Box::new(
            OpenCodeAgent::new(
                config::resolve_opencode_model(args.exec_model.clone(), persisted_opencode_model),
                run_dir,
            )
            .with_variant(non_empty(args.exec_variant.clone().unwrap_or_default()))
            .with_run_deadline(run_deadline)
            .with_max_minutes_per_issue(claude.max_minutes_per_issue)
            .with_idle_minutes(headless_idle),
        ),
    }
}

/// Resolve which adapter plans: the explicit `--plan-agent`, or `--agent` when
/// the flag is omitted. An absent flag MUST equal `--agent` so a single-agent run
/// is unchanged (docs/adr/0009).
pub(crate) fn resolve_plan_agent(plan_agent: Option<CliAgent>, agent: CliAgent) -> CliAgent {
    plan_agent.unwrap_or(agent)
}

/// Remove `RALPHY_EVENTS_TOKEN` from the process environment so no spawned child
/// (adapter/agent) inherits the sink's bearer token (ADR-0019). Called once at boot
/// after the effective token is resolved and captured by the sink transport — the
/// run keeps using it, children never see it. Mirrors the `ANTHROPIC_API_KEY` scrub.
pub(crate) fn strip_events_token_from_env() {
    std::env::remove_var(events::config::TOKEN_ENV);
}

/// The operating run branch commits land on, for the `data.git.branch` block
/// (ADR-0019 amendment #96): a fresh `afk/run-<stamp>` in `new` mode (matching the
/// `afk/run-{stamp}` format the runner cuts), or the current branch in `current`
/// mode (empty when the current branch could not be resolved). Resolved before the
/// events ctx so `data.git` is constant from the first event.
pub(crate) fn operating_branch(
    mode: BranchMode,
    stamp: &str,
    start_branch: Option<&str>,
) -> String {
    match mode {
        BranchMode::New => format!("afk/run-{stamp}"),
        BranchMode::Current => start_branch.unwrap_or_default().to_string(),
    }
}

/// Pure predicate layer: returns `Err(message)` for the first agent whose
/// `cli_name()` the `locate` closure reports absent, else `Ok(())`. The
/// `locate` indirection lets unit tests inject a fake resolver with no PATH
/// dependency.
pub(crate) fn check_agents_present(
    executor: CliAgent,
    planner: CliAgent,
    locate: impl Fn(&str) -> bool,
) -> Result<(), String> {
    for which in [executor, planner] {
        let cli = which.cli_name();
        if !locate(cli) {
            return Err(format!(
                "the `{cli}` CLI was not found on PATH, PATHEXT, or ~/.local/bin. \
                Install it, or select another agent with --agent / --plan-agent."
            ));
        }
    }
    Ok(())
}

/// Thin wrapper that wires `check_agents_present` to the real `locate_program`
/// resolver and maps the string error into `anyhow`.
pub(crate) fn preflight_agents(executor: CliAgent, planner: CliAgent) -> Result<()> {
    check_agents_present(executor, planner, |n| {
        ralphy_adapter_support::locate_program(n).is_some()
    })
    .map_err(|e| anyhow::anyhow!(e))
}

/// Install the tracing stack. The full structured log always goes to the run's
/// `ralphy.log` (no colour, local timestamps). On screen, the animated presenter
/// (ADR-0006) renders the run's lifecycle by default; `--verbose` (or a set
/// `RUST_LOG`/`RALPHY_LOG`) drops to raw INFO `fmt` lines and disables animation
/// so debugging is unobstructed.
///
/// Local timestamps everywhere fix the reported UTC-vs-local bug at the source:
/// the `fmt` layers use `ChronoLocal`, and the presenter composes its own local
/// timestamps via `chrono::Local`.
///
/// Always returns a `PresenterHandle` — `plain()` on the raw-stderr path (no colour,
/// no bars, no-op `finalize`), or the animated presenter's handle otherwise. The
/// caller routes the early-exit notice and the final panel through it uniformly.
pub(crate) fn init_tracing(
    log_file: Option<std::fs::File>,
    verbose: bool,
    notifier: Option<delivery::DeliveryLayer>,
    events: Option<delivery::DeliveryLayer>,
) -> ui::PresenterHandle {
    use tracing_subscriber::fmt::time::ChronoLocal;
    use tracing_subscriber::{fmt, prelude::*, EnvFilter};

    // The run-border notices are FOLDED from `ralphy_core::emit` events now (#222),
    // where they used to be unconditional `println!`s. A narrow `RUST_LOG` (say
    // `warn`) must not silence the operator-facing notice, so that target is pinned
    // at INFO regardless of the env filter.
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info"))
        .add_directive(
            "ralphy_core::emit=info"
                .parse()
                .expect("a static, valid filter directive"),
        );

    // `--verbose`, or any explicit env filter, means the operator wants the raw
    // log on screen — drop the presenter and disable animation (ADR-0006 D3).
    let raw_stderr = verbose
        || std::env::var_os("RUST_LOG").is_some()
        || std::env::var_os("RALPHY_LOG").is_some();

    let timer = ChronoLocal::new("%Y-%m-%d %H:%M:%S".to_string());

    // `ralphy.log` always carries the full uncoloured, local-time log.
    let file_layer = log_file.map(|file| {
        fmt::layer()
            .with_ansi(false)
            .with_timer(timer.clone())
            .with_writer(move || file.try_clone().expect("clone ralphy.log handle"))
    });

    // The notifier Layer (when installed) composes alongside the file/presenter
    // layers so it sees every consumed event; `Option<Layer>` is a no-op when None.
    // The run-border notice fold (#222) is installed on BOTH console paths — the
    // presenter is dropped entirely under `--verbose`/`RUST_LOG`, so folding inside
    // it would silently lose the notice on the raw-stderr path.
    let edge = std::sync::Arc::new(std::sync::Mutex::new(ui::EdgeNoticeState::default()));

    let registry = tracing_subscriber::registry()
        .with(filter)
        .with(file_layer)
        .with(notifier)
        .with(events)
        .with(ui::EdgeNoticeLayer::new(std::sync::Arc::clone(&edge)));

    if raw_stderr {
        let stderr_layer = fmt::layer().with_timer(timer).with_writer(std::io::stderr);
        registry.with(stderr_layer).init();
        ui::PresenterHandle::plain().with_edge(edge)
    } else {
        let presenter = ui::Presenter::new();
        let handle = presenter.handle().with_edge(edge);
        registry.with(presenter).init();
        handle
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_events_token_removes_env_var() {
        // Guard the process-global env var against the other events-store tests.
        let _g = events::config::ENV_LOCK.lock().unwrap();
        std::env::set_var(events::config::TOKEN_ENV, "sekret");
        assert!(std::env::var(events::config::TOKEN_ENV).is_ok());
        strip_events_token_from_env();
        assert!(
            std::env::var(events::config::TOKEN_ENV).is_err(),
            "token must be absent after strip"
        );
    }

    #[test]
    fn operating_branch_derives_per_mode() {
        // `new` mode cuts a fresh `afk/run-<stamp>` regardless of the current branch.
        assert_eq!(
            operating_branch(BranchMode::New, "20260703-120000", Some("feature")),
            "afk/run-20260703-120000"
        );
        // `current` mode reports the current branch verbatim.
        assert_eq!(
            operating_branch(BranchMode::Current, "20260703-120000", Some("feature")),
            "feature"
        );
        // `current` mode with no resolvable current branch degrades to empty.
        assert_eq!(
            operating_branch(BranchMode::Current, "20260703-120000", None),
            ""
        );
    }

    #[test]
    fn plan_agent_defaults_to_the_executor_when_omitted() {
        // Omitted `--plan-agent` resolves to `--agent`, keeping single-agent runs
        // unchanged; an explicit flag overrides it (any combination allowed).
        assert_eq!(
            resolve_plan_agent(None, CliAgent::Claude),
            CliAgent::Claude,
            "absent flag equals --agent"
        );
        assert_eq!(
            resolve_plan_agent(Some(CliAgent::Claude), CliAgent::OpenCode),
            CliAgent::Claude,
            "explicit --plan-agent overrides --agent"
        );
    }

    #[test]
    fn check_agents_present_aborts_when_executor_absent() {
        let result = check_agents_present(CliAgent::Claude, CliAgent::Claude, |_| false);
        let err = result.unwrap_err();
        assert!(
            err.contains("claude"),
            "message must name the missing cli: {err}"
        );
        assert!(
            err.contains("--agent"),
            "message must mention --agent: {err}"
        );
        assert!(
            err.contains("--plan-agent"),
            "message must mention --plan-agent: {err}"
        );
    }

    #[test]
    fn check_agents_present_gates_planner() {
        // executor (Claude) is present; planner (Codex) is absent → Err naming codex.
        let result = check_agents_present(CliAgent::Claude, CliAgent::Codex, |n| n == "claude");
        let err = result.unwrap_err();
        assert!(
            err.contains("codex"),
            "message must name the absent planner: {err}"
        );
    }

    #[test]
    fn check_agents_present_ok_when_all_present() {
        let result = check_agents_present(CliAgent::Claude, CliAgent::Codex, |_| true);
        assert!(result.is_ok());
    }

    #[test]
    fn resolve_copilot_flag_wins() {
        let persisted = ralphy_agent_copilot::CopilotSettings {
            plan_model: Some("persisted".into()),
            ..Default::default()
        };
        let resolved = resolve_copilot(Some("flag".into()), None, &persisted);
        assert_eq!(resolved.plan_model, Some("flag".into()));
    }

    #[test]
    fn resolve_copilot_uses_persisted_when_flag_absent() {
        let persisted = ralphy_agent_copilot::CopilotSettings {
            plan_model: Some("persisted".into()),
            ..Default::default()
        };
        let resolved = resolve_copilot(None, None, &persisted);
        assert_eq!(resolved.plan_model, Some("persisted".into()));
    }

    #[test]
    fn resolve_copilot_none_when_both_unset() {
        let resolved = resolve_copilot(
            None,
            None,
            &ralphy_agent_copilot::CopilotSettings::default(),
        );
        assert_eq!(resolved.plan_model, None);
        assert_eq!(resolved.exec_model, None);
    }

    #[test]
    fn resolve_copilot_maps_flags_per_phase() {
        let resolved = resolve_copilot(
            Some("p".into()),
            Some("e".into()),
            &ralphy_agent_copilot::CopilotSettings::default(),
        );
        assert_eq!(resolved.plan_model, Some("p".into()));
        assert_eq!(resolved.exec_model, Some("e".into()));
    }
}
