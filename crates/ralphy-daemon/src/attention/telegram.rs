//! The Telegram message of a console request (docs/adr/0085 D14, D16). A
//! console request that is still open [`DELAY`] after it opened, while the
//! operator is not present at a page of this daemon, is one message with a
//! link to the console. A silent console sends nothing, and neither does a
//! run: a run sends its own. Telegram only sends: nothing reads a reply.
//!
//! [`Sender`] keeps the ids already sent. A send that fails is logged once
//! and tried again on the next tick. The send itself ([`deliver`]) is
//! blocking and reads the config each time, so the tick runs it off the
//! runtime ([`Notifier`]).
//!
//! A daemon that sends also sends the console requests of its peers, which
//! it reads from each peer's own list, and the operator is present when a
//! page of the daemon or of a peer is. A peer whose main daemon claimed it
//! (`telegram_state::Claim`) sends nothing: its main sends for it.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use ralphy_telegram::client::{BotClient, Transport};

use super::{AttentionRequest, Kind};
use crate::presence_board;
use crate::session::Agent;
use crate::telegram_state::{self, SharedClaim};

/// How long a console request is open before it is sent.
pub(crate) const DELAY: Duration = Duration::from_secs(120);

/// One message to send, for the request `id`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Message {
    pub(crate) id: String,
    pub(crate) text: String,
}

/// What a message names besides the request: this daemon's id and the origin
/// of the operator's last page (the link).
#[derive(Debug, Default)]
pub(crate) struct Context {
    pub(crate) daemon_id: Option<String>,
    pub(crate) origin: Option<String>,
}

/// What a console request of `kind` says it is, or `None` for a request that
/// is not sent: a silent console has no state to tell, and a run sends its
/// own message.
fn what(kind: Kind) -> Option<&'static str> {
    match kind {
        Kind::ConsoleWaiting => Some("is waiting"),
        Kind::ConsoleDone => Some("has finished its turn"),
        Kind::ConsoleSilent | Kind::RunWaiting | Kind::RunFinished => None,
    }
}

/// The vendor's name as a person writes it; an unknown one as it came.
fn vendor_name(vendor: &str) -> &str {
    match Agent::from_query(vendor) {
        Some(Agent::Claude) => "Claude",
        Some(Agent::Codex) => "Codex",
        Some(Agent::Copilot) => "Copilot",
        Some(Agent::Cursor) => "Cursor",
        Some(Agent::Gemini) => "Gemini",
        Some(Agent::Kimi) => "Kimi",
        Some(Agent::OpenCode) => "OpenCode",
        None => vendor,
    }
}

/// The text of the message of `request`: what waits, where, and the link to
/// the console when the origin of a page and this daemon's id are known.
fn text_of(request: &AttentionRequest, what: &str, ctx: &Context) -> String {
    let console = request.console.as_deref().unwrap_or_default();
    let mut text = format!("{} console", vendor_name(&request.vendor));
    if let Some(name) = &request.name {
        text.push_str(&format!(" \"{name}\""));
    }
    if let Some(repo) = &request.repo {
        text.push_str(&format!(" in {repo}"));
    }
    text.push(' ');
    text.push_str(what);
    if let Some(detail) = &request.detail {
        text.push_str(": ");
        text.push_str(&short(detail));
    }
    if let Some(link) = link_of(console, ctx) {
        text.push('\n');
        text.push_str(&link);
    }
    text
}

/// The link that opens `console`: `{origin}/?console={daemon_id}/{session}`.
/// A peer's request names its console `{peer daemon_id}/{session}` already;
/// a request of this daemon names its session only.
fn link_of(console: &str, ctx: &Context) -> Option<String> {
    let origin = ctx.origin.as_ref()?;
    let target = if console.contains('/') {
        console.to_string()
    } else {
        format!("{}/{console}", ctx.daemon_id.as_ref()?)
    };
    Some(format!("{origin}/?console={target}"))
}

/// The most of a detail a message carries, in characters. Telegram refuses a
/// text over 4096 characters, and a refused message would be tried forever.
const MAX_DETAIL_CHARS: usize = 300;

/// `detail`, cut to [`MAX_DETAIL_CHARS`].
fn short(detail: &str) -> String {
    match detail.char_indices().nth(MAX_DETAIL_CHARS) {
        Some((at, _)) => format!("{}...", &detail[..at]),
        None => detail.to_string(),
    }
}

/// How long `request` has been open at `now`; `None` when its `since` is not
/// an RFC 3339 time.
fn age(request: &AttentionRequest, now: SystemTime) -> Option<Duration> {
    let since = chrono::DateTime::parse_from_rfc3339(&request.since).ok()?;
    let since = SystemTime::from(since.with_timezone(&chrono::Utc));
    Some(now.duration_since(since).unwrap_or(Duration::ZERO))
}

/// The messages due at `now`: one per console request of `requests` that
/// waits or finished a turn, is open at least [`DELAY`], and is not in
/// `sent`; none while the operator is `present`.
pub(crate) fn due(
    requests: &[AttentionRequest],
    sent: &HashSet<String>,
    present: bool,
    now: SystemTime,
    ctx: &Context,
) -> Vec<Message> {
    if present {
        return Vec::new();
    }
    requests
        .iter()
        .filter(|request| !sent.contains(&request.id))
        .filter_map(|request| {
            let what = what(request.kind)?;
            (age(request, now)? >= DELAY).then(|| Message {
                id: request.id.clone(),
                text: text_of(request, what, ctx),
            })
        })
        .collect()
}

/// What one [`deliver`] did.
#[derive(Debug)]
pub(crate) enum Delivery {
    /// No bot is set up: nothing was sent, and the messages stay due.
    Off,
    /// The config could not be read; only its outer message.
    NoConfig(String),
    /// Each message's id, and the error of a send that failed.
    Sent(Vec<(String, Result<(), String>)>),
}

/// Send `messages` with the bot of the config at `config`, through the
/// transport `transport` builds for its token. Blocking. A config with no
/// bot sends nothing.
pub(crate) fn deliver<T: Transport>(
    config: &Path,
    transport: impl FnOnce(String) -> T,
    messages: Vec<Message>,
) -> Delivery {
    let bot = match telegram_state::bot_at(config) {
        Ok(Some(bot)) => bot,
        Ok(None) => return Delivery::Off,
        Err(e) => return Delivery::NoConfig(e.to_string()),
    };
    let client = BotClient::new(transport(bot.token));
    Delivery::Sent(
        messages
            .into_iter()
            .map(|message| {
                let sent = client
                    .send_message(bot.chat_id, &message.text)
                    .map(drop)
                    .map_err(|e| format!("{e:#}"));
                (message.id, sent)
            })
            .collect(),
    )
}

/// The ids sent, and what was already logged.
#[derive(Debug, Default)]
pub(crate) struct Sender {
    sent: HashSet<String>,
    failing: HashSet<String>,
    config_error: Option<String>,
}

impl Sender {
    /// [`Sender::next_with`], with every peer answered.
    #[cfg(test)]
    pub(crate) fn next(
        &mut self,
        requests: &[AttentionRequest],
        present: bool,
        now: SystemTime,
        ctx: &Context,
    ) -> Vec<Message> {
        self.next_with(requests, &[], present, now, ctx)
    }

    /// The messages due now. An id no longer in `requests` is forgotten, so
    /// a console that waits again later is sent again; the ids of a peer in
    /// `unanswered` are kept, so a peer that misses one tick is not sent
    /// again.
    pub(crate) fn next_with(
        &mut self,
        requests: &[AttentionRequest],
        unanswered: &[String],
        present: bool,
        now: SystemTime,
        ctx: &Context,
    ) -> Vec<Message> {
        let open: HashSet<&str> = requests.iter().map(|r| r.id.as_str()).collect();
        let keep = |id: &String| {
            open.contains(id.as_str())
                || unanswered.iter().any(|peer| {
                    id.strip_prefix(peer.as_str())
                        .is_some_and(|rest| rest.starts_with('/'))
                })
        };
        self.sent.retain(keep);
        self.failing.retain(keep);
        due(requests, &self.sent, present, now, ctx)
    }

    /// Keep what `delivery` did: a sent id is not sent again, and a failure
    /// is logged the first time only. The answer is the warnings logged.
    pub(crate) fn finish(&mut self, delivery: Delivery) -> Vec<String> {
        match delivery {
            Delivery::Off => {
                self.config_error = None;
                Vec::new()
            }
            Delivery::NoConfig(error) => self.config_failed(error).into_iter().collect(),
            Delivery::Sent(outcomes) => {
                self.config_error = None;
                outcomes
                    .into_iter()
                    .filter_map(|(id, outcome)| self.outcome(id, outcome))
                    .collect()
            }
        }
    }

    /// The config could not be read: logged when the error is new.
    fn config_failed(&mut self, error: String) -> Option<String> {
        if self.config_error.as_ref() == Some(&error) {
            return None;
        }
        tracing::warn!(%error, "the Telegram config could not be read");
        self.config_error = Some(error.clone());
        Some(error)
    }

    /// Keep the outcome of the send of `id`; the warning, when one is logged.
    fn outcome(&mut self, id: String, outcome: Result<(), String>) -> Option<String> {
        match outcome {
            Ok(()) => {
                self.failing.remove(&id);
                self.sent.insert(id);
                None
            }
            Err(error) => {
                if !self.failing.insert(id.clone()) {
                    return None;
                }
                tracing::warn!(request = %id, %error, "the Telegram message could not be sent; it is tried again");
                Some(error)
            }
        }
    }
}

/// A transport of the sender, whatever its type: the Telegram API, or a
/// test's fake.
pub(crate) struct AnyTransport(pub(crate) Box<dyn Transport + Send>);

impl Transport for AnyTransport {
    fn get(&self, method: &str) -> anyhow::Result<serde_json::Value> {
        self.0.get(method)
    }

    fn post(&self, method: &str, body: serde_json::Value) -> anyhow::Result<serde_json::Value> {
        self.0.post(method, body)
    }
}

/// What builds the transport for a token.
pub(crate) type Transports = Arc<dyn Fn(String) -> AnyTransport + Send + Sync>;

/// The transports of a [`Notifier`]: the Telegram API. In a unit test of
/// this crate every router and every notifier gets [`NoApi`] instead, so no
/// test can reach the API, whatever config or token it has.
#[cfg(not(test))]
fn api_transports() -> Transports {
    Arc::new(|token| AnyTransport(Box::new(ralphy_telegram::client::UreqTransport::new(token))))
}

/// [`api_transports`] in a unit test: [`NoApi`].
#[cfg(test)]
fn api_transports() -> Transports {
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
    transport: Transports,
    sender: Sender,
    flight: Option<tokio::task::JoinHandle<Delivery>>,
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
    /// read, is kept as the sender keeps a send that found it so.
    async fn sends(&mut self, now: SystemTime) -> bool {
        if telegram_state::holder_at(&self.claim, now).is_some() {
            return false;
        }
        let config = self.config.clone();
        let bot = tokio::task::spawn_blocking(move || {
            telegram_state::bot_at(&config).map(|bot| bot.is_some())
        })
        .await;
        match bot {
            Ok(Ok(true)) => true,
            Ok(Ok(false)) => {
                self.sender.finish(Delivery::Off);
                false
            }
            Ok(Err(e)) => {
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
        let peers = super::tick::peer_requests(self.peers_dir.clone()).await;
        let (present, origin) = self.presence(&peers.present, now);
        let ctx = Context {
            daemon_id: self.daemon_id.clone(),
            origin,
        };
        let all: Vec<AttentionRequest> = requests.iter().cloned().chain(peers.requests).collect();
        let messages = self
            .sender
            .next_with(&all, &peers.unanswered, present, now, &ctx);
        if messages.is_empty() {
            return;
        }
        let (config, transport) = (self.config.clone(), self.transport.clone());
        self.flight = Some(tokio::task::spawn_blocking(move || {
            deliver(&config, |token| transport(token), messages)
        }));
    }
}

#[cfg(test)]
mod peer_tests;
#[cfg(test)]
mod tests;
