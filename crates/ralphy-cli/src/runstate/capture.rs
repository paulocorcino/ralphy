//! Test-only capture harness for the `tracing` event vocabulary (ADR-0039 §2).
//!
//! [`capture_events`] runs a closure with every `tracing` event emitted on the
//! calling thread recorded as a [`Captured`] — the `(level, target, message,
//! fields)` triple, with `fields` built by the SAME [`EventFields`] `Visit` impl
//! the production decoder consumes. That shared extractor is the point: a pin
//! written here fails when an emitter drifts away from what
//! [`super::event_to_runevent`] reads, not merely when a string changes.
//!
//! This module is permanent infrastructure — the ADR-0039 §2 round-trip gate is
//! built on `capture_events` in Fase 1 — so it returns the unfiltered
//! `Vec<Captured>` rather than an asserted view.
//!
//! Nothing here is compiled into the shipped binary (`#[cfg(test)]` at the
//! `mod` declaration in `runstate.rs`); the crate's public surface is unchanged.

use std::cell::RefCell;
use std::sync::{Arc, Mutex, OnceLock};

use tracing::Level;
use tracing_subscriber::layer::{Context, Layer, SubscriberExt};

use super::EventFields;

/// One captured `tracing` event: the metadata triple plus the decoder's own
/// field extraction.
pub(crate) struct Captured {
    pub level: Level,
    pub target: String,
    pub message: String,
    pub fields: EventFields,
}

type Sink = Arc<Mutex<Vec<Captured>>>;

std::thread_local! {
    /// The active sink for THIS thread, if any. A `#[test]` runs on its own
    /// thread, so a capturing test never sees a sibling test's events.
    static SINK: RefCell<Option<Sink>> = const { RefCell::new(None) };
}

/// Routes every event to the calling thread's [`SINK`] (a no-op when none is
/// set).
struct CaptureLayer;

impl<S: tracing::Subscriber> Layer<S> for CaptureLayer {
    fn on_event(&self, event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
        SINK.with(|s| {
            let Some(sink) = s.borrow().as_ref().cloned() else {
                return;
            };
            // The `Visit` impl never sets `level`; the decoder's "level wins"
            // short-circuit reads `fields.level`, so seed it from the metadata
            // BEFORE recording (a visitor could never supply it).
            let mut fields = EventFields {
                level: *event.metadata().level(),
                ..Default::default()
            };
            event.record(&mut fields);
            // Built BEFORE the lock: nothing that could re-enter `tracing` may run
            // while the sink's Mutex is held.
            let captured = Captured {
                level: fields.level,
                target: event.metadata().target().to_string(),
                message: fields.message.clone(),
                fields,
            };
            sink.lock().unwrap().push(captured);
        });
    }
}

/// Install [`CaptureLayer`] as the process-global default exactly once.
///
/// Global, not `with_default`: a callsite first registered on another thread
/// caches its interest as *disabled* under a thread-local dispatcher, and the
/// event then silently never arrives (the trap already paid for in
/// `crates/ralphy-core/tests/queue.rs`). A default set elsewhere is a harmless
/// no-op — the capture simply records nothing.
fn install() {
    static ONCE: OnceLock<()> = OnceLock::new();
    ONCE.get_or_init(|| {
        let _ = tracing::subscriber::set_global_default(
            tracing_subscriber::registry().with(CaptureLayer),
        );
    });
}

/// Restores the sink that was active before this capture, on every exit path.
///
/// Restore-previous rather than clear: nesting one `capture_events` inside
/// another must not silently disable the outer one for the rest of its closure.
/// `Drop` rather than a plain reset: under `--test-threads=1` every test shares
/// the main thread, so a panicking `f` would otherwise leave a stale sink behind
/// and poison the NEXT test.
struct SinkGuard(Option<Sink>);

impl Drop for SinkGuard {
    fn drop(&mut self) {
        let previous = self.0.take();
        SINK.with(|s| *s.borrow_mut() = previous);
    }
}

/// Run `f` with this thread's `tracing` events captured, returning its value and
/// the events in emission order.
pub(crate) fn capture_events<T>(f: impl FnOnce() -> T) -> (T, Vec<Captured>) {
    install();
    let sink: Sink = Arc::new(Mutex::new(Vec::new()));
    let _guard = SinkGuard(SINK.with(|s| s.borrow_mut().replace(sink.clone())));
    let out = f();
    drop(_guard);
    let events = std::mem::take(&mut *sink.lock().unwrap());
    (out, events)
}

#[cfg(test)]
mod tests;
