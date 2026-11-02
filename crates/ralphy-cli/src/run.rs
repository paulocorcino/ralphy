//! The `ralphy run` orchestrator: `run_cmd` drives the whole run lifecycle and keeps
//! its ordering-sensitive side effects (env scrubs, presenter finalize, teardown).

use anyhow::Result;
use ralphy_core::{
    git, github, run_queue, Agent, BranchMode, GhTracker, QueueConfig, WallClock, Workspace,
};
use tracing::{info, warn};

use crate::cli::RunArgs;
use crate::{config, events, runlock, runstate, split_agent, telegram, ui};

mod derived;
mod lifecycle;
mod queue_built;
// `pub(crate)` only so `runstate::capture`'s #[cfg(test)] pins can drive the real
// `emit_run_finished` (#219). Bin crate: no public surface is widened.
pub(crate) mod report;
mod snapshot_engine;
pub(crate) mod summary;
mod wiring;

use lifecycle::{
    finalize_run, finish_border, finish_if_idle, install_observability, start_delivery,
};

use derived::{prune_old_run_folders, repo_relative_plan_path, resolve_verify_timeout_minutes};
use queue_built::emit_queue_built;
use report::{empty_queue_scope, render_final_panel};
pub(crate) use wiring::strip_secret_tokens_from_env;
use wiring::{
    build_agent, build_run_queue, operating_branch, preflight_agents, resolve_plan_agent,
    ResolvedClaude, ResolvedEffort,
};

pub(crate) fn run_cmd(args: RunArgs) -> Result<()> {
    // Anchors the `run.finished` `duration_s` (ADR-0019) — the run's wall-clock.
    let run_start = std::time::Instant::now();

    // Clear any inherited ANTHROPIC_API_KEY so the agent draws on the subscription quota.
    // `""`, not `remove_var`: the PowerShell oracle assigns `""`, and how Claude treats an
    // absent key is not verified.
    // Edition 2024 makes `set_var` unsafe: this call then needs an `unsafe` block.
    #[expect(
        clippy::disallowed_methods,
        reason = "this is the first statement of the run on the main thread, so no \
                  other thread exists to read the environment while it changes"
    )]
    std::env::set_var("ANTHROPIC_API_KEY", "");

    let repo_root = git::resolve_toplevel(&args.repo)?;
    crate::daemon::register_repo(&repo_root);
    let plan_agent = resolve_plan_agent(args.plan_agent, args.agent);
    preflight_agents(args.agent, plan_agent)?;
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
    // Minted here: the run snapshot is keyed by it and published unconditionally (ADR-0047 §4).
    let runid = events::emitter::new_runid();
    let ws = Workspace::new(&repo_root);
    let run_dir = ws.run_dir(&stamp);
    #[expect(
        clippy::unused_result_ok,
        reason = "the run goes on without a log file, and creating that file fails \
                  the same way"
    )]
    std::fs::create_dir_all(&run_dir).ok();

    let log_file = std::fs::File::create(run_dir.join("ralphy.log")).ok();

    // The notifier and sink rings/Layers, the events-token env scrub (BEFORE any worker
    // thread) and the tracing subscriber. `obs` stays whole so `start_delivery` is the ONE
    // place a delivery worker is spawned; a `?` bail above it returns with the rings unstarted.
    let obs = install_observability(log_file, &args, &repo_root);
    let presenter = &obs.presenter;
    let events_slug = obs.events_slug.clone();

    // The repo name feeds the run title, which seeds the header face and the Telegram card.
    let repo_name = repo_root
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("repo");

    info!(repo = %repo_root.display(), %stamp, dry_run = args.dry_run, "ralphy run");

    #[expect(
        clippy::unused_result_ok,
        reason = "every write into this folder reports its own error"
    )]
    std::fs::create_dir_all(ws.ralphy_dir()).ok();

    // Above the lock check: the `--if-idle` deferral needs it for its title. A pure read.
    let effective_labels = github::resolve_queue_labels(&args.queue_label, &repo_root);

    // Presence lock (issue #72): `--if-idle` defers to a live run with exit 0; without it a
    // live lock only warns. A stale lock (dead PID) is taken over.
    match runlock::inspect(&ws.run_lock_path(), runlock::pid_is_alive) {
        runlock::LockState::HeldAlive(info) if args.if_idle => {
            let since = chrono::DateTime::parse_from_rfc3339(&info.started_at)
                .map(|t| t.format("%Y-%m-%d %H:%M:%S").to_string())
                .unwrap_or(info.started_at);
            let msg = format!("skipped: run in progress since {since}, pid {}", info.pid);
            // The deferral is a run BORDER (ADR-0019 amendment, #222): it rides the bus, and
            // both `shutdown()`s run AFTER the emit so the ring drains.
            let title = telegram::notifier::derive_title(
                repo_name,
                0,
                &effective_labels,
                None,
                args.title.as_deref(),
            );
            // No branch was cut: report the branch the repo is on.
            let (notifier, events_handle, snapshot_handle) = start_delivery(
                &obs,
                &title,
                0,
                &git::current_branch(&repo_root).unwrap_or_default(),
                &repo_root,
                &ws,
                &runid,
                None,
            );
            return finish_if_idle(presenter, &msg, notifier, events_handle, snapshot_handle);
        }
        runlock::LockState::HeldAlive(info) => {
            warn!(
                pid = info.pid,
                since = %info.started_at,
                "a run is already active in this repo — proceeding anyway"
            );
        }
        runlock::LockState::Stale(info) => {
            warn!(
                pid = info.pid,
                "ignoring stale run.lock (process not running)"
            );
        }
        runlock::LockState::Corrupt => warn!("ignoring unreadable run.lock"),
        runlock::LockState::Free => {}
    }
    // Held for the rest of run_cmd; Drop removes the file on every exit path.
    let _run_lock = runlock::acquire(&ws.run_lock_path())?;
    // CROSS-PATH INVARIANT (ADR-0047 §8): bound right after the run lock, so on EVERY exit
    // path it drops AFTER the worker handles shut down. A worker detached by the 5s shutdown
    // bound (`delivery.rs`) may still write after the drop; the reader's dead-pid sweep
    // deletes that orphan.
    let _snapshot_guard = snapshot_engine::snapshot_guard(&repo_root, &runid, &run_dir);

    // Loaded once (ADR-0010) BEFORE the queue build: `queue.assignee` filters the label
    // queue. A load failure warns and uses defaults. Each knob: flag > settings.json > default.
    let settings = match ralphy_core::Settings::load(&ws) {
        Ok(s) => s,
        Err(e) => {
            warn!(error = %e, "could not load .ralphy/settings.json — persisted defaults ignored");
            ralphy_core::Settings::default()
        }
    };
    prune_old_run_folders(&ws, &settings, &run_dir, args.dry_run);
    // The assignee filter of the label queue (ADR-0021): flag > `--no-assignee` > persisted
    // `queue.assignee` > none. `--only-issue`/`--issues` bypass it.
    let assignee = config::resolve_assignee(
        args.assignee.as_deref(),
        args.no_assignee,
        settings.queue.assignee.as_deref(),
    );

    // The queue and the explicitly named ("forced") issue set; see `build_run_queue`.
    let (queue, forced_issues) =
        build_run_queue(&args, assignee.as_deref(), &effective_labels, &repo_root)?;

    // The run title seeds both the console header and the Telegram card. A single named
    // issue titles the card with its own title; a longer list falls back to "N issues".
    let single_title = if forced_issues.len() == 1 {
        queue.first().map(|i| i.title.clone())
    } else {
        None
    };
    // An explicit `--issues` selection is not label-scoped: no default labels on the card.
    let title_labels: &[String] = if args.issues.is_empty() {
        &effective_labels
    } else {
        &[]
    };
    let title = telegram::notifier::derive_title(
        repo_name,
        queue.len(),
        title_labels,
        single_title.as_deref(),
        args.title.as_deref(),
    );

    // Header and info line. Each info-line part is best-effort: a detached HEAD or a
    // local-only repo drops it.
    presenter.print_header(&title);
    let start_branch = git::current_branch(&repo_root).ok();
    let repo_url = git::origin_url(&repo_root).map(|u| ui::normalize_remote_url(&u));
    presenter.print_info_line(repo_name, start_branch.as_deref(), repo_url.as_deref());

    // An empty queue does NOT return here (#222): it rides the full triad (`queue.built`
    // 0, `run.started`, `run.finished` no_work) and exits below.
    let order: Vec<String> = queue.iter().map(|i| format!("#{}", i.number)).collect();
    // Where the run halts: the first `stop-before` issue in the sorted order (0 = none; an
    // explicit selection overrides the label). The runner shares `first_stop_before`, so the
    // pending bar's marker and the runner's gate cannot disagree.
    let stop_before = ralphy_core::first_stop_before(&queue, &forced_issues).unwrap_or(0);

    // Resolved once (ADR-0016) for the queue snapshot and the `gh`-free core.
    let human_return_labels = github::resolve_human_return_labels(&repo_root);

    // After the header prints and before the notifier worker start, so the ring drain
    // order is unchanged.
    emit_queue_built(
        &queue,
        &forced_issues,
        &human_return_labels,
        &repo_root,
        &args,
        assignee.as_deref(),
        &order,
        stop_before,
        &empty_queue_scope(
            &args.issues,
            args.only_issue,
            &effective_labels,
            assignee.as_deref(),
        ),
    );

    // An adapter's settings section is opaque JSON to the core: deserialize the typed slices
    // here, warning and defaulting on a malformed section (ADR-0002 amendment).
    let claude_settings: ralphy_agent_claude::ClaudeSettings = settings
        .agent_settings(ralphy_agent_claude::ClaudeSettings::SECTION)
        .unwrap_or_else(|e| {
            warn!(error = %e, "malformed claude settings section — its persisted defaults ignored");
            Default::default()
        });
    let opencode_settings: ralphy_agent_opencode::OpenCodeSettings = settings
        .agent_settings(ralphy_agent_opencode::OpenCodeSettings::SECTION)
        .unwrap_or_else(|e| {
            warn!(
                error = %e,
                "malformed opencode settings section — its persisted defaults ignored"
            );
            Default::default()
        });
    let persisted_opencode_model = opencode_settings.model.clone();
    let copilot_settings: ralphy_agent_copilot::CopilotSettings = settings
        .agent_settings(ralphy_agent_copilot::CopilotSettings::SECTION)
        .unwrap_or_else(|e| {
            warn!(
                error = %e,
                "malformed copilot settings section — its persisted defaults ignored"
            );
            Default::default()
        });
    let cursor_settings: ralphy_agent_cursor::CursorSettings = settings
        .agent_settings(ralphy_agent_cursor::CursorSettings::SECTION)
        .unwrap_or_else(|e| {
            warn!(error = %e, "malformed cursor settings section — its persisted defaults ignored");
            Default::default()
        });
    let gemini_settings: ralphy_agent_gemini::GeminiSettings = settings
        .agent_settings(ralphy_agent_gemini::GeminiSettings::SECTION)
        .unwrap_or_else(|e| {
            warn!(error = %e, "malformed gemini settings section — its persisted defaults ignored");
            Default::default()
        });
    let base_branch = config::resolve_str(
        args.base_branch.clone(),
        settings.base_branch.clone(),
        "origin/main",
    );
    let branch_mode: BranchMode = args
        .branch_mode
        .map(BranchMode::from)
        .or_else(|| {
            settings
                .branch_mode
                .as_deref()
                .and_then(|m| config::parse_branch_mode(m).ok())
        })
        .unwrap_or(BranchMode::New);
    // Constant from the first event (ADR-0019 amendment #96).
    let operating_branch = operating_branch(branch_mode, &stamp, start_branch.as_deref());

    // Above `start_delivery`: a `?` below the worker start would skip `shutdown()` and let
    // the snapshot worker re-publish after `_snapshot_guard` removed the document.
    let resolved_effort = ResolvedEffort {
        plan: config::resolve_effort(args.plan_effort, claude_settings.plan_effort.clone(), None)?,
        exec: config::resolve_effort(args.exec_effort, claude_settings.exec_effort.clone(), None)?,
    };

    // Events before this point (`queue built`) wait in the rings and drain on start.
    let (notifier, events_handle, snapshot_handle) = start_delivery(
        &obs,
        &title,
        queue.len(),
        &operating_branch,
        &repo_root,
        &ws,
        &runid,
        Some(runstate::snapshot::SnapshotCtx {
            runid: runid.clone(),
            pid: std::process::id(),
            repo: obs.events_slug.clone(),
            branch: operating_branch.clone(),
            started_at: chrono::Local::now().to_rfc3339(),
            plan_path: repo_relative_plan_path(&repo_root, &ws),
        }),
    );

    // The run's wall-clock deadline: it clamps each issue's budget and gates the next issue.
    let run_deadline = args
        .deadline_hours
        .map(|h| std::time::Instant::now() + std::time::Duration::from_secs_f64(h * 3600.0));

    // `run.started` (ADR-0019) carries the CLI-only run parameters the core never sees. It
    // follows `queue.built` (docs/events.md).
    let branch_mode_str = match branch_mode {
        BranchMode::New => "new",
        BranchMode::Current => "current",
    };
    ralphy_core::emit::run_started(
        &events_slug,
        &effective_labels.join(","),
        args.agent.cli_name(),
        plan_agent.cli_name(),
        branch_mode_str,
        &base_branch,
        args.deadline_hours.unwrap_or(0.0),
    );

    // The empty-queue border (#222): the full triad, no work. NOT `finalize_run`: that would
    // spawn a paid consolidation session. Both `shutdown()`s run here, after the emit.
    if queue.is_empty() {
        report::emit_run_finished_no_work(run_start);
        finish_border(presenter, notifier, events_handle, snapshot_handle);
        return Ok(());
    }

    let resolved_claude = ResolvedClaude {
        plan_model: config::resolve_str(
            args.plan_model.clone(),
            claude_settings.plan_model.clone(),
            "opus",
        ),
        default_exec_model: config::resolve_str(
            args.default_exec_model.clone(),
            claude_settings.default_exec_model.clone(),
            "sonnet",
        ),
        max_minutes_per_issue: config::resolve_u64(
            args.max_minutes_per_issue,
            claude_settings.max_minutes_per_issue,
            ralphy_core::DEFAULT_MAX_MINUTES_PER_ISSUE,
        ),
        remote_control: config::resolve_remote_control(
            args.remote_control,
            args.no_remote_control,
            settings.remote_control,
        ),
        run_without_guard_proof: claude_settings.run_without_guard_proof_i_understand_the_risk,
    };
    let resolved_copilot = wiring::resolve_copilot(
        args.plan_model.clone(),
        args.exec_model.clone(),
        &copilot_settings,
    );
    let resolved_cursor = wiring::resolve_cursor(
        args.plan_model.clone(),
        args.exec_model.clone(),
        &cursor_settings,
    );
    let resolved_gemini = wiring::resolve_gemini(
        args.plan_model.clone(),
        args.exec_model.clone(),
        &gemini_settings,
    );
    // Stays an `Option`: absent means each execution path uses the default its progress
    // signal supports (docs/adr/0038); `Some(0)` opts out.
    let idle_minutes = args.idle_minutes.or(settings.idle_minutes);
    // Boxed as `&dyn Agent` (docs/adr/0004). When `--plan-agent` equals `--agent` the executor
    // goes to the core unwrapped; otherwise a `SplitAgent` routes plan to the planner and
    // execute to the executor (docs/adr/0009).
    let executor = build_agent(
        args.agent,
        &args,
        run_dir.clone(),
        run_deadline,
        persisted_opencode_model.clone(),
        &resolved_claude,
        &resolved_effort,
        &resolved_copilot,
        &resolved_cursor,
        &resolved_gemini,
        idle_minutes,
    );
    let agent: Box<dyn Agent> = if plan_agent == args.agent {
        executor
    } else {
        Box::new(split_agent::SplitAgent {
            planner: build_agent(
                plan_agent,
                &args,
                run_dir,
                run_deadline,
                persisted_opencode_model,
                &resolved_claude,
                &resolved_effort,
                &resolved_copilot,
                &resolved_cursor,
                &resolved_gemini,
                idle_minutes,
            ),
            executor,
        })
    };
    let cfg = QueueConfig {
        repo_root,
        base_branch,
        dry_run: args.dry_run,
        stamp,
        branch_mode,
        forced_issues,
        human_return_labels,
        // Every agent auto-resumes on a usage limit (ADR-0030); `--stop-on-limit` stops and
        // reports instead.
        stop_on_limit_plan: args.stop_on_limit,
        stop_on_limit_exec: args.stop_on_limit,
        // ADR-0011: the per-repo fallback command (one argv), used only when a plan has no
        // `## Verify` section.
        verify_fallback: settings
            .verify
            .command
            .as_deref()
            .map(str::trim)
            .filter(|c| !c.is_empty())
            .map(|c| vec![ralphy_core::verify::tokenize(c)]),
        // The gate owns its clock (docs/adr/0038): the per-issue cap is `0` by default.
        verify_timeout: std::time::Duration::from_secs(
            resolve_verify_timeout_minutes(settings.verify.timeout_minutes).saturating_mul(60),
        ),
        // ADR-0015: a `Done` with no verify gate is parked for a human, not closed.
        require_verify_gate: settings.verify.require_verify_gate.unwrap_or(false),
        // Named once at the adapter layer; the core gets it as data (ADR-0002 amendment, #79).
        done_signal: ralphy_adapter_support::DONE_SENTINEL.to_owned(),
    };

    // The same deadline gates starting the next issue (between-issue clock).
    let clock = WallClock {
        deadline: run_deadline,
    };
    // Drops non-collaborator comments unless the operator opted out (security audit
    // 2026-09-21, F8).
    let tracker = GhTracker::new(cfg.repo_root.clone())
        .with_comment_trust(settings.queue.trust_all_comments.unwrap_or(false));

    let result = run_queue(&cfg, &queue, agent.as_ref(), &tracker, &clock);

    // ONE fold of the report, read by both `run.finished` and the final panel.
    let summary = result
        .as_ref()
        .ok()
        .map(|r| summary::RunSummary::from_report(r, queue.len()));

    // Finalize the presenter, consolidate, emit `run finished`, tear down the notifier then
    // the sink, in this order (ADR-0006/-0007/-0019). Before the `?`: a non-green result
    // still finalizes.
    let consolidation_usage = finalize_run(
        args.agent,
        presenter,
        &result,
        args.dry_run,
        &ws,
        &cfg.stamp,
        summary.as_ref(),
        run_start,
        notifier,
        events_handle,
        snapshot_handle,
    );

    let report = result?;
    let summary = summary.expect("run_queue returned Ok, so the summary was built");

    render_final_panel(
        presenter,
        report,
        &summary,
        branch_mode,
        args.dry_run,
        &cfg.repo_root,
        &consolidation_usage,
    );
    Ok(())
}

#[cfg(test)]
mod tests;
