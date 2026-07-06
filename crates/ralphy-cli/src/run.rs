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

use crate::cli::RunArgs;
use crate::{config, events, runlock, runstate, split_agent, telegram, ui};

mod report;
mod wiring;

use report::{
    emit_run_finished, empty_queue_scope, maybe_consolidate_knowledge, render_final_panel,
};
use wiring::{
    build_agent, build_run_queue, effective_stop_on_limit, init_tracing, operating_branch,
    preflight_agents, resolve_plan_agent, strip_events_token_from_env, ResolvedClaude,
};

pub(crate) fn run_cmd(args: RunArgs) -> Result<()> {
    // Anchors the `run.finished` `duration_s` (ADR-0019) — the run's wall-clock.
    let run_start = std::time::Instant::now();
    let repo_root = git::resolve_toplevel(&args.repo)?;
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
    let Observability {
        presenter,
        event_queue,
        tg_cfg,
        event_sink_queue,
        events_url,
        events_token,
        events_slug,
    } = install_observability(log_file, &args, &repo_root);

    // The repo name feeds the run title (below); the branding header is printed once
    // that title is known, so the console face is seeded by the same title as the
    // Telegram card — identical per run, varying across runs.
    let repo_name = repo_root
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("repo");

    info!(repo = %repo_root.display(), %stamp, dry_run = args.dry_run, "ralphy run");

    std::fs::create_dir_all(ws.ralphy_dir()).ok();

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
            info!("{msg}");
            // finalize before printing so the live region is cleared first (ADR-0006).
            presenter.finalize();
            presenter.print_notice(&msg);
            return Ok(());
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
    let effective_labels = github::resolve_queue_labels(&args.queue_label, &repo_root);
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

    if queue.is_empty() {
        let scope = empty_queue_scope(
            &args.issues,
            args.only_issue,
            &effective_labels,
            assignee.as_deref(),
        );
        // finalize before printing so the live region is cleared first (ADR-0006).
        presenter.finalize();
        presenter.print_notice(&format!("No open issues for {scope} to process. Done."));
        return Ok(());
    }
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
    // `info!("queue built")` the notifier/presenter consume. Positioned after the
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
    );

    // Start the Telegram notifier worker now that the queue (and thus the title)
    // is known. `try_start_notifier` runs `getMe`; on failure it warns once and
    // returns `None`, leaving the installed Layer inert and the run unaffected
    // (ADR-0007 D7). Events emitted before this point (just `queue built`) are
    // buffered in the ring and drained by the worker on start.
    let mut notifier: Option<telegram::notifier::NotifierHandle> = None;
    if let (Some(event_queue), Some(cfg)) = (event_queue.as_ref(), tg_cfg.as_ref()) {
        if let (Some(chat_id), Some(token)) = (
            cfg.chat_id,
            telegram::config::effective_token(Some(&cfg.token)),
        ) {
            let state = runstate::RunState::new(title.clone(), queue.len());
            let client =
                telegram::client::BotClient::new(telegram::client::UreqTransport::new(token));
            notifier =
                telegram::notifier::try_start_notifier(client, chat_id, state, event_queue.clone());
        }
    }

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

    // Start the CloudEvents sink worker now that the run context is known. The
    // process `runid` and the emitter identity are minted once here; the worker
    // drains the ring (already buffering `queue built`) and POSTs each event as a
    // CloudEvents envelope. A spawn failure leaves the installed Layer inert.
    let mut events_handle: Option<events::sink::EventsHandle> = None;
    if let (Some(queue), Some(url)) = (event_sink_queue.as_ref(), events_url.as_ref()) {
        let ctx = events::envelope::EventCtx {
            source: events::emitter::source(&events_slug),
            runid: events::emitter::new_runid(),
            emitter: serde_json::to_value(events::emitter::detect(&repo_root)).unwrap_or_default(),
            git: serde_json::json!({
                "repository": events_slug,
                "branch": operating_branch,
            }),
        };
        let transport = events::client::UreqEventTransport::new(url.clone(), events_token.clone());
        events_handle = events::sink::try_start_sink(transport, ctx, queue.clone(), ws.plan_path());
    }

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
    info!(
        repo = %events_slug,
        queue_labels = %effective_labels.join(","),
        agent = args.agent.cli_name(),
        plan_agent = plan_agent.cli_name(),
        branch_mode = branch_mode_str,
        base = %base_branch,
        deadline_hours = args.deadline_hours.unwrap_or(0.0),
        "run started"
    );
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
    };
    let executor = build_agent(
        args.agent,
        &args,
        run_dir.clone(),
        run_deadline,
        persisted_opencode_model.clone(),
        &resolved_claude,
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
        // Per-phase limit stance, each derived from the agent serving that phase;
        // an explicit `--stop-on-limit` forces both (docs/adr/0009).
        stop_on_limit_plan: effective_stop_on_limit(args.stop_on_limit, plan_agent),
        stop_on_limit_exec: effective_stop_on_limit(args.stop_on_limit, args.agent),
        // The runner-enforced verify gate (ADR-0011): the per-repo fallback
        // command (tokenized into one argv) used only when a plan emits no
        // `## Verify` section, and the gate's time budget (the per-issue budget).
        verify_fallback: settings
            .verify
            .command
            .as_deref()
            .map(str::trim)
            .filter(|c| !c.is_empty())
            .map(|c| vec![ralphy_core::verify::tokenize(c)]),
        // The verify gate borrows the per-issue budget, but a disabled budget
        // (`0` = no per-issue cap) must not collapse the gate to a 0s timeout —
        // fall back to the default window so verify still has room to run.
        verify_timeout: std::time::Duration::from_secs(
            match resolved_claude.max_minutes_per_issue {
                0 => ralphy_core::VERIFY_GATE_FALLBACK_MINUTES,
                n => n,
            }
            .saturating_mul(60),
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

    // Finalize the presenter, consolidate knowledge, emit `run finished`, and tear
    // down the notifier then the sink — in that exact order (ADR-0006/-0007/-0019).
    // Kept before the `?` propagation so a non-green result still finalizes and
    // pushes; `render_final_panel` runs after, only on the green path.
    finalize_run(
        &presenter,
        &result,
        args.dry_run,
        &ws,
        &cfg.stamp,
        queue.len(),
        run_start,
        notifier,
        events_handle,
    );

    let report = result?;

    render_final_panel(
        &presenter,
        report,
        branch_mode,
        args.dry_run,
        &cfg.repo_root,
    );
    Ok(())
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
/// scope, terminating in the single stable `info!("queue built")` the notifier /
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
    // message consumed by the telegram notifier / presenter — keep stable
    info!(
        count = queue.len(),
        order = %order.join(" -> "),
        stop_before,
        issues_json = %issues_json,
        assignee_filter = %assignee_filter.as_deref().unwrap_or(""),
        "queue built"
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
    presenter: &ui::PresenterHandle,
    result: &Result<ralphy_core::QueueReport>,
    dry_run: bool,
    ws: &Workspace,
    stamp: &str,
    queue_len: usize,
    run_start: std::time::Instant,
    notifier: Option<telegram::notifier::NotifierHandle>,
    events_handle: Option<events::sink::EventsHandle>,
) {
    // Flush the queue bar to N/N and clear the live region before anything else
    // prints — whether that is the panel or `anyhow`'s error on the `?` propagation.
    presenter.finalize();

    // Consolidate any loose knowledge notes into KNOWLEDGE.md. Runs BEFORE the
    // notifier/sink shutdown and AFTER the presenter finalize so it surfaces as a
    // first-class lifecycle event in both surfaces (see `maybe_consolidate_knowledge`).
    maybe_consolidate_knowledge(result.is_ok(), dry_run, ws, stamp);

    // ADR-0019 run-boundary event: emitted only on a CLEAN termination — a crash/kill
    // is detected by heartbeat silence, never a `run.finished`. Emitted BEFORE the
    // sink shutdown so the worker drains and POSTs it as the run's last event.
    if let Ok(report) = result.as_ref() {
        emit_run_finished(report, queue_len, run_start);
    }

    // Tear down the notifier (ADR-0007 D4), then the CloudEvents sink: each worker
    // renders/drains its terminal state and flushes, joined under a bounded timeout.
    if let Some(notifier) = notifier {
        notifier.shutdown();
    }
    if let Some(events_handle) = events_handle {
        events_handle.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
