//! The CloudEvents HTTP event sink (ADR-0019).
//!
//! A second, independent `tracing_subscriber::Layer` ([`sink::EventsLayer`])
//! decodes the run's event bus through the canonical `event_to_runevent` decoder
//! and POSTs each event as a CloudEvents 1.0 structured-mode JSON envelope to a
//! per-repo configured URL — asynchronously, best-effort, never blocking the run.
//! It mirrors the Telegram notifier (ADR-0007) exactly: a bounded drop-oldest ring
//! filled by the Layer on the logging thread, drained by one background worker that
//! maps each [`crate::runstate::RunEvent`] to a CloudEvent and delivers it with a
//! short retry. The contract a consumer programs against is `docs/events.md`.

pub mod client;
pub mod config;
pub mod emitter;
pub mod envelope;
pub mod sink;
