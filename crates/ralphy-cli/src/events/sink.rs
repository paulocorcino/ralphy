//! The run-time CloudEvents sink Layer + worker (ADR-0019).
//!
//! [`EventsLayer`] mirrors the Telegram [`NotifierLayer`](crate::telegram::notifier)
//! exactly: it decodes each `tracing` event into a [`crate::runstate::RunEvent`] and
//! pushes it onto a bounded, drop-oldest ring on the logging thread — never touching
//! the network. One background worker ([`delivery::run_sender`]) drains the ring,
//! folds each event into a local [`crate::runstate::RunState`] (so the adapter
//! events that carry issue `0` resolve to the active issue), maps it to a
//! CloudEvents envelope, and POSTs it through the injectable
//! [`crate::events::client::EventSink`]. The Layer ignores the sink's own `tracing`
//! target so a runtime `warn!` on a delivery drop can never feed back into the ring
//! and loop.

mod delivery;
mod layer;
mod poller;

pub use delivery::{try_start_sink, EventsHandle};
pub use layer::{new_queue, EventsLayer};
