//! The Telegram bot, as a leaf crate: a blocking Bot API client and the
//! reading of the global Telegram config. It depends on no other `ralphy-*`
//! crate. Saving the config (owner-only) stays with the CLI.
#![cfg_attr(
    test,
    expect(
        clippy::disallowed_methods,
        reason = "nextest runs each test in its own process, so a test may change its environment"
    )
)]

pub mod client;
pub mod config;
