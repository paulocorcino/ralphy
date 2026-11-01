//! The join of the peers' answers: one question asked of every peer at the
//! same time, and the peers that did not answer named, never read as "nothing
//! there". `/api/sessions`, `/api/attention`, `/api/telegram` and the
//! attention tick read their peers through it.

use std::path::PathBuf;

use axum::response::Response;

use super::read_peer_store;
use super::PeerDescriptor;
use crate::attention::AttentionList;

/// The response header of a joined reply that names, comma-separated, the
/// peers whose part the reply does not hold because they did not answer.
/// Absent when every peer answered.
pub(crate) const UNANSWERED_HEADER: &str = "x-ralphy-unanswered";

/// The reply of each peer in `peers_dir` to `GET <path>`, asked at the same
/// time.
pub(crate) async fn ask_peers(
    peers_dir: PathBuf,
    path: &str,
) -> Vec<(PeerDescriptor, anyhow::Result<(u16, Vec<u8>)>)> {
    let (peers, _) = read_peer_store(peers_dir).await;
    futures_util::future::join_all(peers.into_iter().map(|peer| async move {
        let reply = super::client::get(&peer, path).await;
        (peer, reply)
    }))
    .await
}

/// Name the peers in `unanswered` in the reply's `x-ralphy-unanswered`. It
/// is a header, not a body field: the body stays the shape every reader and
/// the peer-to-peer `?local=1` form already parse.
pub(crate) fn name_unanswered(response: &mut Response, unanswered: &[String]) {
    if unanswered.is_empty() {
        return;
    }
    match unanswered.join(",").parse() {
        Ok(value) => {
            response.headers_mut().insert(UNANSWERED_HEADER, value);
        }
        Err(e) => tracing::warn!(error = %e, "could not name the peers that did not answer"),
    }
}

/// One peer's own list, as its `?local=1` reply has it.
pub(crate) struct PeerList {
    pub(crate) daemon_id: String,
    pub(crate) list: AttentionList,
}

/// The own list of each peer in `peers_dir`, and the ids of the peers that
/// did not answer with one. An older peer has no such route, and is named
/// too.
pub(crate) async fn read_peer_lists(peers_dir: PathBuf) -> (Vec<PeerList>, Vec<String>) {
    let mut lists = Vec::new();
    let mut unanswered = Vec::new();
    for (peer, reply) in ask_peers(peers_dir, "/api/attention?local=1").await {
        let Ok((200, body)) = reply else {
            unanswered.push(peer.daemon_id);
            continue;
        };
        match serde_json::from_slice::<AttentionList>(&body) {
            Ok(list) => lists.push(PeerList {
                daemon_id: peer.daemon_id,
                list,
            }),
            Err(_) => {
                tracing::warn!(
                    daemon_id = %peer.daemon_id,
                    "peer returned an invalid attention list"
                );
                unanswered.push(peer.daemon_id);
            }
        }
    }
    (lists, unanswered)
}
