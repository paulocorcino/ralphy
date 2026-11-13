//! The `ralphy run` orchestrator: `run_cmd` drives the whole run lifecycle and keeps
//! its ordering-sensitive side effects (env scrubs, presenter finalize, teardown).

use std::path::PathBuf;

use anyhow::Result;
use ralphy_core::{git, github, run_queue, Agent, GhTracker, QueueConfig, WallClock, Workspace};
use tracing::info;

use crate::cli::{CliAgent, RunArgs};
use crate::{config, events, runlock, split_agent, telegram, ui};

mod derived;
mod lifecycle;
mod prepare;
mod queue_built;
// `pub(crate)` only so `runstate::capture`'s #[cfg(test)] pins can drive the real
// `emit_run_finished` (#219). Bin crate: no public surface is widened.
pub(crate) mod report;
mod snapshot_engine;
pub(crate) mod summary;
mod wiring;

use lifecycle::{
    finalize_run, finish_border, install_observability, start_delivery, Observability,
};
use prepare::{
    branch_mode_word, defer_to_live_run, load_settings, resolve_branch_mode, resolve_claude,
    run_title, snapshot_ctx, verify_fallback, verify_timeout, warn_on_lock, AdapterSettings,
};

use derived::prune_old_run_folders;
use queue_built::emit_queue_built;
use report::{empty_queue_scope, render_final_panel};
pub(crate) use wiring::strip_secret_tokens_from_env;
use wiring::{
    build_agent, build_run_queue, operating_branch, preflight_agents, resolve_plan_agent,
    ResolvedEffort,
};

pub(crate) fn run_cmd(args: &RunArgs) -> Result<()> {
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
    let obs = install_observability(log_file, args, &repo_root);

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
            let title = telegram::notifier::derive_title(
                repo_name,
                0,
                &effective_labels,
                None,
                args.title.as_deref(),
            );
            defer_to_live_run(&obs, info, &title, &ws, &runid);
            return Ok(());
        }
        lock @ (runlock::LockState::HeldAlive(_)
        | runlock::LockState::Stale(_)
        | runlock::LockState::Corrupt
        | runlock::LockState::Free) => warn_on_lock(&lock),
    }
    // Held for the rest of run_cmd; Drop removes the file on every exit path.
    let _run_lock = runlock::acquire(&ws.run_lock_path())?;
    // CROSS-PATH INVARIANT (ADR-0047 §8): bound right after the run lock, so on EVERY exit
    // path it drops AFTER the worker handles shut down. A worker detached by the 5s shutdown
    // bound (`delivery.rs`) may still write after the drop; the reader's dead-pid sweep
    // deletes that orphan.
    let _snapshot_guard = snapshot_engine::snapshot_guard(&repo_root, &runid, &run_dir);
    let boot = Boot {
        run_start,
        plan_agent,
        obs: &obs,
        repo_name,
        effective_labels,
        ws: &ws,
        run_dir,
        stamp,
        runid: &runid,
    };
    run_locked(args, boot)
}

/// What `run_cmd` holds when the run lock is taken.
struct Boot<'a> {
    run_start: std::time::Instant,
    plan_agent: CliAgent,
    obs: &'a Observability,
    repo_name: &'a str,
    effective_labels: Vec<String>,
    ws: &'a Workspace,
    run_dir: PathBuf,
    stamp: String,
    runid: &'a str,
}

/// The run under its lock: build and announce the queue, start the delivery
/// workers, then hand the queue to [`work_the_queue`].
fn run_locked(args: &RunArgs, boot: Boot<'_>) -> Result<()> {
    let (obs, ws, labels) = (boot.obs, boot.ws, &boot.effective_labels);
    let repo_root = ws.repo_root();
    // Loaded once (ADR-0010) BEFORE the queue build: `queue.assignee` filters the label
    // queue. Each knob: flag > settings.json > default.
    let settings = load_settings(ws);
    prune_old_run_folders(ws, &settings, &boot.run_dir, args.dry_run);
    // The assignee filter of the label queue (ADR-0021): flag > `--no-assignee` > persisted
    // `queue.assignee` > none. `--only-issue`/`--issues` bypass it.
    let assignee = config::resolve_assignee(
        args.assignee.as_deref(),
        args.no_assignee,
        settings.queue.assignee.as_deref(),
    );

    // The queue and the explicitly named ("forced") issue set; see `build_run_queue`.
    let (queue, forced_issues) = build_run_queue(args, assignee.as_deref(), labels, repo_root)?;
    let title = run_title(args, boot.repo_name, &queue, &forced_issues, labels);

    // Header and info line. Each info-line part is best-effort: a detached HEAD or a
    // local-only repo drops it.
    obs.presenter.print_header(&title);
    let start_branch = git::current_branch(repo_root).ok();
    let repo_url = git::origin_url(repo_root).map(|u| ui::normalize_remote_url(&u));
    let (name, branch) = (boot.repo_name, start_branch.as_deref());
    obs.presenter
        .print_info_line(name, branch, repo_url.as_deref());

    // An empty queue does NOT return here (#222): it rides the full triad (`queue.built`
    // 0, `run.started`, `run.finished` no_work) and exits below.
    let order: Vec<String> = queue.iter().map(|i| format!("#{}", i.number)).collect();
    // Where the run halts: the first `stop-before` issue in the sorted order (0 = none; an
    // explicit selection overrides the label). The runner shares `first_stop_before`, so the
    // pending bar's marker and the runner's gate cannot disagree.
    let stop_before = ralphy_core::first_stop_before(&queue, &forced_issues).unwrap_or(0);

    // Resolved once (ADR-0016) for the queue snapshot and the `gh`-free core.
    let human_return_labels = github::resolve_human_return_labels(repo_root);

    // After the header prints and before the notifier worker start, so the ring drain
    // order is unchanged.
    let scope = empty_queue_scope(&args.issues, args.only_issue, labels, assignee.as_deref());
    emit_queue_built(
        &queue,
        &forced_issues,
        &human_return_labels,
        repo_root,
        args,
        assignee.as_deref(),
        &order,
        stop_before,
        &scope,
    );

    let adapters = AdapterSettings::load(&settings);
    let base_branch = config::resolve_str(
        args.base_branch.clone(),
        settings.base_branch.clone(),
        "origin/main",
    );
    let branch_mode = resolve_branch_mode(args, &settings);
    // Constant from the first event (ADR-0019 amendment #96).
    let operating_branch = operating_branch(branch_mode, &boot.stamp, start_branch.as_deref());

    // Above `start_delivery`: a `?` below the worker start would skip `shutdown()` and let
    // the snapshot worker re-publish after `_snapshot_guard` removed the document.
    let resolved_effort = ResolvedEffort {
        plan: config::resolve_effort(args.plan_effort, adapters.claude.plan_effort.clone(), None)?,
        exec: config::resolve_effort(args.exec_effort, adapters.claude.exec_effort.clone(), None)?,
    };

    // Events before this point (`queue built`) wait in the rings and drain on start.
    let snapshot_ctx = snapshot_ctx(boot.runid, obs, &operating_branch, ws);
    let (len, runid) = (queue.len(), boot.runid);
    let workers = start_delivery(
        obs,
        &title,
        len,
        &operating_branch,
        repo_root,
        ws,
        runid,
        Some(snapshot_ctx),
    );

    // `run.started` (ADR-0019) carries the CLI-only run parameters the core never sees. It
    // follows `queue.built` (docs/events.md).
    ralphy_core::emit::run_started(
        &obs.events_slug,
        &labels.join(","),
        args.agent.cli_name(),
        boot.plan_agent.cli_name(),
        branch_mode_word(branch_mode),
        &base_branch,
        args.deadline_hours.unwrap_or(0.0),
    );

    // The empty-queue border (#222): the full triad, no work. NOT `finalize_run`: that would
    // spawn a paid consolidation session. Both `shutdown()`s run here, after the emit.
    if queue.is_empty() {
        report::emit_run_finished_no_work(boot.run_start);
        let (notifier, events_handle, snapshot_handle) = workers;
        finish_border(&obs.presenter, notifier, events_handle, snapshot_handle);
        return Ok(());
    }

    let cfg = QueueConfig {
        repo_root: repo_root.to_path_buf(),
        base_branch,
        dry_run: args.dry_run,
        stamp: boot.stamp,
        branch_mode,
        forced_issues,
        human_return_labels,
        // Every agent auto-resumes on a usage limit (ADR-0030); `--stop-on-limit` stops and
        // reports instead.
        stop_on_limit_plan: args.stop_on_limit,
        stop_on_limit_exec: args.stop_on_limit,
        verify_fallback: verify_fallback(&settings),
        verify_timeout: verify_timeout(&settings),
        // ADR-0015: a `Done` with no verify gate is parked for a human, not closed.
        require_verify_gate: settings.verify.require_verify_gate.unwrap_or(false),
        // Named once at the adapter layer; the core gets it as data (ADR-0002 amendment, #79).
        done_signal: ralphy_adapter_support::DONE_SENTINEL.to_owned(),
    };
    let work = Work {
        run_start: boot.run_start,
        plan_agent: boot.plan_agent,
        obs,
        ws,
        run_dir: boot.run_dir,
        settings: &settings,
        adapters,
        resolved_effort,
        queue,
        cfg,
    };
    work_the_queue(args, &work, workers)
}

/// What the queue work takes from [`run_locked`].
struct Work<'a> {
    run_start: std::time::Instant,
    plan_agent: CliAgent,
    obs: &'a Observability,
    ws: &'a Workspace,
    run_dir: PathBuf,
    settings: &'a ralphy_core::Settings,
    adapters: AdapterSettings,
    resolved_effort: ResolvedEffort,
    queue: Vec<ralphy_core::Issue>,
    cfg: QueueConfig,
}

/// The started delivery workers: the notifier, the events sink and the snapshot.
type Workers = (
    Option<telegram::notifier::NotifierHandle>,
    Option<events::sink::EventsHandle>,
    Option<crate::delivery::WorkerHandle>,
);

/// Build the agents, work the queue, then finalize and print the panel.
fn work_the_queue(args: &RunArgs, work: &Work<'_>, workers: Workers) -> Result<()> {
    let Work {
        settings,
        adapters,
        cfg,
        ..
    } = work;
    // The run's wall-clock deadline: it clamps each issue's budget and gates the next issue.
    let run_deadline = args
        .deadline_hours
        .map(|h| std::time::Instant::now() + std::time::Duration::from_secs_f64(h * 3600.0));
    let resolved_claude = resolve_claude(args, &adapters.claude, settings);
    let (plan_flag, exec_flag) = (&args.plan_model, &args.exec_model);
    let resolved_copilot =
        wiring::resolve_copilot(plan_flag.clone(), exec_flag.clone(), &adapters.copilot);
    let resolved_cursor =
        wiring::resolve_cursor(plan_flag.clone(), exec_flag.clone(), &adapters.cursor);
    let resolved_antigravity = wiring::resolve_antigravity(
        plan_flag.clone(),
        exec_flag.clone(),
        work.resolved_effort,
        &adapters.antigravity,
    );
    // Stays an `Option`: absent means each execution path uses the default its progress
    // signal supports (docs/adr/0038); `Some(0)` opts out.
    let idle_minutes = args.idle_minutes.or(settings.idle_minutes);
    let resolved_effort = work.resolved_effort;
    let persisted_opencode_model = adapters.opencode.model.clone();
    // Boxed as `&dyn Agent` (docs/adr/0004). When `--plan-agent` equals `--agent` the executor
    // goes to the core unwrapped; otherwise a `SplitAgent` routes plan to the planner and
    // execute to the executor (docs/adr/0009).
    let executor = build_agent(
        args.agent,
        args,
        work.run_dir.clone(),
        run_deadline,
        persisted_opencode_model.clone(),
        &resolved_claude,
        resolved_effort,
        &resolved_copilot,
        &resolved_cursor,
        &resolved_antigravity,
        idle_minutes,
    );
    let agent: Box<dyn Agent> = if work.plan_agent == args.agent {
        executor
    } else {
        Box::new(split_agent::SplitAgent {
            planner: build_agent(
                work.plan_agent,
                args,
                work.run_dir.clone(),
                run_deadline,
                persisted_opencode_model,
                &resolved_claude,
                resolved_effort,
                &resolved_copilot,
                &resolved_cursor,
                &resolved_antigravity,
                idle_minutes,
            ),
            executor,
        })
    };

    // The same deadline gates starting the next issue (between-issue clock).
    let clock = WallClock {
        deadline: run_deadline,
    };
    // Drops non-collaborator comments unless the operator opted out (security audit
    // 2026-09-21, F8).
    let tracker = GhTracker::new(cfg.repo_root.clone())
        .with_comment_trust(settings.queue.trust_all_comments.unwrap_or(false));

    let result = run_queue(cfg, &work.queue, agent.as_ref(), &tracker, &clock);

    // ONE fold of the report, read by both `run.finished` and the final panel.
    let queue_len = work.queue.len();
    let summary = result
        .as_ref()
        .ok()
        .map(|r| summary::RunSummary::from_report(r, queue_len));

    // Finalize the presenter, consolidate, emit `run finished`, tear down the notifier then
    // the sink, in this order (ADR-0006/-0007/-0019). Before the `?`: a non-green result
    // still finalizes.
    let presenter = &work.obs.presenter;
    let (notifier, events_handle, snapshot_handle) = workers;
    let consolidation_usage = finalize_run(
        args.agent,
        presenter,
        &result,
        args.dry_run,
        work.ws,
        &cfg.stamp,
        summary.as_ref(),
        work.run_start,
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
        cfg.branch_mode,
        args.dry_run,
        &cfg.repo_root,
        &consolidation_usage,
    );
    Ok(())
}

#[cfg(test)]
mod tests;
