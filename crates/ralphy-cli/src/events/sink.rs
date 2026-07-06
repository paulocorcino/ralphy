//! The run-time CloudEvents sink (ADR-0019, ADR-0024).
//!
//! The sink is a [`CloudEventsEngine`](delivery) fold over the shared
//! [`crate::delivery`] worker: [`new_events_layer`](layer::new_events_layer) installs
//! a [`crate::delivery::DeliveryLayer`] that decodes each `tracing` event into a
//! [`crate::runstate::RunEvent`] and pushes it onto the sink's bounded, drop-oldest
//! ring on the logging thread — never touching the network. The background worker
//! folds each event into a local [`crate::runstate::RunState`] (so the adapter events
//! that carry issue `0` resolve to the active issue), maps it to a CloudEvents
//! envelope, and POSTs it through the injectable [`crate::events::client::EventSink`]
//! with a short retry. The Layer ignores the sink's own `tracing` target so a runtime
//! `warn!` on a delivery drop can never feed back into the ring and loop.

mod delivery;
mod layer;
mod poller;

pub use crate::delivery::WorkerHandle as EventsHandle;
pub use delivery::try_start_sink;
pub use layer::{new_events_layer, new_queue};
