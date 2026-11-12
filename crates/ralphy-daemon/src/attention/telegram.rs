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
use std::fmt::Write as _;
use std::path::Path;
use std::time::{Duration, SystemTime};

use ralphy_telegram::client::{BotClient, Transport};

use super::{AttentionRequest, Kind};
use crate::session::Agent;
use crate::telegram_state;

/// How long a console request is open before it is sent.
pub(crate) const DELAY: Duration = Duration::from_secs(120);

/// One message to send, for the request `id`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Message {
    pub(crate) id: String,
    pub(crate) text: String,
}

/// What a message names besides the request: this daemon's id, the origin
/// of the operator's last page (the link), and the peers whose runs this
/// daemon sends. `unanswered` names the peers that did not answer this tick.
#[derive(Debug, Default)]
pub(crate) struct Context {
    pub(crate) daemon_id: Option<String>,
    pub(crate) origin: Option<String>,
    pub(crate) runs_of: HashSet<String>,
    pub(crate) unanswered: Vec<String>,
}

/// What a request says it is, or `None` for a request that is not sent: a
/// silent console has no state to tell, and a run sends its own message
/// unless it runs on a peer with no bot (ADR-0085 D14, amendment of
/// 2026-11-03, A4).
fn what(request: &AttentionRequest, ctx: &Context) -> Option<&'static str> {
    match request.kind {
        Kind::ConsoleWaiting => Some("is waiting"),
        Kind::ConsoleDone => Some("has finished its turn"),
        Kind::ConsoleSilent => None,
        Kind::RunWaiting => sends_runs_of(request, ctx).then_some("is waiting"),
        Kind::RunFinished => sends_runs_of(request, ctx).then_some("stopped"),
    }
}

/// Whether this daemon sends the runs of the daemon that holds `request`: a
/// peer's request id is `<daemon_id>/<id>`, and this daemon's own has no
/// prefix.
fn sends_runs_of(request: &AttentionRequest, ctx: &Context) -> bool {
    request
        .id
        .split_once('/')
        .is_some_and(|(peer, _)| ctx.runs_of.contains(peer))
}

/// The vendor's name as a person writes it; an unknown one as it came.
fn vendor_name(vendor: &str) -> &str {
    match Agent::from_query(vendor) {
        Some(Agent::Antigravity) => "Antigravity",
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

/// What a message calls the source of `request`: a console with its name,
/// or a run with its issue.
fn subject_of(request: &AttentionRequest) -> String {
    let vendor = vendor_name(&request.vendor);
    match request.kind {
        Kind::RunWaiting | Kind::RunFinished => match &request.issue {
            Some(issue) => format!("{vendor} run on issue #{issue}"),
            None => format!("{vendor} run"),
        },
        Kind::ConsoleWaiting | Kind::ConsoleDone | Kind::ConsoleSilent => match &request.name {
            Some(name) => format!("{vendor} console \"{name}\""),
            None => format!("{vendor} console"),
        },
    }
}

/// The text of the message of `request`: what waits, where, and the link to
/// the console when the origin of a page and this daemon's id are known. A
/// run has no console, so no link.
fn text_of(request: &AttentionRequest, what: &str, ctx: &Context) -> String {
    let mut text = subject_of(request);
    if let Some(repo) = &request.repo {
        write!(text, " in {repo}").expect(crate::WRITE_TO_STRING);
    }
    text.push(' ');
    text.push_str(what);
    if let Some(detail) = &request.detail {
        text.push_str(": ");
        text.push_str(&short(detail));
    }
    let console = request.console.as_deref().filter(|c| !c.is_empty());
    if let Some(link) = console.and_then(|console| link_of(console, ctx)) {
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
        console.to_owned()
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
        Some((at, _)) => format!("{}...", detail.split_at(at).0),
        None => detail.to_owned(),
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
            let what = what(request, ctx)?;
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
    /// The messages due now. An id no longer in `requests` is forgotten, so
    /// a console that waits again later is sent again; the ids of a peer in
    /// `ctx.unanswered` are kept, so a peer that misses one tick is not sent
    /// again.
    pub(crate) fn next(
        &mut self,
        requests: &[AttentionRequest],
        present: bool,
        now: SystemTime,
        ctx: &Context,
    ) -> Vec<Message> {
        let open: HashSet<&str> = requests.iter().map(|r| r.id.as_str()).collect();
        let keep = |id: &String| {
            open.contains(id.as_str())
                || ctx.unanswered.iter().any(|peer| {
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
                tracing::warn!(
                    request = %id,
                    %error,
                    "the Telegram message could not be sent; it is tried again"
                );
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

mod notifier;

pub(crate) use notifier::{api_transports, Notifier, Transports};

#[cfg(test)]
mod peer_run_tests;
#[cfg(test)]
mod peer_tests;
#[cfg(test)]
mod tests;
