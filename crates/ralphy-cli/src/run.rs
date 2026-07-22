//! The `ralphy run` orchestrator: parse-resolved flags in hand, this drives the
//! whole run lifecycle — preflight the agents, build the queue, wire the notifier
//! and CloudEvents sink, resolve the per-run knobs, build the adapter(s), hand off
//! to the core queue loop, and render the final panel. `main.rs` stays the thin
//! composition root that dispatches here; the ordering-sensitive lifecycle side
//! effects (env scrubs, presenter finalize, teardown) live in `run_cmd` itself.

use std::sync::Arc;

use anyhow::Result;
use ralphy_core::{
    git, github, run_queue, Agent, BranchMode, GhTracker, QueueConfig, WallClock, Workspace,
};
use tracing::{info, warn};

use crate::cli::{CliAgent, RunArgs};
use crate::{config, events, runlock, runstate, split_agent, telegram, ui};

// `pub(crate)` only so `runstate::capture`'s #[cfg(test)] pins can drive the real
// `emit_run_finished` (#219). Bin crate: no public surface is widened.
pub(crate) mod report;
pub(crate) mod summary;
mod wiring;

use report::{
    emit_run_finished, empty_queue_scope, maybe_consolidate_knowledge, render_final_panel,
};
use wiring::{
    build_agent, build_run_queue, init_tracing, operating_branch, preflight_agents,
    resolve_plan_agent, strip_events_token_from_env, ResolvedClaude,
};

pub(crate) fn run_cmd(args: RunArgs) -> Result<()> {
    // Anchors the `run.finished` `duration_s` (ADR-0019) — the run's wall-clock.
    let run_start = std::time::Instant::now();
    let repo_root = git::resolve_toplevel(&args.repo)?;
    crate::daemon::register_repo(&repo_root);
    let plan_agent = resolve_plan_agent(args.plan_agent, args.agent);
    preflight_agents(args.agent, plan_agent)?;
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
    let ws = Workspace::new(&repo_root);
    let run_dir = ws.run_dir(&stamp);
    std::fs::create_dir_all(&run_dir).ok();

    let log_file = std::fs::File::create(run_dir.join("ralphy.log")).ok();

    // Wire the run's observability stack — notifier ring/Layer, CloudEvents sink
    // ring/Layer, the events-token env scrub, and the tracing subscriber — in one
    // place, returning the handles the later worker starts consume. The scrub runs
    // there BEFORE any worker thread, so the block stays ordering-safe.
    // Kept whole (rather than destructured) so `start_delivery` can be the ONE
    // place a delivery worker is spawned — every exit path AFTER it reaches a
    // started worker. (The `?` bails above it — lock acquire, queue build — still
    // return with the rings unstarted; those are error exits, not run borders.)
    let obs = install_observability(log_file, &args, &repo_root);
    let presenter = &obs.presenter;
    let events_slug = obs.events_slug.clone();

    // The repo name feeds the run title (below); the branding header is printed once
    // that title is known, so the console face is seeded by the same title as the
    // Telegram card — identical per run, varying across runs.
    let repo_name = repo_root
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("repo");

    info!(repo = %repo_root.display(), %stamp, dry_run = args.dry_run, "ralphy run");

    std::fs::create_dir_all(ws.ralphy_dir()).ok();

    // Resolved above the lock check because the `--if-idle` deferral needs it for its
    // run title: a pure per-repo config read (`github/labels.rs`), no side effect.
    let effective_labels = github::resolve_queue_labels(&args.queue_label, &repo_root);

    // Presence lock (issue #72): the concurrency policy lives in the invocation.
    // `--if-idle` defers to a live run (clean exit 0, so a scheduler's history
    // shows no false failures); without it a live lock only warns — intentional
    // concurrency stays the human's call. Stale locks (dead PID after a crash
    // or reboot) are taken over so a crash never silences a schedule.
    match runlock::inspect(&ws.run_lock_path(), runlock::pid_is_alive) {
        runlock::LockState::HeldAlive(info) if args.if_idle => {
            let since = chrono::DateTime::parse_from_rfc3339(&info.started_at)
                .map(|t| t.format("%Y-%m-%d %H:%M:%S").to_string())
                .unwrap_or(info.started_at);
            let msg = format!("skipped: run in progress since {since}, pid {}", info.pid);
            // The deferral is a run BORDER, not a silent return: it rides the bus so
            // every sink sees it (ADR-0019 amendment, #222). The workers start here
            // and both `shutdown()`s run on this path AFTER the emit, so the ring is
            // drained rather than discarded.
            let title = telegram::notifier::derive_title(
                repo_name,
                0,
                &effective_labels,
                None,
                args.title.as_deref(),
            );
            // The run never cut a branch, so report the branch the repo is actually
            // on — not the `afk/run-<stamp>` a real run would have created.
            let (notifier, events_handle) = start_delivery(
                &obs,
                &title,
                0,
                &git::current_branch(&repo_root).unwrap_or_default(),
                &repo_root,
                &ws,
            );
            return finish_if_idle(presenter, &msg, notifier, events_handle);
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

    // Load the persisted settings once here (ADR-0010) BEFORE the queue build so the
    // `queue.assignee` default is available to the label-queue path (and, further
    // down, the events ctx). A load failure warns and falls back to defaults so a
    // malformed settings file never aborts a run. Precedence for each knob: per-run
    // flag > settings.json > hardcoded default.
    let settings = match ralphy_core::Settings::load(&ws) {
        Ok(s) => s,
        Err(e) => {
            warn!(error = %e, "could not load .ralphy/settings.json — persisted defaults ignored");
            ralphy_core::Settings::default()
        }
    };
    // The effective assignee filter for the label-built queue (ADR-0021): flag >
    // `--no-assignee` > persisted `queue.assignee` > none. `--only-issue`/`--issues`
    // bypass it below so an explicit selection always fetches unfiltered.
    let assignee = config::resolve_assignee(
        args.assignee.as_deref(),
        args.no_assignee,
        settings.queue.assignee.as_deref(),
    );

    // Build the queue and the explicitly-named ("forced") issue set. Pure of the
    // lifecycle ordering (no env/thread side effects), so it lives outside the
    // orchestrator; see `build_run_queue` for the two-path selection + dependency sort.
    let (queue, forced_issues) =
        build_run_queue(&args, assignee.as_deref(), &effective_labels, &repo_root)?;

    // Derive the run title once, before any on-screen line, so it can seed both the
    // console branding header and the Telegram card — the face then matches across
    // both surfaces and varies per run (a different queue → a different face).
    // A single named issue (`--only-issue N`, or `--issues N` with one entry)
    // titles the card with that issue's own title; a multi-issue list falls back
    // to the "N issues" form.
    let single_title = if forced_issues.len() == 1 {
        queue.first().map(|i| i.title.clone())
    } else {
        None
    };
    // An explicit `--issues` selection isn't label-scoped, so don't tag the card
    // with the (unused) default labels.
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

    // Branding header + info line, seeded by the run title (see above). All info-line
    // segments are best-effort — a detached HEAD or a local-only repo drops that part.
    presenter.print_header(&title);
    let start_branch = git::current_branch(&repo_root).ok();
    let repo_url = git::origin_url(&repo_root).map(|u| ui::normalize_remote_url(&u));
    presenter.print_info_line(repo_name, start_branch.as_deref(), repo_url.as_deref());

    // NOTE: an empty queue does NOT return here (#222) — it rides the full triad
    // (`queue.built` count 0 → `run.started` → `run.finished` no_work) and exits
    // below, after the delivery workers have started and drained.
    let order: Vec<String> = queue.iter().map(|i| format!("#{}", i.number)).collect();
    // Where the run will halt: the first issue carrying `stop-before` in the sorted
    // order (0 = none). An explicit selection (`--only-issue`/`--issues`) overrides
    // the label, so the cut never applies there. Emitted so the pending bar can mark
    // the boundary up front (the run won't touch that issue or anything after it).
    // Mirrors the runner's gate in runner.rs.
    // The CLI shares the runner's exact predicate (`first_stop_before`) so the
    // boundary marker on the pending bar and the runner's own gate can never
    // disagree; `0` is the "no stop-before" sentinel the event decoder expects.
    let stop_before = ralphy_core::first_stop_before(&queue, &forced_issues).unwrap_or(0);

    // The human-return label set (ADR-0016): resolved once here and reused both for
    // the enriched queue snapshot below and the `gh`-free core (`QueueConfig`).
    let human_return_labels = github::resolve_human_return_labels(&repo_root);

    // Emit the `queue built` telemetry (ADR-0020/-0021): the enriched per-issue
    // snapshot and the applied assignee scope, terminating in the single stable
    // `emit::queue_built` the notifier/presenter consume. Positioned after the
    // header/info-line prints and before the notifier worker start, so the
    // buffered-ring drain order is unchanged.
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

    // Settings were loaded above (before the queue build) so `queue.assignee` could
    // filter the label queue; the operating run branch resolved from them still feeds
    // the events ctx below (ADR-0019 amendment #96), constant from the first event.
    // Each adapter's settings section is opaque JSON to the core; deserialize
    // the typed slices here with the same warn-and-default tolerance as the
    // file load, so a malformed section never aborts a run (ADR-0002 amendment).
    let claude_settings: ralphy_agent_claude::ClaudeSettings = settings
        .agent_settings(ralphy_agent_claude::ClaudeSettings::SECTION)
        .unwrap_or_else(|e| {
            warn!(error = %e, "malformed claude settings section — its persisted defaults ignored");
            Default::default()
        });
    let opencode_settings: ralphy_agent_opencode::OpenCodeSettings = settings
        .agent_settings(ralphy_agent_opencode::OpenCodeSettings::SECTION)
        .unwrap_or_else(|e| {
            warn!(error = %e, "malformed opencode settings section — its persisted defaults ignored");
            Default::default()
        });
    let persisted_opencode_model = opencode_settings.model.clone();
    let copilot_settings: ralphy_agent_copilot::CopilotSettings = settings
        .agent_settings(ralphy_agent_copilot::CopilotSettings::SECTION)
        .unwrap_or_else(|e| {
            warn!(error = %e, "malformed copilot settings section — its persisted defaults ignored");
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
    // The branch commits land on: a fresh `afk/run-<stamp>` in `new` mode (matching
    // the format literal in `runner.rs`), the current branch in `current` mode.
    let operating_branch = operating_branch(branch_mode, &stamp, start_branch.as_deref());

    // Start both delivery workers now that the run context is known. Events emitted
    // before this point (`queue built`) are buffered in the rings and drained on start.
    let (notifier, events_handle) = start_delivery(
        &obs,
        &title,
        queue.len(),
        &operating_branch,
        &repo_root,
        &ws,
    );

    // Clear any inherited ANTHROPIC_API_KEY so the agent draws on the subscription
    // quota (matching the ps1 oracle behaviour).
    //
    // Why `""` and not `remove_var`: setting to the empty string deliberately
    // mirrors the PowerShell oracle, which assigns `""` rather than unsetting.
    // Claude's handling of an absent key vs. an empty key is not verified, so we
    // keep the same sentinel value to stay on the tested path.
    //
    // Single-threaded safety: `set_var` is safe to call here because this point
    // in `main` is reached before any threads are spawned; no concurrent reader
    // can observe a torn environment state.
    //
    // Edition-2024 migration note: in Rust edition 2024, `std::env::set_var` (and
    // `remove_var`) become `unsafe` functions. When the crate migrates to edition
    // 2024, this call must be wrapped in an `unsafe` block with a comment
    // reiterating the single-threaded safety argument above.
    std::env::set_var("ANTHROPIC_API_KEY", "");

    // The run's global wall-clock deadline (if any), shared by the agent — which
    // clamps each issue's budget to it — and the queue's between-issue clock.
    let run_deadline = args
        .deadline_hours
        .map(|h| std::time::Instant::now() + std::time::Duration::from_secs_f64(h * 3600.0));

    // Select the adapter(s) per run and box as `&dyn Agent`; the core takes a
    // single `&dyn Agent` and never learns which vendor it holds (docs/adr/0004).
    // `--plan-agent` defaults to `--agent`: when they match, the executor box is
    // handed to the core directly so the single-agent path carries no wrapper
    // (byte-for-byte unchanged); otherwise a `SplitAgent` routes plan→planner,
    // execute→executor (docs/adr/0009).
    //
    // The persisted settings snapshot + `base_branch`/`branch_mode` were resolved
    // above (before the events ctx, so the operating branch is constant from the
    // first event); every run knob below resolves against that same snapshot.
    // ADR-0019 run-boundary event: emitted here, after branch-mode/base-branch
    // resolution, so it carries the CLI-only run parameters the core never sees.
    // The stream order is `queue.built` (above) then `run.started`; consumers order
    // by `id`/`time` and tolerate it (docs/events.md).
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

    // The empty-queue border (#222): the run emitted the full triad and did no work.
    // Deliberately NOT routed through `finalize_run` — that would also trigger
    // `maybe_consolidate_knowledge`, i.e. spawn a paid session on a run with nothing
    // to consolidate. Both `shutdown()`s run here, after the emit, so the rings drain.
    if queue.is_empty() {
        report::emit_run_finished_no_work(run_start);
        finish_border(presenter, notifier, events_handle);
        return Ok(());
    }

    let resolved_claude = ResolvedClaude {
        plan_model: config::resolve_str(
            args.plan_model.clone(),
            claude_settings.plan_model.clone(),
            "opus",
        ),
        plan_effort: config::resolve_str(
            args.plan_effort.clone(),
            claude_settings.plan_effort.clone(),
            "medium",
        ),
        exec_effort: config::resolve_str(
            args.exec_effort.clone(),
            claude_settings.exec_effort.clone(),
            "medium",
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
    // The idle watchdog knob stays an `Option` through the composition root: an
    // absent value is not "off", it is "let each execution path use the default
    // its progress signal can support" (docs/adr/0038). `Some(0)` is the opt-out.
    let idle_minutes = args.idle_minutes.or(settings.idle_minutes);
    let executor = build_agent(
        args.agent,
        &args,
        run_dir.clone(),
        run_deadline,
        persisted_opencode_model.clone(),
        &resolved_claude,
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
                &resolved_copilot,
                &resolved_cursor,
                &resolved_gemini,
                idle_minutes,
            ),
            executor,
        })
    };
    // `human_return_labels` (ADR-0016) was resolved once above (for the enriched
    // queue snapshot) and is handed here to the `gh`-free core, which skips any
    // queued issue carrying one of these and continues the queue.
    let cfg = QueueConfig {
        repo_root,
        base_branch,
        dry_run: args.dry_run,
        stamp,
        branch_mode,
        forced_issues,
        human_return_labels,
        // Limit stance for both phases. Every agent auto-resumes on a usage limit:
        // a scheduled reset (Codex/Claude) waits to its target time, and a limit with
        // no parseable reset (Kimi/OpenCode) parks a synthetic ~30-min poll window
        // (ADR-0030). `--stop-on-limit` is the opt-out that stops-and-reports instead.
        stop_on_limit_plan: args.stop_on_limit,
        stop_on_limit_exec: args.stop_on_limit,
        // The runner-enforced verify gate (ADR-0011): the per-repo fallback
        // command (tokenized into one argv) used only when a plan emits no
        // `## Verify` section, and the gate's own time budget.
        verify_fallback: settings
            .verify
            .command
            .as_deref()
            .map(str::trim)
            .filter(|c| !c.is_empty())
            .map(|c| vec![ralphy_core::verify::tokenize(c)]),
        // The gate owns its clock (docs/adr/0038): it no longer derives from the
        // per-issue cap, which is opt-in and `0`/unbounded by default and would
        // collapse the gate to a 0s timeout.
        verify_timeout: std::time::Duration::from_secs(
            resolve_verify_timeout_minutes(settings.verify.timeout_minutes).saturating_mul(60),
        ),
        // ADR-0015: when set, a `Done` that resolves to no verify gate at all is
        // parked for a human (`ready-for-human`) instead of closed on the
        // self-report. Absent/false preserves the ADR-0011 warn-and-close.
        require_verify_gate: settings.verify.require_verify_gate.unwrap_or(false),
        // The completion token the core quotes in its repair briefs. Named once
        // at the adapter layer; the core receives it as data and never learns
        // the literal (ADR-0002 amendment, #79).
        done_signal: ralphy_adapter_support::DONE_SENTINEL.to_owned(),
    };

    // The same deadline gates starting the next issue (between-issue clock).
    let clock = WallClock {
        deadline: run_deadline,
    };
    let tracker = GhTracker::new(cfg.repo_root.clone());

    let result = run_queue(&cfg, &queue, agent.as_ref(), &tracker, &clock);

    // ONE fold of the report, read by both `run.finished` and the final panel.
    let summary = result
        .as_ref()
        .ok()
        .map(|r| summary::RunSummary::from_report(r, queue.len()));

    // Finalize the presenter, consolidate knowledge, emit `run finished`, and tear
    // down the notifier then the sink — in that exact order (ADR-0006/-0007/-0019).
    // Kept before the `?` propagation so a non-green result still finalizes and
    // pushes; `render_final_panel` runs after, only on the green path.
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

/// Close out the `--if-idle` deferral border (#222): emit `run skipped`, clear the
/// live region, print the folded notice, then drain BOTH delivery rings. The
/// ordering is load-bearing — the emit precedes the two `shutdown()`s, so the event
/// is delivered rather than discarded with the ring. Exit code 0 is preserved: a
/// deferral is a clean outcome, so a scheduler's history shows no false failure.
fn finish_if_idle(
    presenter: &ui::PresenterHandle,
    msg: &str,
    notifier: Option<telegram::notifier::NotifierHandle>,
    events: Option<events::sink::EventsHandle>,
) -> Result<()> {
    ralphy_core::emit::run_skipped(msg);
    finish_border(presenter, notifier, events);
    Ok(())
}

/// The teardown both no-work borders share (#222), in the ONE order that works:
/// clear the live region, print the folded notice, THEN drain both delivery rings.
/// Every caller must have emitted its border event BEFORE calling this — a
/// `shutdown()` that runs first would drop the event with the ring.
fn finish_border(
    presenter: &ui::PresenterHandle,
    notifier: Option<telegram::notifier::NotifierHandle>,
    events: Option<events::sink::EventsHandle>,
) {
    // finalize before printing so the live region is cleared first (ADR-0006).
    presenter.finalize();
    presenter.print_edge_notice();
    if let Some(n) = notifier {
        n.shutdown();
    }
    if let Some(e) = events {
        e.shutdown();
    }
}

/// Start both delivery workers (Telegram notifier + CloudEvents sink) over the
/// already-installed rings. The CROSS-PATH INVARIANT this exists for: it is the ONLY
/// place a worker is spawned, so every exit path of `run_cmd` — including the two
/// no-work borders (#222) — can reach a STARTED worker and drain its ring rather than
/// discarding the buffered events.
///
/// `try_start_notifier` runs `getMe` and `try_start_sink` spawns a thread; either
/// failing warns once and returns `None`, leaving the installed Layer inert and the
/// run unaffected (ADR-0007 D7).
fn start_delivery(
    obs: &Observability,
    title: &str,
    queue_len: usize,
    branch: &str,
    repo_root: &std::path::Path,
    ws: &Workspace,
) -> (
    Option<telegram::notifier::NotifierHandle>,
    Option<events::sink::EventsHandle>,
) {
    let mut notifier: Option<telegram::notifier::NotifierHandle> = None;
    if let (Some(event_queue), Some(cfg)) = (obs.event_queue.as_ref(), obs.tg_cfg.as_ref()) {
        if let (Some(chat_id), Some(token)) = (
            cfg.chat_id,
            telegram::config::effective_token(Some(&cfg.token)),
        ) {
            let state = runstate::RunState::new(title.to_string(), queue_len);
            let client =
                telegram::client::BotClient::new(telegram::client::UreqTransport::new(token));
            notifier =
                telegram::notifier::try_start_notifier(client, chat_id, state, event_queue.clone());
        }
    }

    // The process `runid` and the emitter identity are minted once here.
    let mut events_handle: Option<events::sink::EventsHandle> = None;
    if let (Some(queue), Some(url)) = (obs.event_sink_queue.as_ref(), obs.events_url.as_ref()) {
        let ctx = events::envelope::EventCtx {
            source: events::emitter::source(&obs.events_slug),
            runid: events::emitter::new_runid(),
            emitter: serde_json::to_value(events::emitter::detect(repo_root)).unwrap_or_default(),
            git: serde_json::json!({
                "repository": obs.events_slug,
                "branch": branch,
            }),
        };
        let transport =
            events::client::UreqEventTransport::new(url.clone(), obs.events_token.clone());
        events_handle = events::sink::try_start_sink(transport, ctx, queue.clone(), ws.plan_path());
    }

    (notifier, events_handle)
}

/// The observability bundle installed once at run boot: the presenter handle plus
/// the notifier/sink rings and the resolved events identity — every field the later
/// worker starts (`try_start_notifier`, `try_start_sink`) and the `run started` /
/// `queue built` emits consume. Built in [`install_observability`].
struct Observability {
    presenter: ui::PresenterHandle,
    event_queue: Option<Arc<telegram::notifier::EventQueue>>,
    tg_cfg: Option<telegram::config::TelegramConfig>,
    event_sink_queue: Option<Arc<telegram::notifier::EventQueue>>,
    events_url: Option<String>,
    events_token: Option<String>,
    events_slug: String,
}

/// Wire the run's observability stack — the Telegram notifier ring/Layer, the
/// CloudEvents sink ring/Layer, the events-token env scrub, and the tracing
/// subscriber — and return the handles the later worker starts consume.
///
/// ORDERING (load-bearing, ADR-0019): `strip_events_token_from_env` runs HERE, before
/// `init_tracing` installs the layers and before any worker thread is spawned, so the
/// `remove_var` stays single-threaded with no concurrent `getenv` to race. The caller
/// invokes this at one fixed position in `run_cmd`, so no side effect is reordered.
fn install_observability(
    log_file: Option<std::fs::File>,
    args: &RunArgs,
    repo_root: &std::path::Path,
) -> Observability {
    // Decide up front whether this run notifies (ADR-0007 D1/D7): only when
    // Telegram is configured (a token AND a captured chat) and the run is neither
    // `--no-telegram` nor a `--dry-run`. When it does, create the shared event ring
    // and install the notifier Layer alongside the file/presenter layers so it sees
    // the lifecycle from `queue built` onward. The worker is started later, once the
    // queue (and thus the title) is known.
    let tg_cfg = telegram::config::TelegramConfig::load().ok().flatten();
    let configured = tg_cfg.as_ref().is_some_and(|c| {
        c.chat_id.is_some() && telegram::config::effective_token(Some(&c.token)).is_some()
    });
    let notify = telegram::notifier::should_notify(configured, args.no_telegram, args.dry_run);
    let event_queue = notify.then(|| Arc::new(telegram::notifier::EventQueue::new()));
    let notifier_layer = event_queue
        .as_ref()
        .map(|q| telegram::notifier::new_notifier_layer(q.clone()));

    // The CloudEvents sink (ADR-0019): active only when this repo has an
    // `events.url` in the global store (`~/.ralphy/events.toml`) — an absent entry
    // means non-users pay nothing. Build the ring + Layer here so the sink sees the
    // lifecycle from `queue built` onward; the worker starts once the run context
    // is known (below). The token honours `RALPHY_EVENTS_TOKEN` over the stored one.
    let events_slug = git::project_slug(repo_root);
    let events_entry = events::config::EventsStore::load()
        .ok()
        .unwrap_or_default()
        .entry(&events_slug)
        .cloned();
    let events_url = events_entry.as_ref().and_then(|e| e.url.clone());
    let events_token =
        events::config::effective_token(events_entry.as_ref().and_then(|e| e.token.as_deref()));
    // Strip RALPHY_EVENTS_TOKEN from the process env now that the effective token is
    // captured in `events_token` (an owned String the sink transport keeps using):
    // every child spawned later inherits this environment and none must see the
    // sink's bearer token (ADR-0019). Done HERE — before init_tracing installs the
    // layers and before any worker thread is spawned — so the `remove_var` runs
    // single-threaded, with no concurrent `getenv` to race (edition 2021).
    strip_events_token_from_env();
    let event_sink_queue = events_url.as_ref().map(|_| events::sink::new_queue());
    let events_layer = event_sink_queue
        .as_ref()
        .map(|q| events::sink::new_events_layer(q.clone()));

    let presenter = init_tracing(log_file, args.verbose, notifier_layer, events_layer);

    Observability {
        presenter,
        event_queue,
        tg_cfg,
        event_sink_queue,
        events_url,
        events_token,
        events_slug,
    }
}

/// Emit the ADR-0020/-0021 `queue built` telemetry: enrich it with the per-issue
/// snapshot the runner would judge (`data.issues[]`) and mark the applied assignee
/// scope, terminating in the single stable `emit::queue_built` the notifier /
/// presenter consume. Both resolutions are best-effort telemetry — a `gh` blip warns
/// and emits the legacy/unmarked shape rather than aborting the run. The caller
/// positions this after the header/info-line prints and before the notifier worker
/// start, so the buffered-ring drain order is unchanged.
#[allow(clippy::too_many_arguments)]
fn emit_queue_built(
    queue: &[ralphy_core::Issue],
    forced_issues: &[u64],
    human_return_labels: &[String],
    repo_root: &std::path::Path,
    args: &RunArgs,
    assignee: Option<&str>,
    order: &[String],
    stop_before: u64,
    scope: &str,
) {
    let issues_json = {
        let tracker = GhTracker::new(repo_root);
        match ralphy_core::resolve_queue_view(queue, forced_issues, human_return_labels, &tracker) {
            Ok(view) => serde_json::to_string(&view.issues).unwrap_or_default(),
            Err(e) => {
                warn!(error = %e, "resolving the queue snapshot failed — emitting the legacy queue.built shape");
                String::new()
            }
        }
    };
    // The filter is the *applied* one — `None` on an explicit `--issues`/`--only-issue`
    // selection (which bypasses the assignee filter), matching how those paths fetch
    // unfiltered; the concrete login is emitted, never the literal `@me`.
    let applied_assignee = if !args.issues.is_empty() || args.only_issue.is_some() {
        None
    } else {
        assignee.map(str::to_string)
    };
    let assignee_filter: Option<String> = match applied_assignee.as_deref() {
        Some(a) => match github::resolve_login(a, repo_root) {
            Ok(l) => Some(l),
            Err(e) => {
                warn!(error = %e, "resolving @me for the assignee_filter mark failed — emitting queue.built without the scope mark");
                None
            }
        },
        None => None,
    };
    ralphy_core::emit::queue_built(
        queue.len() as u64,
        &order.join(" -> "),
        stop_before,
        &issues_json,
        assignee_filter.as_deref().unwrap_or(""),
        scope,
    );
}

/// Close out the run in the exact ADR-0006/-0007/-0019 order: finalize the presenter
/// FIRST (clears the live region before any print), THEN consolidate loose knowledge
/// notes, THEN emit `run finished` (only on a clean `Ok` — a crash is detected by
/// heartbeat silence), THEN tear down the notifier, THEN the CloudEvents sink. Each
/// teardown joins under a bounded timeout so a wedged network can't hold the process
/// open. Borrows `result` so the caller can still `?`-propagate it afterwards.
#[allow(clippy::too_many_arguments)]
fn finalize_run(
    agent: CliAgent,
    presenter: &ui::PresenterHandle,
    result: &Result<ralphy_core::QueueReport>,
    dry_run: bool,
    ws: &Workspace,
    stamp: &str,
    summary: Option<&summary::RunSummary>,
    run_start: std::time::Instant,
    notifier: Option<telegram::notifier::NotifierHandle>,
    events_handle: Option<events::sink::EventsHandle>,
) -> ralphy_core::Usage {
    // Flush the queue bar to N/N and clear the live region before anything else
    // prints — whether that is the panel or `anyhow`'s error on the `?` propagation.
    presenter.finalize();

    // Consolidate any loose knowledge notes into KNOWLEDGE.md. Runs BEFORE the
    // notifier/sink shutdown and AFTER the presenter finalize so it surfaces as a
    // first-class lifecycle event in both surfaces (see `maybe_consolidate_knowledge`).
    // Its token cost is returned so the caller folds it into the panel run total
    // (issue #269); the ledger line is written inside `maybe_consolidate_knowledge`.
    let consolidation_usage =
        maybe_consolidate_knowledge(agent, result.is_ok(), dry_run, ws, stamp);

    // ADR-0019 run-boundary event: emitted only on a CLEAN termination — a crash/kill
    // is detected by heartbeat silence, never a `run.finished`. Emitted BEFORE the
    // sink shutdown so the worker drains and POSTs it as the run's last event. The
    // run usage folds in the consolidation pass so the event reports total vendor
    // spend, matching the panel footer (issue #269).
    if let (Some(s), Ok(report)) = (summary, result.as_ref()) {
        let mut run_usage = report.run_usage.clone();
        run_usage.add_tokens(&consolidation_usage);
        emit_run_finished(s, &run_usage, run_start);
    }

    // Tear down the notifier (ADR-0007 D4), then the CloudEvents sink: each worker
    // renders/drains its terminal state and flushes, joined under a bounded timeout.
    if let Some(notifier) = notifier {
        notifier.shutdown();
    }
    if let Some(events_handle) = events_handle {
        events_handle.shutdown();
    }

    consolidation_usage
}

/// The verify gate's time budget in minutes: the persisted `verify.timeout_minutes`,
/// else [`ralphy_core::VERIFY_GATE_FALLBACK_MINUTES`].
///
/// Split out of the `QueueConfig` literal so the "the gate never inherits the
/// per-issue cap" rule is a testable unit rather than an inline expression
/// (docs/adr/0038). Unlike the per-issue cap, `0` is not a sentinel here — a
/// gate with no timeout is not a thing the runner can enforce.
fn resolve_verify_timeout_minutes(persisted: Option<u64>) -> u64 {
    persisted
        .filter(|m| *m > 0)
        .unwrap_or(ralphy_core::VERIFY_GATE_FALLBACK_MINUTES)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The two no-work borders emit events now (#222); no imperative notice print
    /// may survive in the orchestrator, or the console would double-print (or, worse,
    /// keep printing on a path that no longer emits).
    #[test]
    fn no_print_notice_call_remains_in_run_cmd() {
        assert!(
            // Split so this assertion is not itself the occurrence it forbids.
            !include_str!("run.rs").contains(concat!("print_", "notice(")),
            "the run borders print from the folded event, never imperatively"
        );
    }

    /// A fake CloudEvents sink recording every delivered envelope.
    struct RecordingSink(Arc<std::sync::Mutex<Vec<serde_json::Value>>>);
    impl crate::events::client::EventSink for RecordingSink {
        fn post(
            &self,
            body: &serde_json::Value,
        ) -> Result<crate::events::client::PostOutcome, anyhow::Error> {
            self.0
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(body.clone());
            Ok(crate::events::client::PostOutcome::Delivered)
        }
    }

    /// Run `f` with the REAL sink Layer installed over a started worker, and hand
    /// back every envelope the worker delivered. This is the whole border path —
    /// emit → Layer → ring → running worker → drain — not a hand-built ring.
    fn envelopes_delivered_by(f: impl FnOnce(Option<events::sink::EventsHandle>)) -> Vec<String> {
        use tracing_subscriber::prelude::*;

        let queue = events::sink::new_queue();
        let recorded = Arc::new(std::sync::Mutex::new(Vec::new()));
        let handle = events::sink::try_start_sink(
            RecordingSink(Arc::clone(&recorded)),
            events::envelope::EventCtx {
                source: "ralphy/o/r".to_string(),
                runid: "01TESTRUNIDTESTRUNIDTE".to_string(),
                emitter: serde_json::json!({ "version": "0.0.0", "pid": 4242 }),
                git: serde_json::json!({ "repository": "o/r", "branch": "main" }),
            },
            queue.clone(),
            std::env::temp_dir().join("ralphy-nonexistent-plan.md"),
        );
        let subscriber =
            tracing_subscriber::registry().with(events::sink::new_events_layer(queue.clone()));
        tracing::subscriber::with_default(subscriber, || f(handle));
        let out = recorded.lock().unwrap_or_else(|e| e.into_inner()).clone();
        out.iter()
            .map(|e| e["type"].as_str().unwrap_or_default().to_string())
            .collect()
    }

    /// The `--if-idle` deferral is a clean exit 0 — a scheduler's history must show
    /// no failure — AND its `run.skipped` actually reaches the sink: the emit happens
    /// before the drain, over a worker that was started on this very path. Reordering
    /// the emit after the shutdown, or dropping `start_delivery` from the border,
    /// reds here.
    #[test]
    fn if_idle_edge_returns_ok() {
        let presenter = ui::PresenterHandle::plain();
        let mut out = None;
        let types = envelopes_delivered_by(|events| {
            out = Some(finish_if_idle(
                &presenter,
                "skipped: run in progress since 2026-07-19 10:00:00, pid 4242",
                None,
                events,
            ));
        });
        assert!(out.expect("the border ran").is_ok(), "a deferral exits 0");
        assert_eq!(types, vec!["dev.ralphy.run.skipped"]);
    }

    /// The empty-queue border's own teardown: `emit_run_finished_no_work` then
    /// `finish_border` must deliver the `run.finished`, not discard it with the ring.
    #[test]
    fn no_work_border_delivers_run_finished_before_the_drain() {
        let presenter = ui::PresenterHandle::plain();
        let types = envelopes_delivered_by(|events| {
            report::emit_run_finished_no_work(std::time::Instant::now());
            finish_border(&presenter, None, events);
        });
        assert_eq!(types, vec!["dev.ralphy.run.finished"]);
    }

    #[test]
    fn resolution_byte_for_byte_when_absent() {
        use crate::cli::CliBranchMode;

        // With no flag AND no setting, every knob must resolve to today's
        // hardcoded default, leaving behaviour unchanged (ADR-0010).
        assert_eq!(
            config::resolve_str(None, None, "origin/main"),
            "origin/main"
        );
        assert_eq!(config::resolve_str(None, None, "opus"), "opus");
        assert_eq!(config::resolve_str(None, None, "medium"), "medium");
        assert_eq!(config::resolve_str(None, None, "sonnet"), "sonnet");
        assert_eq!(config::resolve_u64(None, None, 90), 90);

        // The branch_mode resolution chain with (no flag, no setting) yields New.
        let flag: Option<CliBranchMode> = None;
        let persisted: Option<String> = None;
        let branch_mode: BranchMode = flag
            .map(BranchMode::from)
            .or_else(|| {
                persisted
                    .as_deref()
                    .and_then(|m| config::parse_branch_mode(m).ok())
            })
            .unwrap_or(BranchMode::New);
        assert_eq!(branch_mode, BranchMode::New);
    }

    #[test]
    fn unset_max_minutes_resolves_to_uncapped() {
        // The per-issue cap is opt-in (docs/adr/0038): an unset flag AND unset
        // setting resolve to `0`, which `issue_deadline` reads as "no per-issue
        // cap" — the issue is bounded only by `--deadline-hours`. Liveness is
        // the idle watchdog's job, not this knob's; a finite default here would
        // cut healthy long issues to catch a hang it cannot recognize.
        let resolved = config::resolve_u64(None, None, ralphy_core::DEFAULT_MAX_MINUTES_PER_ISSUE);
        assert_eq!(resolved, 0);
    }

    #[test]
    fn max_minutes_precedence_flag_over_setting_over_default() {
        // Opting in must still work in both directions, in the documented order.
        let d = ralphy_core::DEFAULT_MAX_MINUTES_PER_ISSUE;
        assert_eq!(config::resolve_u64(Some(30), Some(90), d), 30);
        assert_eq!(config::resolve_u64(None, Some(90), d), 90);
        // An explicit `0` is a deliberate "no cap", not an absent value.
        assert_eq!(config::resolve_u64(Some(0), Some(90), d), 0);
    }

    #[test]
    fn verify_timeout_no_longer_derives_from_the_per_issue_cap() {
        // The gate owns its own clock (docs/adr/0038): with the cap uncapped
        // (`0`, the default) verify must still get a finite window, and it must
        // not silently inherit an unrelated per-issue cap either.
        assert_eq!(
            resolve_verify_timeout_minutes(None),
            ralphy_core::VERIFY_GATE_FALLBACK_MINUTES
        );
        assert_eq!(resolve_verify_timeout_minutes(Some(15)), 15);
        assert!(resolve_verify_timeout_minutes(None) > 0);
    }
}
