//! The Telegram bot, as a leaf crate: a blocking Bot API client and the
//! reading of the global Telegram config. It depends on no other `ralphy-*`
//! crate. Saving the config (owner-only) stays with the CLI.

pub mod client;
pub mod config;
