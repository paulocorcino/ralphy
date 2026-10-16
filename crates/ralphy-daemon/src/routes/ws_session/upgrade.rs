//! The `/ws/session` handler: one route, four shapes (reattach, watch, agent
//! launch, free-console launch), each local or relayed to the peer that owns
//! the repo.

use std::ops::ControlFlow;
use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::ws::WebSocketUpgrade;
use axum::extract::Query;
use axum::http::StatusCode;
use axum::response::Response;

use super::join::{claim_for, Joined};
use super::refuse::Refuser;
use super::traffic::Tab;
use super::{peer_session_query, relay_to_peer, session_ws, Opening};
use super::{SessionHost, SessionLabels, SessionQuery};
use crate::routes::audit_layer::{ConsoleKind, ConsoleRecord, SocketAudit};
use crate::routes::read_peer_store;
use crate::{fleet, peer, registry, session};

/// `GET /ws/session`: four shapes over one route.
///
/// - `?id=<id>[&takeover=1]` — REATTACH to a daemon-owned session. `attach`
///   returns `404` for an unknown id and `409` for a busy one (a single writer is
///   attached and `takeover` was not set) — both BEFORE the upgrade, so a refusal
///   is an HTTP status the browser can read, not a silently-dropped socket.
/// - `?id=<id>&watch=1` — REATTACH read-only (issue #334): the same replay and
///   live stream, but the writer slot is never claimed, so a busy session is
///   reachable (never `409`) and nobody is evicted. Only `404` refuses it. This
///   is what lets a second workbench see a session instead of stealing it.
/// - `?repo=<slug>&agent=<claude|codex|opencode>[&checkout=<name>][&name=<console name>]`
///   — NEW agent launch. Refuses an unknown agent, an unreadable registry, or an
///   unregistered slug before anything is spawned; an unknown or malformed
///   `checkout` is refused as `unknown checkout` before anything is written or
///   spawned (ADR-0063 §3); a spawn failure is refused too.
/// - `?console=1[&repo=<slug>][&command=<cmd>]` — NEW free-console launch
///   (issue #167): the platform shell in the chosen repo's dir, or the home dir
///   when `repo` is absent. With `command`, the shell runs that command instead
///   of a prompt and the session ends when it exits (the startup-command
///   console: `htop`, `btop`…); the session's `agent` label is then the
///   command, so the workbench can tell it from a bare shell. Refuses an
///   unreadable registry, an unregistered slug, or a spawn failure.
///
/// Either NEW launch may carry `&record=<window record id>`. While a live
/// session already serves that record, nothing is spawned: the socket joins
/// that session, as the writer when the slot is free or is this holder's,
/// else read-only with `watch: true` in its `session-open`.
///
/// A NEW launch is refused after the upgrade, by a `session-end` frame with
/// `reason: "refused"` and the reason as `message` ([`Refuser`]); the browser
/// cannot read the body of a refused upgrade, and a new launch has no id to
/// retry. A reattach keeps the HTTP statuses above, except a claim that a
/// newer attempt of the same holder superseded: on any shape it upgrades and
/// ends with `reason: "superseded"`, and nobody is evicted (ADR-0079 D7).
pub(crate) async fn session_ws_upgrade(
    ws: WebSocketUpgrade,
    Query(query): Query<SessionQuery>,
    sessions: Arc<session::SessionManager>,
    registry_path: PathBuf,
    host: SessionHost,
    shutdown: tokio::sync::watch::Receiver<bool>,
    who: SocketAudit,
) -> Response {
    // Every upgrade below, the peer relay included, inherits the cap.
    let ws = ws
        .max_message_size(crate::tree::MAX_COMMAND_BYTES)
        .max_frame_size(crate::tree::MAX_COMMAND_BYTES);
    let upgrade = Upgrade::new(ws, query, sessions, registry_path, host, shutdown, who);
    let upgrade = match upgrade.relay_unowned().await {
        ControlFlow::Break(response) => return response,
        ControlFlow::Continue(upgrade) => upgrade,
    };
    if let Some(id) = upgrade.query.id {
        return upgrade.reattach(id);
    }
    if upgrade.query.console == Some(1) {
        return upgrade.console().await;
    }
    upgrade.agent().await
}

/// One `/ws/session` request, read: the socket still to upgrade, the query,
/// and the values every shape of the route needs.
pub(super) struct Upgrade {
    pub(super) ws: WebSocketUpgrade,
    pub(super) query: SessionQuery,
    pub(super) sessions: Arc<session::SessionManager>,
    pub(super) registry_path: PathBuf,
    peers_dir: PathBuf,
    pub(super) daemon_id: String,
    pub(super) environment: String,
    pub(super) bound_port: u16,
    pub(super) tab: Tab,
    pub(super) who: SocketAudit,
    pub(super) record: Option<String>,
    pub(super) refuser: Refuser,
    pub(super) shutdown: tokio::sync::watch::Receiver<bool>,
    /// The session `query.id` names, when this daemon hosts it.
    pub(super) locally_owned: Option<session::SessionInfo>,
}

/// Where the repo ref of a request lives.
pub(super) enum Placement {
    Local,
    Peer {
        peer: Box<peer::PeerDescriptor>,
        slug: String,
        repo_ref: String,
    },
    UnknownDaemon(String),
}

impl Upgrade {
    fn new(
        ws: WebSocketUpgrade,
        query: SessionQuery,
        sessions: Arc<session::SessionManager>,
        registry_path: PathBuf,
        host: SessionHost,
        shutdown: tokio::sync::watch::Receiver<bool>,
        who: SocketAudit,
    ) -> Upgrade {
        let SessionHost {
            peers_dir,
            identity,
            environment,
            bound_port,
        } = host;
        let tab = query.tab(who.device);
        let record = query.record().map(str::to_owned);
        let daemon_id = identity
            .as_ref()
            .map(|identity| identity.id.to_string())
            .unwrap_or_default();
        let refuser = Refuser::new(query.id, &daemon_id, &environment, tab.clone());
        // A peer free console is the one composite-ref session hosted HERE. Match
        // both id and repo so an equal numeric id owned by the peer still proxies.
        let locally_owned = query.id.and_then(|id| {
            sessions.get(id).filter(|info| {
                query
                    .repo
                    .as_deref()
                    .map(|repo| repo == info.repo)
                    .unwrap_or(true)
            })
        });
        Upgrade {
            ws,
            query,
            sessions,
            registry_path,
            peers_dir,
            daemon_id,
            environment,
            bound_port,
            tab,
            who,
            record,
            refuser,
            shutdown,
            locally_owned,
        }
    }

    /// Refuse with `message`, as the [`Refuser`] of this request does.
    pub(super) fn refuse(self, status: StatusCode, message: impl Into<String>) -> Response {
        self.refuser.refuse(self.ws, status, message)
    }

    /// Bridge the upgraded socket to the attachment `att` of session `id`.
    pub(super) fn bridge(
        self,
        att: session::Attachment,
        id: session::SessionId,
        environment: String,
        labels: SessionLabels,
    ) -> Response {
        self.ws.on_upgrade(move |socket| {
            session_ws(
                socket,
                att,
                id,
                self.daemon_id,
                environment,
                labels,
                self.tab,
                self.shutdown,
            )
        })
    }

    /// Where the repo of the query lives, or `None` when it names no repo. A
    /// repo of this daemon is rewritten to its slug.
    pub(super) async fn place(&mut self) -> Option<Placement> {
        let repo_ref = self.query.repo.clone()?;
        let (descriptors, rejects) = read_peer_store(self.peers_dir.clone()).await;
        Some(
            match fleet::route(&repo_ref, &self.daemon_id, &descriptors) {
                fleet::route::Route::Local { slug } => {
                    self.query.repo = Some(slug.to_string());
                    Placement::Local
                }
                fleet::route::Route::Peer { peer, slug } => Placement::Peer {
                    peer: Box::new(peer.clone()),
                    slug: slug.to_string(),
                    repo_ref,
                },
                fleet::route::Route::UnknownDaemon { daemon_id } => {
                    Placement::UnknownDaemon(fleet::unknown_daemon(daemon_id, &rejects))
                }
            },
        )
    }

    /// Relay a request for a repo that lives on another daemon. Any other
    /// request goes on, with a repo of this daemon rewritten to its slug.
    async fn relay_unowned(mut self) -> ControlFlow<Response, Upgrade> {
        if self.query.console == Some(1) || self.locally_owned.is_some() {
            return ControlFlow::Continue(self);
        }
        match self.place().await {
            None | Some(Placement::Local) => ControlFlow::Continue(self),
            Some(Placement::Peer { peer, slug, .. }) => {
                ControlFlow::Break(self.relay(&peer, &slug).await)
            }
            Some(Placement::UnknownDaemon(why)) => {
                ControlFlow::Break(self.refuse(StatusCode::BAD_GATEWAY, why))
            }
        }
    }

    async fn relay(self, peer: &peer::PeerDescriptor, slug: &str) -> Response {
        let me = peer::client::SelfRef {
            port: self.bound_port,
            daemon_id: &self.daemon_id,
        };
        let peer_query = peer_session_query(&self.query, slug);
        let opening = Opening {
            record: relayed_record(&self.query, slug, &peer.daemon_id, &self.tab),
            tab: self.tab,
            who: self.who,
        };
        relay_to_peer(
            self.ws,
            peer,
            &peer_query,
            opening,
            me,
            &self.refuser,
            self.shutdown,
        )
        .await
    }

    /// Take the claim on the window record and join the live session that
    /// serves it. The claim is held until the caller drops it, which is after
    /// the spawn that does not join.
    pub(super) async fn claim_and_join(&self) -> (Option<session::RecordClaim>, Option<Joined>) {
        let claim = claim_for(&self.sessions, &self.record).await;
        let joined = Joined::find(
            &self.sessions,
            claim.as_ref(),
            self.tab.holder.as_deref(),
            self.query.attempt,
            &self.environment,
        );
        (claim, joined)
    }

    /// The path of the registered repo `slug`. `purpose` names the launch in
    /// the log line of an unreadable registry.
    pub(super) fn registered_repo(
        &self,
        slug: &str,
        purpose: &str,
    ) -> Result<String, &'static str> {
        let store = registry::load_from(&self.registry_path).map_err(|e| {
            tracing::warn!(error = %e, "failed to load repo registry for {purpose}");
            "repo registry unreadable"
        })?;
        store
            .entry_or_former(slug)
            .map(|entry| entry.path.clone())
            .ok_or("unknown repo")
    }
}

/// The audit record of a session this daemon just spawned.
pub(super) fn launch_record(
    session: session::SessionId,
    agent: &str,
    repo: Option<String>,
    peer: Option<String>,
    tab: &Tab,
) -> ConsoleRecord {
    ConsoleRecord {
        kind: ConsoleKind::Launch,
        session: Some(session),
        agent: Some(agent.to_string()),
        repo,
        peer,
        holder: tab.holder.clone(),
    }
}

/// The audit record of a request relayed to the peer `peer_id`: a launch, a
/// take-over, or nothing for a plain reattach or a watch. The peer owns the id
/// of a session it launches, so a relayed launch names none.
pub(super) fn relayed_record(
    query: &SessionQuery,
    slug: &str,
    peer_id: &str,
    tab: &Tab,
) -> Option<ConsoleRecord> {
    let (kind, session, agent) = match query.id {
        Some(id) if query.takeover == Some(1) && query.watch != Some(1) => {
            (ConsoleKind::Takeover, Some(id), None)
        }
        Some(_) => return None,
        None if query.console == Some(1) => {
            (ConsoleKind::Launch, None, Some("console".to_string()))
        }
        None => (ConsoleKind::Launch, None, query.agent.clone()),
    };
    Some(ConsoleRecord {
        kind,
        session,
        agent,
        repo: Some(slug.to_string()),
        peer: Some(peer_id.to_string()),
        holder: tab.holder.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn query(json: serde_json::Value) -> SessionQuery {
        serde_json::from_value(json).expect("a session query")
    }

    #[test]
    fn a_relayed_request_records_a_launch_or_a_takeover_and_nothing_else() {
        let tab = Tab {
            holder: Some("tab-a".to_string()),
            ..Tab::default()
        };
        let record = |q| relayed_record(&query(q), "owner/repo", "01PEER", &tab);

        let agent = record(serde_json::json!({"agent": "claude"})).expect("a launch");
        assert_eq!(agent.kind, ConsoleKind::Launch);
        assert_eq!(agent.agent.as_deref(), Some("claude"));
        assert_eq!(
            agent.session, None,
            "the peer owns the id of what it launches"
        );
        assert_eq!(agent.peer.as_deref(), Some("01PEER"));
        assert_eq!(agent.holder.as_deref(), Some("tab-a"));

        let console =
            record(serde_json::json!({"console": 1, "command": "htop"})).expect("a launch");
        assert_eq!(
            console.agent.as_deref(),
            Some("console"),
            "never the startup command"
        );

        let takeover = record(serde_json::json!({"id": 7, "takeover": 1})).expect("a take-over");
        assert_eq!(takeover.kind, ConsoleKind::Takeover);
        assert_eq!(takeover.session, Some(7));

        assert!(record(serde_json::json!({"id": 7})).is_none(), "a reattach");
        assert!(
            record(serde_json::json!({"id": 7, "takeover": 1, "watch": 1})).is_none(),
            "a watch"
        );
    }
}
