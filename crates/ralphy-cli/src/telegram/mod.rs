//! The Telegram notifier slice (ADR-0007): global config store, a blocking Bot
//! API client, and the `ralphy telegram` command group. This is the onboarding
//! and transport spine; the run-time notifier Layer (D1/D6) lands in a later
//! slice.

pub mod config;
