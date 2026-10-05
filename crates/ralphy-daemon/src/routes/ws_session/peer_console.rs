//! A free console in a project of a WSL peer: the peer lends its folder,
//! and the shell runs on this computer through `wsl.exe` in that distro. A
//! peer with no WSL distro is relayed instead.

use std::path::Path;
use std::sync::Arc;

use axum::extract::ws::WebSocketUpgrade;
use axum::http::StatusCode;
use axum::response::Response;

use super::join::{claim_for, hold, Joined};
use super::refuse::Refuser;
use super::{peer_session_query, relay_to_peer, session_ws};
use super::{SessionLabels, SessionQuery};
use crate::{fleet, peer, session};

/// The request values a peer free-console launch reads.
pub(super) struct PeerConsole<'a> {
    pub(super) sessions: Arc<session::SessionManager>,
    pub(super) query: &'a SessionQuery,
    pub(super) peer: &'a peer::PeerDescriptor,
    pub(super) slug: &'a str,
    pub(super) repo_ref: &'a str,
    pub(super) command: Option<String>,
    pub(super) agent_label: String,
    pub(super) daemon_id: String,
    pub(super) environment: String,
    pub(super) bound_port: u16,
    pub(super) holder: Option<String>,
    pub(super) record: Option<String>,
    pub(super) refuser: Refuser,
    pub(super) shutdown: tokio::sync::watch::Receiver<bool>,
}

impl PeerConsole<'_> {
    /// Relay, join or spawn the console. Every refusal goes through the
    /// launch's [`Refuser`].
    pub(super) async fn launch(self, ws: WebSocketUpgrade) -> Response {
        let PeerConsole {
            sessions,
            query,
            peer,
            slug,
            repo_ref,
            command,
            agent_label,
            daemon_id,
            environment,
            bound_port,
            holder,
            record,
            refuser,
            shutdown,
        } = self;
        // A peer with no WSL distro is on another machine (ADR-0067 §7): its
        // free console runs THERE, through the same relay as an agent
        // session, so it outlives this computer.
        let Some(nudge) = peer.nudge.as_ref() else {
            let me = peer::client::SelfRef {
                port: bound_port,
                daemon_id: &daemon_id,
            };
            let peer_query = peer_session_query(query, slug);
            return relay_to_peer(ws, peer, &peer_query, holder, me, &refuser, shutdown).await;
        };
        let status = peer::client::probe(
            peer,
            peer::client::SelfRef {
                port: bound_port,
                daemon_id: &daemon_id,
            },
        )
        .await;
        if status != peer::client::PeerStatus::Reachable {
            return refuser.refuse(
                ws,
                StatusCode::BAD_GATEWAY,
                status.diagnosis(&peer.environment),
            );
        }
        let Some(launcher) = session::peer_console_launcher() else {
            return refuser.refuse(
                ws,
                StatusCode::BAD_GATEWAY,
                format!(
                    "{} cannot host a free console: wsl.exe launcher not found",
                    peer.environment
                ),
            );
        };
        let entry = match peer::client::get(peer, "/api/repos").await {
            Ok((200, body)) => match fleet::repo_from_repos_json(&body, slug) {
                Ok(Some(entry)) => entry,
                Ok(None) => {
                    return refuser.refuse(
                        ws,
                        StatusCode::BAD_REQUEST,
                        format!("{} does not have this project", peer.environment),
                    );
                }
                Err(_) => {
                    return refuser.refuse(
                        ws,
                        StatusCode::BAD_GATEWAY,
                        format!(
                            "{} returned an unreadable repository list",
                            peer.environment
                        ),
                    );
                }
            },
            Ok((status, _)) => {
                return refuser.refuse(
                    ws,
                    StatusCode::BAD_GATEWAY,
                    format!(
                        "{} refused its repository list with HTTP {status}",
                        peer.environment
                    ),
                );
            }
            Err(error) => {
                return refuser.refuse(
                    ws,
                    StatusCode::BAD_GATEWAY,
                    fleet::route::peer_unreachable(peer, &format!("{error:#}")),
                );
            }
        };
        if entry.path.is_empty() {
            return refuser.refuse(
                ws,
                StatusCode::BAD_REQUEST,
                format!("{} sent no folder for this project", peer.environment),
            );
        }
        if !entry.reachable {
            return refuser.refuse(
                ws,
                StatusCode::BAD_REQUEST,
                format!(
                    "{} cannot reach the folder {}",
                    peer.environment, entry.path
                ),
            );
        }
        let spec = session::peer_console_spec(
            launcher,
            &nudge.distro,
            Path::new(&entry.path),
            24,
            80,
            command.as_deref(),
        );
        let claim = claim_for(&sessions, &record).await;
        if let Some(joined) =
            Joined::find(&sessions, claim.as_ref(), holder.as_deref(), &environment)
        {
            return joined.upgrade(ws, daemon_id, holder, shutdown);
        }
        let effective_environment = peer.environment.clone();
        match sessions
            .spawn_attached(
                repo_ref.to_string(),
                agent_label,
                "console".to_string(),
                Some(effective_environment.clone()),
                None,
                record.clone(),
                spec,
            )
            .inspect(|(_, att)| hold(att, holder.as_deref()))
        {
            Ok((id, att)) => ws.on_upgrade(move |socket| {
                session_ws(
                    socket,
                    att,
                    id,
                    daemon_id,
                    effective_environment,
                    SessionLabels::default(),
                    holder,
                    shutdown,
                )
            }),
            Err(error) => {
                tracing::warn!(
                    environment = %peer.environment,
                    error = %error,
                    "failed to spawn a peer free console"
                );
                refuser.refuse(
                    ws,
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("{} free-console launcher failed: {error}", peer.environment),
                )
            }
        }
    }
}
