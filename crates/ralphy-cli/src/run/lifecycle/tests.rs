use super::*;
use crate::run::report;

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
            None,
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
        finish_border(&presenter, None, events, None);
    });
    assert_eq!(types, vec!["dev.ralphy.run.finished"]);
}
