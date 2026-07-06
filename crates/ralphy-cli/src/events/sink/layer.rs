//! The sink's ring constructor and `tracing` Layer factory (ADR-0019, ADR-0024).
//! Both the ring and the Layer are the shared [`crate::delivery`] types; this module
//! only pins the sink's ~1000-event bound and its self-target loop-guard marker.

use std::sync::Arc;

use crate::delivery::{DeliveryLayer, EventQueue};

/// The sink ring's capacity (ADR-0019: bounded ~1000, drop-oldest).
const QUEUE_CAPACITY: usize = 1000;

/// The substring identifying the sink's own `tracing` target, so a drop `warn!`
/// from the worker never feeds back into the Layer and loops (ADR-0019).
const SELF_TARGET_MARKER: &str = "events::sink";

/// A sink ring with the ADR-0019 ~1000-event bound.
pub fn new_queue() -> Arc<EventQueue> {
    Arc::new(EventQueue::with_capacity(QUEUE_CAPACITY))
}

/// The sink's `tracing` Layer: a [`DeliveryLayer`] over the sink ring, tagged with
/// the sink's own target so a runtime `warn!` never feeds back into the ring.
pub fn new_events_layer(queue: Arc<EventQueue>) -> DeliveryLayer {
    DeliveryLayer::new(queue, SELF_TARGET_MARKER)
}
