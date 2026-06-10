//! The Telegram notifier slice (ADR-0007): global config store, a blocking Bot
//! API client, and the `ralphy telegram` command group. This is the onboarding
//! and transport spine; the run-time notifier Layer (D1/D6) lands in a later
//! slice.

pub mod client;
pub mod config;

use std::time::Duration;

use anyhow::{Context, Result};
use clap::Subcommand;

use client::{detect_chat_id, BotClient, UreqTransport};
use config::{effective_token, masked_token, TelegramConfig};

/// The `ralphy telegram` command group (ADR-0007 D2).
#[derive(Subcommand)]
pub enum TelegramCommand {
    /// Store the bot token, then capture the chat from an inbound `/start`.
    Setup {
        /// The bot token from BotFather. Falls back to `RALPHY_TELEGRAM_TOKEN`.
        #[arg(long)]
        token: Option<String>,
    },
    /// Send a ping to the configured chat to confirm the token and chat.
    Test,
    /// Show the configured chat and a masked token.
    Status,
    /// Remove the stored config.
    Disable,
}

/// How long `setup` polls `getUpdates` for the operator's `/start`.
const SETUP_POLL_ATTEMPTS: u32 = 30;
const SETUP_POLL_INTERVAL: Duration = Duration::from_secs(2);

/// Dispatch a `telegram` subcommand.
pub fn run(cmd: TelegramCommand) -> Result<()> {
    match cmd {
        TelegramCommand::Setup { token } => setup(token),
        TelegramCommand::Test => test(),
        TelegramCommand::Status => status(),
        TelegramCommand::Disable => disable(),
    }
}

/// Resolve the token to use, preferring `RALPHY_TELEGRAM_TOKEN` over `stored`,
/// erroring with guidance when neither supplies one.
fn require_token(stored: Option<&str>) -> Result<String> {
    effective_token(stored).context(
        "no Telegram token: pass `--token`, set RALPHY_TELEGRAM_TOKEN, or run `telegram setup`",
    )
}

fn setup(token_arg: Option<String>) -> Result<()> {
    let stored = TelegramConfig::load()?;
    let token = match token_arg {
        Some(t) if !t.trim().is_empty() => t,
        _ => require_token(stored.as_ref().map(|c| c.token.as_str()))?,
    };

    let client = BotClient::new(UreqTransport::new(token.clone()));
    let me = client
        .get_me()
        .context("getMe failed — check the bot token")?;
    let bot = me
        .get("username")
        .and_then(|v| v.as_str())
        .unwrap_or("your bot");
    println!("Token accepted for @{bot}.");
    println!("Now open Telegram and send /start to @{bot}. Waiting…");

    let mut chat_id = None;
    for _ in 0..SETUP_POLL_ATTEMPTS {
        let updates = client.get_updates()?;
        if let Some(id) = detect_chat_id(&updates) {
            chat_id = Some(id);
            break;
        }
        std::thread::sleep(SETUP_POLL_INTERVAL);
    }

    let chat_id = chat_id.context(
        "no /start received in time — re-run `telegram setup` and send /start to the bot",
    )?;
    TelegramConfig {
        token,
        chat_id: Some(chat_id),
    }
    .save()?;
    println!("Captured chat {chat_id}. Telegram notifications are configured.");
    Ok(())
}

fn test() -> Result<()> {
    let cfg = TelegramConfig::load()?
        .context("Telegram is not configured — run `telegram setup` first")?;
    let chat_id = cfg
        .chat_id
        .context("no chat configured — run `telegram setup` to capture one")?;
    let token = require_token(Some(&cfg.token))?;
    let client = BotClient::new(UreqTransport::new(token));
    client
        .send_message(chat_id, "Ralphy test message ✓")
        .context("sendMessage failed")?;
    println!("Sent a test message to chat {chat_id}.");
    Ok(())
}

fn status() -> Result<()> {
    match TelegramConfig::load()? {
        None => {
            println!("Telegram: not configured. Run `telegram setup`.");
        }
        Some(cfg) => {
            let token = effective_token(Some(&cfg.token)).unwrap_or_default();
            match cfg.chat_id {
                Some(id) => println!("Telegram: configured (chat {id}, notifications on)."),
                None => {
                    println!("Telegram: token stored but no chat captured — run `telegram setup`.")
                }
            }
            println!("Token: {}", masked_token(&token));
        }
    }
    Ok(())
}

fn disable() -> Result<()> {
    if TelegramConfig::load()?.is_none() {
        println!("Telegram was not configured; nothing to remove.");
        return Ok(());
    }
    TelegramConfig::delete()?;
    println!("Telegram config removed.");
    Ok(())
}
