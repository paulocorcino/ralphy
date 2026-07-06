//! The `tracing` Layer half of the sink (ADR-0019): decodes each consumed event
//! into a [`crate::runstate::RunEvent`] and pushes it onto the shared ring, doing
//! no I/O on the logging thread.

use std::sync::Arc;

use tracing::{Event, Subscriber};
use tracing_subscriber::layer::{Context, Layer};

use crate::runstate::{event_to_runevent, EventFields};
use crate::telegram::notifier::EventQueue;

/// The sink ring's capacity (ADR-0019: bounded ~1000, drop-oldest).
pub(super) const QUEUE_CAPACITY: usize = 1000;

/// The substring identifying the sink's own `tracing` target, so a drop `warn!`
/// from the worker never feeds back into the Layer and loops (ADR-0019).
pub(super) const SELF_TARGET_MARKER: &str = "events::sink";

/// A sink ring with the ADR-0019 ~1000-event bound.
pub fn new_queue() -> Arc<EventQueue> {
    Arc::new(EventQueue::with_capacity(QUEUE_CAPACITY))
}

/// A `tracing` Layer that enqueues each consumed event as a [`crate::runstate::RunEvent`]
/// onto the sink's own ring. Does no I/O on the logging thread — only
/// `event_to_runevent` + a ring push, so an unreachable endpoint never touches the
/// run's wall-clock.
pub struct EventsLayer {
    queue: Arc<EventQueue>,
}

impl EventsLayer {
    /// Wrap the shared sink ring the worker drains.
    pub fn new(queue: Arc<EventQueue>) -> Self {
        EventsLayer { queue }
    }
}

impl<S: Subscriber> Layer<S> for EventsLayer {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let target = event.metadata().target();
        // Ignore the sink's own events so a runtime warn! cannot loop back in.
        if target.contains(SELF_TARGET_MARKER) {
            return;
        }
        let mut fields = EventFields {
            level: *event.metadata().level(),
            ..Default::default()
        };
        event.record(&mut fields);
        if let Some(run_event) = event_to_runevent(target, &fields.message, &fields) {
            self.queue.push(run_event);
        }
    }
}
