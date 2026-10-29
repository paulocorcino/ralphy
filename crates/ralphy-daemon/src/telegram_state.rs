//! The Telegram state of this daemon (docs/adr/0085 D14): whether it sends
//! the console messages itself, sends nothing, or leaves them to the main
//! daemon of its host. It comes from the Telegram config, which is read again
//! each time it is asked for: the operator can run `ralphy telegram setup`
//! while the daemon runs. `GET /api/telegram` serves it and `ralphy doctor`
//! prints it.

use std::path::{Path, PathBuf};

use anyhow::Result;
use ralphy_telegram::config::{effective_token, TelegramConfig};

/// What this daemon does with a console request that waits for an operator
/// who is away. Matched exhaustively.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "state", rename_all = "kebab-case")]
pub enum TelegramState {
    /// No bot is set up on this machine: no message is sent.
    Off,
    /// This daemon sends the messages.
    Sends,
    /// The main daemon `main` sends the messages of this daemon.
    ThroughMain { main: String },
}

impl TelegramState {
    /// The state as a short line of text, after "Telegram: ".
    pub fn text(&self) -> String {
        match self {
            TelegramState::Off => "off".to_string(),
            TelegramState::Sends => "sends from this machine".to_string(),
            TelegramState::ThroughMain { main } => {
                format!("sends through the main daemon {main}")
            }
        }
    }
}

/// Where this daemon reads the Telegram config: the file `ralphy telegram
/// setup` writes, or `telegram.toml` in a scratch store
/// (`$RALPHY_DAEMON_DIR`), so a daemon a test starts never sends with the
/// operator's bot.
pub fn config_path() -> Result<PathBuf> {
    match std::env::var_os("RALPHY_DAEMON_DIR") {
        Some(dir) => Ok(PathBuf::from(dir).join("telegram.toml")),
        None => TelegramConfig::config_path(),
    }
}

/// The bot this daemon sends with. It has no `Debug`: the token never goes
/// into a log (ADR-0072 D7).
pub struct Bot {
    pub(crate) token: String,
    pub(crate) chat_id: i64,
}

/// The bot of the config at `path`, when it can send: a token (the
/// environment's, or the file's) and a chat. An empty path is no config, so
/// a daemon built with no config path sends nothing.
pub fn bot_at(path: &Path) -> Result<Option<Bot>> {
    if path.as_os_str().is_empty() {
        return Ok(None);
    }
    let Some(config) = TelegramConfig::load_from(path)? else {
        return Ok(None);
    };
    let Some(chat_id) = config.chat_id else {
        return Ok(None);
    };
    Ok(effective_token(Some(&config.token))
        .filter(|token| !token.trim().is_empty())
        .map(|token| Bot { token, chat_id }))
}

/// The state of the config at `path`. A config that cannot be read is
/// logged and is `Off`. Only the outer message of the error is logged: the
/// parser quotes the line it failed on, which may hold the token.
pub fn state_at(path: &Path) -> TelegramState {
    match bot_at(path) {
        Ok(Some(_)) => TelegramState::Sends,
        Ok(None) => TelegramState::Off,
        Err(e) => {
            tracing::warn!(error = %e, "the Telegram config could not be read");
            TelegramState::Off
        }
    }
}
