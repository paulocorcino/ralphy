//! The sender as the attention tick runs it (ADR-0085 D14): a [`Notifier`]
//! reads the claim and the config on each tick, starts one send at a time,
//! and builds its transport for the bot's token.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::SystemTime;

#[cfg(test)]
use ralphy_telegram::client::Transport;

use super::{deliver, AnyTransport, Context, Delivery, Sender};
use crate::attention::AttentionRequest;
use crate::presence_board;
use crate::telegram_state::{self, OffReason, SharedClaim, TelegramState};

/// What builds the transport for a token.
pub(crate) type Transports = Arc<dyn Fn(String) -> AnyTransport + Send + Sync>;

/// The transports of a [`Notifier`]: the Telegram API. In a unit test of
/// this crate every router and every notifier gets [`NoApi`] instead, so no
/// test can reach the API, whatever config or token it has.
#[cfg(not(test))]
pub(crate) fn api_transports() -> Transports {
    Arc::new(|token| AnyTransport(Box::new(ralphy_telegram::client::UreqTransport::new(token))))
}

/// [`api_transports`] in a unit test: [`NoApi`].
#[cfg(test)]
pub(crate) fn api_transports() -> Transports {
    Arc::new(|_token| AnyTransport(Box::new(NoApi)))
}

/// The transport of a unit test that sets none: every call fails.
#[cfg(test)]
pub(crate) struct NoApi;

#[cfg(test)]
impl Transport for NoApi {
    fn get(&self, method: &str) -> anyhow::Result<serde_json::Value> {
        anyhow::bail!("a unit test reaches no Telegram API ({method})")
    }

    fn post(&self, method: &str, _body: serde_json::Value) -> anyhow::Result<serde_json::Value> {
        anyhow::bail!("a unit test reaches no Telegram API ({method})")
    }
}

/// The sender as the attention tick runs it: the config path, the presence
/// board, the claim of a main daemon on this one, the peers, this daemon's
/// id, and the send in flight. A tick starts no send while one is in flight,
/// so a slow network holds back neither the tick nor a second copy of a
/// message.
pub(crate) struct Notifier {
    config: PathBuf,
    board: presence_board::Shared,
    claim: SharedClaim,
    peers_dir: PathBuf,
    daemon_id: Option<String>,
    pub(super) transport: Transports,
    pub(super) sender: Sender,
    pub(super) flight: Option<tokio::task::JoinHandle<Delivery>>,
    log: telegram_state::StateLog,
}

impl Notifier {
    pub(crate) fn new(
        config: PathBuf,
        (board, claim): (presence_board::Shared, SharedClaim),
        peers_dir: PathBuf,
        daemon_id: Option<String>,
    ) -> Self {
        Notifier {
            config,
            board,
            claim,
            peers_dir,
            daemon_id,
            transport: api_transports(),
            sender: Sender::default(),
            flight: None,
            log: telegram_state::StateLog::default(),
        }
    }

    /// Whether the operator is present now, at a page of this daemon or of a
    /// peer (`peers_present`), and the origin of the last page of this
    /// daemon. A poisoned board is logged and reads as "not present".
    fn presence(&self, peers_present: &[bool], now: SystemTime) -> (bool, Option<String>) {
        match self.board.lock() {
            Ok(board) => (board.present_with(peers_present, now), board.last_origin()),
            Err(e) => {
                tracing::warn!(error = %e, "the presence board is poisoned");
                (false, None)
            }
        }
    }

    /// Keep what the send in flight did, once it ended. `false` while it
    /// runs.
    async fn landed(&mut self) -> bool {
        if self.flight.as_ref().is_some_and(|f| !f.is_finished()) {
            return false;
        }
        if let Some(flight) = self.flight.take() {
            match flight.await {
                Ok(delivery) => {
                    self.sender.finish(delivery);
                }
                Err(e) => tracing::warn!(error = %e, "the Telegram send stopped"),
            }
        }
        true
    }

    /// Whether this daemon sends at `now`: no main daemon's claim holds, and
    /// its config has a bot. A config with no bot, or one that cannot be
    /// read, is kept as the sender keeps a send that found it so. The state
    /// is logged when it changes.
    async fn sends(&mut self, now: SystemTime) -> bool {
        if let Some(main) = telegram_state::holder_at(&self.claim, now) {
            self.log
                .note(TelegramState::ThroughMain { main }, &self.config);
            return false;
        }
        let config = self.config.clone();
        let why_off = tokio::task::spawn_blocking(move || {
            telegram_state::bot_or_reason(&config).map(Result::err)
        })
        .await;
        match why_off {
            Ok(Ok(None)) => {
                self.log.note(TelegramState::Sends, &self.config);
                true
            }
            Ok(Ok(Some(why))) => {
                self.log.note(TelegramState::off(why), &self.config);
                self.sender.finish(Delivery::Off);
                false
            }
            Ok(Err(e)) => {
                self.log
                    .note(TelegramState::off(OffReason::Unreadable), &self.config);
                self.sender.finish(Delivery::NoConfig(e.to_string()));
                false
            }
            Err(e) => {
                tracing::warn!(error = %e, "the Telegram config read stopped");
                false
            }
        }
    }

    /// One tick: keep what the last send did, then start the send of the
    /// messages due for `requests` and for the console requests of the
    /// peers.
    pub(crate) async fn tick(&mut self, requests: &[AttentionRequest]) {
        self.tick_at(requests, SystemTime::now()).await;
    }

    /// [`Notifier::tick`] at `now`.
    pub(crate) async fn tick_at(&mut self, requests: &[AttentionRequest], now: SystemTime) {
        if !self.landed().await || !self.sends(now).await {
            return;
        }
        let peers = crate::attention::tick::peer_requests(self.peers_dir.clone()).await;
        let (present, origin) = self.presence(&peers.present, now);
        let ctx = Context {
            daemon_id: self.daemon_id.clone(),
            origin,
            runs_of: peers.bot_less,
            unanswered: peers.unanswered,
        };
        let all: Vec<AttentionRequest> = requests.iter().cloned().chain(peers.requests).collect();
        let messages = self.sender.next(&all, present, now, &ctx);
        if messages.is_empty() {
            return;
        }
        let (config, transport) = (self.config.clone(), self.transport.clone());
        self.flight = Some(tokio::task::spawn_blocking(move || {
            deliver(&config, |token| transport(token), messages)
        }));
    }
}
