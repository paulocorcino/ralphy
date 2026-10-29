//! The main daemon's claim on its peers (docs/adr/0085 D14): a daemon that
//! sends Telegram messages tells each peer in its peers folder, every
//! [`EVERY`], that it is their main (`POST /api/telegram/main`). A claimed
//! peer sends no message while the claim holds, and the main sends the
//! peer's console requests, so one request is one message.
//!
//! A peer that does not answer, or answers no to the claim, changes nothing
//! here. An older peer answers 404: it has no such route, and keeps sending
//! by itself if it has a bot, so the operator may get one message from each
//! until both run the same version. None of these is a fault of this daemon,
//! so each is logged at debug only.

use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use crate::peer::{self, PeerDescriptor};
use crate::telegram_state::{self, SharedClaim};

/// How often the main daemon claims its peers. A claim holds three minutes
/// (`telegram_state::CLAIM_HOLDS`), so two claims can be lost in a row.
pub(crate) const EVERY: Duration = Duration::from_secs(60);

/// What the claim of one peer came to.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Claimed {
    /// The peer took the claim.
    Taken,
    /// The peer has no claim route: an older version.
    OldPeer,
    /// The peer answered with another status, or did not answer.
    Refused(String),
}

/// What a daemon claims its peers with.
#[derive(Clone)]
pub(crate) struct Claimer {
    /// This daemon's Telegram config.
    pub(crate) config: PathBuf,
    /// The claim of a main daemon on this daemon.
    pub(crate) claim: SharedClaim,
    pub(crate) peers_dir: PathBuf,
    pub(crate) daemon_id: String,
}

/// Whether this daemon sends at `now`: no main daemon's claim holds on it,
/// and its config has a bot. A config that cannot be read is "no": the
/// attention tick logs it.
fn sends(c: &Claimer, now: SystemTime) -> bool {
    telegram_state::holder_at(&c.claim, now).is_none()
        && matches!(telegram_state::bot_at(&c.config), Ok(Some(_)))
}

/// Claim `peer` for the main daemon `daemon_id`.
async fn claim_peer(peer: &PeerDescriptor, daemon_id: &str) -> Claimed {
    let body = serde_json::json!({ "daemon_id": daemon_id });
    let claimed = match peer::client::post_json(peer, "/api/telegram/main", &body).await {
        Ok((200, _)) => Claimed::Taken,
        Ok((404, _)) => Claimed::OldPeer,
        Ok((status, _)) => Claimed::Refused(format!("HTTP {status}")),
        Err(e) => Claimed::Refused(format!("{e:#}")),
    };
    match &claimed {
        Claimed::Taken => {}
        Claimed::OldPeer => {
            tracing::debug!(daemon_id = %peer.daemon_id, "the peer has no Telegram claim route; it is an older version");
        }
        Claimed::Refused(why) => {
            tracing::debug!(daemon_id = %peer.daemon_id, %why, "the peer did not take the Telegram claim");
        }
    }
    claimed
}

/// One round: when this daemon sends, claim each of its peers. The answer
/// is each peer's id and what its claim came to; empty when this daemon
/// does not send, so it claims no peer.
pub(crate) async fn claim_once(c: &Claimer) -> Vec<(String, Claimed)> {
    let check = c.clone();
    let sends = tokio::task::spawn_blocking(move || sends(&check, SystemTime::now())).await;
    if !matches!(sends, Ok(true)) {
        return Vec::new();
    }
    let (peers, _) = crate::routes::read_peer_store(c.peers_dir.clone()).await;
    let claims = peers
        .iter()
        .map(|peer| async { (peer.daemon_id.clone(), claim_peer(peer, &c.daemon_id).await) });
    futures_util::future::join_all(claims).await
}

/// Claim the peers every [`EVERY`] until the daemon stops. The first round
/// runs at once, so a peer stops sending soon after its main starts.
pub(crate) async fn run(c: Claimer, mut shutdown: tokio::sync::watch::Receiver<bool>) {
    let mut every = tokio::time::interval(EVERY);
    every.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            _ = shutdown.changed() => break,
            _ = every.tick() => {
                claim_once(&c).await;
            }
        }
    }
}
