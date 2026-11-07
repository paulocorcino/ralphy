//! The Telegram state of this daemon (docs/adr/0085 D14): whether it sends
//! the console messages itself, sends nothing, or leaves them to the main
//! daemon of its host. It comes from the Telegram config, which is read again
//! each time it is asked for: the operator can run `ralphy telegram setup`
//! while the daemon runs. `GET /api/telegram` serves it and `ralphy doctor`
//! prints it.
//!
//! A main daemon that sends claims each of its peers (`POST
//! /api/telegram/main`, `peer::telegram_claim`). While a claim is less than
//! `CLAIM_HOLDS` old the peer is [`TelegramState::ThroughMain`], whatever its
//! own config says, so one request is one message.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, SystemTime};

use anyhow::Result;
use ralphy_telegram::config::TelegramConfig;

pub mod legacy;
pub mod store;

/// What this daemon does with a console request that waits for an operator
/// who is away. Matched exhaustively.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "state", rename_all = "kebab-case")]
pub enum TelegramState {
    /// No message is sent. `why` is `None` from an older daemon, which does
    /// not say (ADR-0085 D14, amendment of 2026-11-03).
    Off {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        why: Option<OffReason>,
    },
    /// This daemon sends the messages.
    Sends,
    /// The main daemon `main` sends the messages of this daemon.
    ThroughMain { main: String },
}

/// Why a daemon sends no message. A fixed word, never error text: a parse
/// error quotes the line it failed on, which may hold the token.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum OffReason {
    /// No settings file, or no token in it.
    NotSetUp,
    /// A token, but no chat captured yet.
    NoChat,
    /// The settings file cannot be read or parsed.
    Unreadable,
    /// The daemon store has no path.
    NoStore,
}

impl OffReason {
    /// The reason as a short phrase, after "off: ".
    pub fn text(self) -> &'static str {
        match self {
            OffReason::NotSetUp => "no bot is set up",
            OffReason::NoChat => "the bot has no chat yet",
            OffReason::Unreadable => "the Telegram settings file cannot be read",
            OffReason::NoStore => "the daemon store cannot be found",
        }
    }

    /// Whether the operator has something to fix: no bot at all is a choice.
    pub fn is_fault(self) -> bool {
        !matches!(self, OffReason::NotSetUp)
    }
}

impl TelegramState {
    /// The daemon is off for `why`.
    pub fn off(why: OffReason) -> Self {
        TelegramState::Off { why: Some(why) }
    }

    /// The state as a short line of text, after "Telegram: ".
    pub fn text(&self) -> String {
        self.text_naming(None)
    }

    /// [`TelegramState::text`], with the main daemon named by `main_name`
    /// when it is known, else by its id.
    pub fn text_naming(&self, main_name: Option<&str>) -> String {
        match self {
            TelegramState::Off { why: None } => "off".to_string(),
            TelegramState::Off { why: Some(why) } => format!("off: {}", why.text()),
            TelegramState::Sends => "sends from this machine".to_string(),
            TelegramState::ThroughMain { main } => {
                format!(
                    "sends through the main daemon {}",
                    main_name.unwrap_or(main)
                )
            }
        }
    }

    /// Whether the operator has something to fix.
    pub fn is_fault(&self) -> bool {
        match self {
            TelegramState::Off { why: Some(why) } => why.is_fault(),
            TelegramState::Off { why: None } => false,
            TelegramState::Sends | TelegramState::ThroughMain { .. } => false,
        }
    }
}

/// The reply of `GET /api/telegram`: this daemon's state and, in the joined
/// reply, the state of each peer and the peers that did not answer.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct FleetTelegram {
    #[serde(flatten)]
    pub state: TelegramState,
    /// The name of the main daemon, when the daemon that answers knows it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub main_name: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub peers: Vec<PeerTelegram>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unanswered: Vec<PeerName>,
}

impl FleetTelegram {
    /// The state of one daemon, with no peer.
    pub fn alone(state: TelegramState) -> Self {
        FleetTelegram {
            state,
            main_name: None,
            peers: Vec::new(),
            unanswered: Vec::new(),
        }
    }
}

/// One peer's state in the joined reply.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PeerTelegram {
    #[serde(flatten)]
    pub peer: PeerName,
    #[serde(flatten)]
    pub state: TelegramState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub main_name: Option<String>,
}

/// Who a peer is, as its descriptor names it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PeerName {
    pub daemon_id: String,
    #[serde(default)]
    pub name: String,
    pub environment: String,
}

/// The Telegram config of the store `store`: one file for the daemon, its runs
/// and `ralphy telegram` (ADR-0007 D2, amendment of 2026-11-03). A scratch
/// store has no bot because it has no such file.
pub fn config_path_in(store: &Path) -> PathBuf {
    store.join("telegram.toml")
}

/// The Telegram config of this machine's daemon store (`auth::store_dir`).
pub fn config_path() -> Result<PathBuf> {
    Ok(config_path_in(&crate::auth::store_dir()?))
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
    Ok(bot_or_reason(path)?.ok())
}

/// The bot of the config at `path`, or why there is none. The error is a
/// config that cannot be read.
pub(crate) fn bot_or_reason(path: &Path) -> Result<Result<Bot, OffReason>> {
    if path.as_os_str().is_empty() {
        return Ok(Err(OffReason::NoStore));
    }
    let Some(config) = TelegramConfig::load_from(path)? else {
        return Ok(Err(OffReason::NotSetUp));
    };
    let Some(chat_id) = config.chat_id else {
        return Ok(Err(OffReason::NoChat));
    };
    Ok(token_of(&config.token)
        .filter(|token| !token.trim().is_empty())
        .map(|token| Bot { token, chat_id })
        .ok_or(OffReason::NotSetUp))
}

/// `RALPHY_TELEGRAM_TOKEN` as the daemon had it when it started, read once.
/// The config file is still read on each call, so `ralphy telegram setup`
/// while the daemon runs takes effect; the environment cannot change from
/// outside a running process.
static ENV_TOKEN: OnceLock<Option<String>> = OnceLock::new();

/// Read `RALPHY_TELEGRAM_TOKEN` now and keep it. The boot path calls it
/// before the daemon serves; a later call keeps the first value.
pub fn capture_env_token() {
    ENV_TOKEN.get_or_init(read_env_token);
}

/// The non-blank value of `RALPHY_TELEGRAM_TOKEN`, if any.
fn read_env_token() -> Option<String> {
    std::env::var(ralphy_telegram::config::TOKEN_ENV)
        .ok()
        .filter(|token| !token.trim().is_empty())
}

/// The token of a config: `RALPHY_TELEGRAM_TOKEN` when it was set at boot,
/// else the file's. A unit test of this crate reads only the file's, so a
/// developer's real token in the environment never reaches a test.
#[cfg(not(test))]
fn token_of(stored: &str) -> Option<String> {
    ENV_TOKEN
        .get_or_init(read_env_token)
        .clone()
        .or_else(|| Some(stored.to_string()))
}

/// [`token_of`] in a unit test: the file's token only.
#[cfg(test)]
fn token_of(stored: &str) -> Option<String> {
    Some(stored.to_string())
}

/// The state of the config at `path`. A config that cannot be read is
/// logged and is `Off`. Only the outer message of the error is logged: the
/// parser quotes the line it failed on, which may hold the token.
pub fn state_at(path: &Path) -> TelegramState {
    match bot_or_reason(path) {
        Ok(Ok(_)) => TelegramState::Sends,
        Ok(Err(why)) => TelegramState::off(why),
        Err(e) => {
            tracing::warn!(error = %e, "the Telegram config could not be read");
            TelegramState::off(OffReason::Unreadable)
        }
    }
}

/// The last state a daemon logged, so its state is logged once at start and
/// again only when it changes.
#[derive(Debug, Default)]
pub(crate) struct StateLog {
    last: Option<TelegramState>,
}

impl StateLog {
    /// Log `state`, read from the config at `config`, when it is not the last
    /// one logged. The answer is whether it was logged.
    pub(crate) fn note(&mut self, state: TelegramState, config: &Path) -> bool {
        if self.last.as_ref() == Some(&state) {
            return false;
        }
        tracing::info!(config = %config.display(), "Telegram: {}", state.text());
        self.last = Some(state);
        true
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
            (
                "3 min 1 s, no config",
                &no_config,
                181,
                TelegramState::off(OffReason::NotSetUp),
            ),
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

    #[test]
    fn off_says_why() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let config = dir.path().join("telegram.toml");
        let write = |text: &str| std::fs::write(&config, text).expect("write the config");
        let off = TelegramState::off;
        assert_eq!(state_at(&config), off(OffReason::NotSetUp), "no file");
        write("token = \"123:abc\"\n");
        assert_eq!(state_at(&config), off(OffReason::NoChat), "no chat");
        write("token = \" \"\nchat_id = 42\n");
        assert_eq!(state_at(&config), off(OffReason::NotSetUp), "a blank token");
        write("token = \"123:abc\"\nchat_id = \"not a number\"\n");
        assert_eq!(
            state_at(&config),
            off(OffReason::Unreadable),
            "a broken file"
        );
        assert_eq!(state_at(Path::new("")), off(OffReason::NoStore), "no store");
        write("token = \"123:abc\"\nchat_id = 42\n");
        assert_eq!(state_at(&config), TelegramState::Sends);
    }

    #[test]
    fn a_state_is_logged_once_and_again_when_it_changes() {
        let mut log = StateLog::default();
        let config = Path::new("telegram.toml");
        assert!(
            log.note(TelegramState::off(OffReason::NotSetUp), config),
            "the first"
        );
        assert!(
            !log.note(TelegramState::off(OffReason::NotSetUp), config),
            "the same"
        );
        assert!(
            log.note(TelegramState::off(OffReason::NoChat), config),
            "a new reason"
        );
        assert!(log.note(TelegramState::Sends, config), "a change");
        assert!(!log.note(TelegramState::Sends, config));
    }

    /// `ralphy doctor` reads the joined reply the route writes.
    #[test]
    fn the_fleet_reply_reads_back() {
        let peer = |id: &str| PeerName {
            daemon_id: id.to_string(),
            name: format!("name-{id}"),
            environment: "WSL: Ubuntu".to_string(),
        };
        let fleet = FleetTelegram {
            state: TelegramState::Sends,
            main_name: None,
            peers: vec![PeerTelegram {
                peer: peer("a"),
                state: TelegramState::ThroughMain {
                    main: "m".to_string(),
                },
                main_name: Some("main".to_string()),
            }],
            unanswered: vec![peer("b")],
        };
        let json = serde_json::to_value(&fleet).expect("JSON");
        assert_eq!(json["peers"][0]["state"], "through-main", "{json}");
        assert_eq!(json["peers"][0]["daemon_id"], "a", "{json}");
        assert_eq!(json["unanswered"][0]["name"], "name-b", "{json}");
        let back: FleetTelegram = serde_json::from_value(json).expect("read back");
        assert_eq!(back, fleet);
        let alone: FleetTelegram =
            serde_json::from_str(r#"{"state":"off","why":"no-chat"}"#).expect("one daemon");
        assert_eq!(
            alone,
            FleetTelegram::alone(TelegramState::off(OffReason::NoChat))
        );
    }

    /// An older daemon says `{"state":"off"}` and an older main reads the new
    /// reply: neither breaks the other.
    #[test]
    fn an_older_daemons_off_still_parses() {
        let older: TelegramState =
            serde_json::from_str(r#"{"state":"off"}"#).expect("an older reply");
        assert_eq!(older, TelegramState::Off { why: None });
        assert_eq!(older.text(), "off");
        let newer = serde_json::to_value(TelegramState::off(OffReason::NoChat)).expect("JSON");
        assert_eq!(
            newer,
            serde_json::json!({ "state": "off", "why": "no-chat" })
        );
        let unknown = serde_json::to_value(TelegramState::Off { why: None }).expect("JSON");
        assert_eq!(
            unknown,
            serde_json::json!({ "state": "off" }),
            "no `why: null`"
        );
    }
}
