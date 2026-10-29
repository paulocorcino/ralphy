//! The Telegram state of this daemon (docs/adr/0085 D14): whether it sends
//! the console messages itself, sends nothing, or leaves them to the main
//! daemon of its host. It comes from the Telegram config, which is read again
//! each time it is asked for: the operator can run `ralphy telegram setup`
//! while the daemon runs. `GET /api/telegram` serves it and `ralphy doctor`
//! prints it.
//!
//! A main daemon that sends claims each of its peers (`POST
//! /api/telegram/main`, `peer::telegram_claim`). While a claim is less than
//! [`CLAIM_HOLDS`] old the peer is [`TelegramState::ThroughMain`], whatever its
//! own config says, so one request is one message.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use anyhow::Result;
use ralphy_telegram::config::{effective_token, TelegramConfig};

/// What this daemon does with a console request that waits for an operator
/// who is away. Matched exhaustively.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
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

/// How long the claim of a main daemon holds. The main claims every minute,
/// so a peer whose main stopped sends by itself again after three minutes.
pub(crate) const CLAIM_HOLDS: Duration = Duration::from_secs(3 * 60);

/// The last claim of a main daemon on this daemon, in memory only: after a
/// restart the main's next claim comes within a minute.
#[derive(Debug, Default)]
pub(crate) struct Claim {
    last: Option<(String, SystemTime)>,
}

/// The claim, shared by `POST /api/telegram/main`, `GET /api/telegram` and
/// the attention tick.
pub(crate) type SharedClaim = Arc<Mutex<Claim>>;

impl Claim {
    /// The main daemon `main` claimed this daemon at `at`.
    pub(crate) fn take(&mut self, main: String, at: SystemTime) {
        self.last = Some((main, at));
    }

    /// The main daemon whose claim holds at `now`. A claim the clock puts
    /// after `now` is a fresh one.
    pub(crate) fn holder(&self, now: SystemTime) -> Option<&str> {
        let (main, at) = self.last.as_ref()?;
        let age = now.duration_since(*at).unwrap_or(Duration::ZERO);
        (age < CLAIM_HOLDS).then_some(main.as_str())
    }
}

/// The main daemon whose claim on this daemon holds at `now`. A poisoned
/// lock is logged and holds no claim.
pub(crate) fn holder_at(claim: &SharedClaim, now: SystemTime) -> Option<String> {
    match claim.lock() {
        Ok(claim) => claim.holder(now).map(str::to_string),
        Err(e) => {
            tracing::warn!(error = %e, "the Telegram claim is poisoned");
            None
        }
    }
}

/// The state of this daemon at `now`: a claim that holds wins over the
/// config at `path`, so a peer with its own bot sends no second message.
pub(crate) fn state_with(path: &Path, claim: &SharedClaim, now: SystemTime) -> TelegramState {
    match holder_at(claim, now) {
        Some(main) => TelegramState::ThroughMain { main },
        None => state_at(path),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(secs: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000 + secs)
    }

    #[test]
    fn a_claim_expires_after_three_minutes() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let with_bot = dir.path().join("telegram.toml");
        std::fs::write(
            &with_bot,
            "token = \"123:abc\"
chat_id = 42
",
        )
        .expect("write the config");
        let no_config = dir.path().join("none.toml");
        let through = TelegramState::ThroughMain {
            main: "m".to_string(),
        };
        // (case, config, seconds since the claim, state)
        let rows = [
            ("2 min 59 s, a config", &with_bot, 179, through),
            ("3 min 1 s, a config", &with_bot, 181, TelegramState::Sends),
            ("3 min 1 s, no config", &no_config, 181, TelegramState::Off),
        ];
        for (case, config, age, want) in rows {
            let claim = SharedClaim::default();
            claim
                .lock()
                .expect("the claim lock")
                .take("m".to_string(), at(0));
            assert_eq!(state_with(config, &claim, at(age)), want, "{case}");
        }
    }
}
