//! A free console in a project of a WSL peer: the peer lends its folder,
//! and the shell runs on this computer through `wsl.exe` in that distro. A
//! peer with no WSL distro is relayed instead.

use std::ffi::OsString;
use std::path::Path;
use std::sync::Arc;

use axum::http::StatusCode;

use super::join::{claim_for, hold, join};
use super::refuse::Refusal;
use super::traffic::Tab;
use super::upgrade::{launch_record, relayed_record, Opened};
use super::{dial_peer, peer_session_open, Opening, PeerDial};
use super::{SessionLabels, SessionQuery};
use crate::routes::audit_layer::SocketAudit;
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
    pub(super) tab: Tab,
    pub(super) who: SocketAudit,
    pub(super) record: Option<String>,
}

impl PeerConsole<'_> {
    /// Relay, join or spawn the console, or refuse it.
    pub(super) async fn launch(self) -> Opened {
        let peer = self.peer;
        // A peer with no WSL distro is on another machine (ADR-0067 §7): its
        // free console runs THERE, through the same relay as an agent
        // session, so it outlives this computer.
        let Some(nudge) = peer.nudge.as_ref() else {
            return self.relay().await;
        };
        let (launcher, folder) = match self.lent_folder().await {
            Ok(lent) => lent,
            Err(refusal) => return Opened::Refused(refusal),
        };
        let spec = session::peer_console_spec(
            launcher,
            &nudge.distro,
            Path::new(&folder),
            24,
            80,
            self.command.as_deref(),
        );
        self.spawn(spec).await
    }

    fn me(&self) -> peer::client::SelfRef<'_> {
        peer::client::SelfRef {
            port: self.bound_port,
            daemon_id: &self.daemon_id,
        }
    }

    async fn relay(self) -> Opened {
        let PeerConsole {
            query,
            peer,
            slug,
            daemon_id,
            bound_port,
            tab,
            who,
            ..
        } = self;
        let fields = peer_session_open(query, slug);
        let opening = Opening {
            record: relayed_record(query, slug, &peer.daemon_id, &tab),
            tab,
            who,
        };
        let dial = PeerDial {
            peer,
            fields,
            me: peer::client::SelfRef {
                port: bound_port,
                daemon_id: &daemon_id,
            },
        };
        dial_peer(dial, opening).await
    }

    /// The `wsl.exe` launcher and the folder the peer lends for the project,
    /// once the peer answers and the folder is reachable.
    async fn lent_folder(&self) -> Result<(OsString, String), Refusal> {
        let peer = self.peer;
        let status = peer::client::probe(peer, self.me()).await;
        if status != peer::client::PeerStatus::Reachable {
            return Err(Refusal::refuse(
                StatusCode::BAD_GATEWAY,
                status.diagnosis(&peer.environment),
            ));
        }
        let Some(launcher) = session::peer_console_launcher() else {
            return Err(Refusal::refuse(
                StatusCode::BAD_GATEWAY,
                format!(
                    "{} cannot host a free console: wsl.exe launcher not found",
                    peer.environment
                ),
            ));
        };
        let entry = self.peer_repo().await?;
        if entry.path.is_empty() {
            return Err(Refusal::refuse(
                StatusCode::BAD_REQUEST,
                format!("{} sent no folder for this project", peer.environment),
            ));
        }
        if !entry.reachable {
            return Err(Refusal::refuse(
                StatusCode::BAD_REQUEST,
                format!(
                    "{} cannot reach the folder {}",
                    peer.environment, entry.path
                ),
            ));
        }
        Ok((launcher, entry.path))
    }

    /// Where the peer keeps the project, read from its repository list.
    async fn peer_repo(&self) -> Result<fleet::PeerRepoLocation, Refusal> {
        let peer = self.peer;
        match peer::client::get(peer, "/api/repos").await {
            Ok((200, body)) => match fleet::repo_from_repos_json(&body, self.slug) {
                Ok(Some(entry)) => Ok(entry),
                Ok(None) => Err(Refusal::refuse(
                    StatusCode::BAD_REQUEST,
                    format!("{} does not have this project", peer.environment),
                )),
                Err(_) => Err(Refusal::refuse(
                    StatusCode::BAD_GATEWAY,
                    format!(
                        "{} returned an unreadable repository list",
                        peer.environment
                    ),
                )),
            },
            Ok((status, _)) => Err(Refusal::refuse(
                StatusCode::BAD_GATEWAY,
                format!(
                    "{} refused its repository list with HTTP {status}",
                    peer.environment
                ),
            )),
            Err(error) => Err(Refusal::refuse(
                StatusCode::BAD_GATEWAY,
                peer::client::transport_failed(peer, format!("{error:#}")).await,
            )),
        }
    }

    /// Join the live session of the window record, or spawn the console.
    async fn spawn(self, spec: session::SessionSpec) -> Opened {
        let PeerConsole {
            sessions,
            query,
            peer,
            slug,
            repo_ref,
            agent_label,
            environment,
            tab,
            who,
            record,
            ..
        } = self;
        let claim = claim_for(&sessions, &record).await;
        if let Some(joined) = join(
            &sessions,
            claim.as_ref(),
            tab.holder.as_deref(),
            query.attempt,
            &environment,
        ) {
            return joined;
        }
        match sessions
            .spawn_attached(
                repo_ref.to_string(),
                agent_label,
                "console".to_string(),
                Some(peer.environment.clone()),
                None,
                record.clone(),
                spec,
            )
            .inspect(|(_, att)| hold(att, tab.holder.as_deref(), query.attempt))
        {
            Ok((id, att)) => {
                who.console(&launch_record(
                    id,
                    "console",
                    Some(slug.to_string()),
                    Some(peer.daemon_id.clone()),
                    &tab,
                ));
                Opened::Session {
                    att,
                    id,
                    environment: peer.environment.clone(),
                    labels: SessionLabels::default(),
                }
            }
            Err(error) => {
                tracing::warn!(
                    environment = %peer.environment,
                    error = %error,
                    "failed to spawn a peer free console"
                );
                Opened::refused(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("{} free-console launcher failed: {error}", peer.environment),
                )
            }
        }
    }
}
