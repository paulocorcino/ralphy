//! The run's observability lifecycle: install the rings and the tracing
//! subscriber, start the delivery workers, and the three exit borders that
//! drain them in the decided order (ADR-0006/-0007/-0019, #222).

use std::sync::Arc;

use anyhow::Result;
use ralphy_core::{git, Workspace};

use super::report::{emit_run_finished, maybe_consolidate_knowledge};
use super::wiring::{init_tracing, strip_events_token_from_env};
use super::{snapshot_engine, summary};
use crate::cli::{CliAgent, RunArgs};
use crate::{events, runstate, telegram, ui};

/// Close out the `--if-idle` deferral border (#222): emit `run skipped`, clear the
/// live region, print the folded notice, then drain BOTH delivery rings. The
/// ordering is load-bearing — the emit precedes the two `shutdown()`s, so the event
/// is delivered rather than discarded with the ring. Exit code 0 is preserved: a
/// deferral is a clean outcome, so a scheduler's history shows no false failure.
pub(super) fn finish_if_idle(
    presenter: &ui::PresenterHandle,
    msg: &str,
    notifier: Option<telegram::notifier::NotifierHandle>,
    events: Option<events::sink::EventsHandle>,
    snapshot: Option<crate::delivery::WorkerHandle>,
) -> Result<()> {
    ralphy_core::emit::run_skipped(msg);
    finish_border(presenter, notifier, events, snapshot);
    Ok(())
}

/// The teardown both no-work borders share (#222), in the ONE order that works:
/// clear the live region, print the folded notice, THEN drain both delivery rings.
/// Every caller must have emitted its border event BEFORE calling this — a
/// `shutdown()` that runs first would drop the event with the ring.
pub(super) fn finish_border(
    presenter: &ui::PresenterHandle,
    notifier: Option<telegram::notifier::NotifierHandle>,
    events: Option<events::sink::EventsHandle>,
    snapshot: Option<crate::delivery::WorkerHandle>,
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
    // Drained last, before the caller returns and the removal guard drops.
    if let Some(s) = snapshot {
        s.shutdown();
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
#[allow(clippy::too_many_arguments)]
pub(super) fn start_delivery(
    obs: &Observability,
    title: &str,
    queue_len: usize,
    branch: &str,
    repo_root: &std::path::Path,
    ws: &Workspace,
    runid: &str,
    snapshot_ctx: Option<runstate::snapshot::SnapshotCtx>,
) -> (
    Option<telegram::notifier::NotifierHandle>,
    Option<events::sink::EventsHandle>,
    Option<crate::delivery::WorkerHandle>,
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
            runid: runid.to_string(),
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

    // The snapshot publisher (ADR-0047 §2). `None` on the `--if-idle` deferral: it
    // never acquired the run lock and never returns to a scope holding the removal
    // guard, so publishing there would leak a document for a run that never ran.
    let snapshot_handle = snapshot_ctx.and_then(|ctx| {
        snapshot_engine::try_start_snapshot(
            ctx,
            runstate::RunState::new(title.to_string(), queue_len),
            repo_root.to_path_buf(),
            obs.snapshot_queue.clone(),
            ws.plan_path(),
        )
    });

    (notifier, events_handle, snapshot_handle)
}

/// The observability bundle installed once at run boot: the presenter handle plus
/// the notifier/sink rings and the resolved events identity — every field the later
/// worker starts (`try_start_notifier`, `try_start_sink`) and the `run started` /
/// `queue built` emits consume. Built in [`install_observability`].
pub(super) struct Observability {
    pub(super) presenter: ui::PresenterHandle,
    pub(super) event_queue: Option<Arc<telegram::notifier::EventQueue>>,
    pub(super) tg_cfg: Option<telegram::config::TelegramConfig>,
    pub(super) event_sink_queue: Option<Arc<telegram::notifier::EventQueue>>,
    /// The run-snapshot ring (ADR-0047 §1): unconditional — no config gates it,
    /// which is exactly what makes a terminal-started run visible in the panel.
    pub(super) snapshot_queue: Arc<telegram::notifier::EventQueue>,
    pub(super) events_url: Option<String>,
    pub(super) events_token: Option<String>,
    pub(super) events_slug: String,
}

/// Wire the run's observability stack — the Telegram notifier ring/Layer, the
/// CloudEvents sink ring/Layer, the events-token env scrub, and the tracing
/// subscriber — and return the handles the later worker starts consume.
///
/// ORDERING (load-bearing, ADR-0019): `strip_events_token_from_env` runs HERE, before
/// `init_tracing` installs the layers and before any worker thread is spawned, so the
/// `remove_var` stays single-threaded with no concurrent `getenv` to race. The caller
/// invokes this at one fixed position in `run_cmd`, so no side effect is reordered.
pub(super) fn install_observability(
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

    // The run-snapshot ring + Layer (ADR-0047 §1/§2): installed unconditionally, so
    // the panel sees every run — including one typed by hand in a terminal.
    let snapshot_queue = Arc::new(telegram::notifier::EventQueue::new());
    let snapshot_layer =
        crate::delivery::DeliveryLayer::new(snapshot_queue.clone(), "run::snapshot");

    let presenter = init_tracing(
        log_file,
        args.verbose,
        notifier_layer,
        events_layer,
        snapshot_layer,
    );

    Observability {
        presenter,
        event_queue,
        tg_cfg,
        event_sink_queue,
        snapshot_queue,
        events_url,
        events_token,
        events_slug,
    }
}

/// Close out the run in the exact ADR-0006/-0007/-0019 order: finalize the presenter
/// FIRST (clears the live region before any print), THEN consolidate loose knowledge
/// notes, THEN emit `run finished` (only on a clean `Ok` — a crash is detected by
/// heartbeat silence), THEN tear down the notifier, THEN the CloudEvents sink. Each
/// teardown joins under a bounded timeout so a wedged network can't hold the process
/// open. Borrows `result` so the caller can still `?`-propagate it afterwards.
#[allow(clippy::too_many_arguments)]
pub(super) fn finalize_run(
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
    snapshot_handle: Option<crate::delivery::WorkerHandle>,
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
    // The snapshot worker last: its terminal document is written by `on_finish`,
    // and the caller's removal guard drops after this returns (ADR-0047 §8).
    if let Some(snapshot_handle) = snapshot_handle {
        snapshot_handle.shutdown();
    }

    consolidation_usage
}

#[cfg(test)]
mod tests;
