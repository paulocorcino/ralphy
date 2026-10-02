//! `/ws/session` and `/api/sessions*`: launch, reattach, watch and close the
//! daemon-owned sessions, locally or relayed to the peer that owns them.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::extract::ws::WebSocketUpgrade;
use axum::extract::Query;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

mod bridge;
mod gemini_root;
mod refuse;
mod relay;

pub(crate) use bridge::*;
use refuse::Refuser;
pub(crate) use relay::*;

use super::{blocking_read, read_peer_store};
use crate::{agent_state, auth, checkout, confine, fleet, identity, peer, registry, session};

/// Query for `/ws/session`. A NEW agent launch carries `repo` + `agent`; a NEW
/// free-console launch (issue #167) carries `console=1` and an optional `repo`
/// (home dir when absent); a REATTACH carries `id` (and optional `takeover=1`,
/// or `watch=1` for a read-only attach, issue #334). All optional so one struct
/// serves every shape; the handler dispatches on `id` first, then `console`.
#[derive(serde::Deserialize)]
pub(crate) struct SessionQuery {
    pub(crate) repo: Option<String>,
    pub(crate) agent: Option<String>,
    pub(crate) id: Option<u64>,
    pub(crate) takeover: Option<u32>,
    pub(crate) watch: Option<u32>,
    pub(crate) console: Option<u32>,
    /// A worktree NAME beside `repo`+`agent` on a NEW agent launch (ADR-0063
    /// §3); ignored on a reattach — the record owns it — and on `console=1`.
    pub(crate) checkout: Option<String>,
    /// The startup command of a `console=1` launch: the shell runs it and the
    /// session ends with it. Ignored on every other path. Whitespace-only is
    /// the same as absent.
    pub(crate) command: Option<String>,
    /// The browser tab's holder id, on a launch or a writer reattach: a
    /// reattach naming the holder that claimed the slot reclaims it without
    /// `takeover` (ADR-0051 §9 amendment 2026-09-22). Ignored on `watch=1`.
    pub(crate) holder: Option<String>,
    /// The console name on a NEW agent launch. Claude takes its folded form as
    /// `--name` when the repo opts in; a name that folds to nothing keeps the
    /// hex name. Ignored on every other path. Empty is
    /// the same as absent; longer than 40 characters is cut to 40.
    pub(crate) name: Option<String>,
}

impl SessionQuery {
    /// The holder, when it is a well-formed one: 1–64 ASCII letters, digits,
    /// `-` or `_`. Anything else is treated as absent — it can only lose the
    /// reclaim, never gain one.
    pub(crate) fn holder(&self) -> Option<&str> {
        self.holder.as_deref().filter(|h| {
            (1..=64).contains(&h.len())
                && h.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        })
    }

    /// The console name, cut to the desk's limit on a char boundary so the
    /// launch and the desk agree on what the name is.
    pub(crate) fn name(&self) -> Option<&str> {
        let name = self.name.as_deref().filter(|n| !n.is_empty())?;
        Some(
            match name.char_indices().nth(crate::desk::CONSOLE_NAME_MAX) {
                Some((cut, _)) => &name[..cut],
                None => name,
            },
        )
    }
}

/// A fresh launch claims its writer slot inside the spawn; name its holder
/// there, so the tab that launched it can reclaim it later.
fn hold(att: &session::Attachment, holder: Option<&str>) {
    if let Some(holder) = holder {
        att.hold_as(holder);
    }
}

/// The two labels a `session-open` frame carries beside the identity: the
/// vendor-side name (Claude only) and the worktree the console lives in. They
/// travel together on every path — launch, reattach, watch.
#[derive(Default)]
pub(crate) struct SessionLabels {
    pub(crate) name: Option<String>,
    pub(crate) checkout: Option<String>,
}

#[derive(Clone)]
pub(crate) struct SessionHost {
    pub(crate) peers_dir: PathBuf,
    pub(crate) identity: Option<identity::Identity>,
    pub(crate) environment: String,
    /// The port this daemon bound, so a peer probe can refuse to dial itself.
    pub(crate) bound_port: u16,
}

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
/// A NEW launch is refused after the upgrade, by a `session-end` frame with
/// `reason: "refused"` and the reason as `message` ([`Refuser`]); the browser
/// cannot read the body of a refused upgrade, and a new launch has no id to
/// retry. A reattach keeps the HTTP statuses above.
pub(crate) async fn session_ws_upgrade(
    ws: WebSocketUpgrade,
    Query(mut query): Query<SessionQuery>,
    sessions: Arc<session::SessionManager>,
    registry_path: PathBuf,
    host: SessionHost,
    shutdown: tokio::sync::watch::Receiver<bool>,
) -> Response {
    // Every upgrade below, the peer relay included, inherits the cap.
    let ws = ws
        .max_message_size(crate::tree::MAX_COMMAND_BYTES)
        .max_frame_size(crate::tree::MAX_COMMAND_BYTES);
    let SessionHost {
        peers_dir,
        identity,
        environment,
        bound_port,
    } = host;
    // Owned: `query` is rewritten below (a peer ref resolves to its slug).
    let holder = query.holder().map(str::to_owned);
    let daemon_id = identity
        .as_ref()
        .map(|identity| identity.id.to_string())
        .unwrap_or_default();
    let refuser = Refuser::new(query.id.is_none(), &daemon_id, &environment);
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
    if query.console != Some(1) && locally_owned.is_none() {
        if let Some(repo_ref) = query.repo.clone() {
            let (descriptors, rejects) = read_peer_store(peers_dir.clone()).await;
            match fleet::route(&repo_ref, &daemon_id, &descriptors) {
                fleet::route::Route::Local { slug } => {
                    query.repo = Some(slug.to_string());
                }
                fleet::route::Route::Peer { peer, slug } => {
                    let me = peer::client::SelfRef {
                        port: bound_port,
                        daemon_id: &daemon_id,
                    };
                    let peer_query = peer_session_query(&query, slug);
                    return relay_to_peer(ws, peer, &peer_query, me, &refuser, shutdown).await;
                }
                fleet::route::Route::UnknownDaemon { daemon_id } => {
                    if let Some((environment, theirs)) = rejects
                        .iter()
                        .find_map(|reject| reject.version_mismatch_for(daemon_id))
                    {
                        let status = peer::client::PeerStatus::VersionMismatch {
                            theirs,
                            ours: peer::PEER_PROTOCOL_VERSION,
                        };
                        return refuser.refuse(
                            ws,
                            StatusCode::BAD_GATEWAY,
                            status.diagnosis(environment),
                        );
                    }
                    return refuser.refuse(
                        ws,
                        StatusCode::BAD_GATEWAY,
                        "the environment of this project is not in the list".to_string(),
                    );
                }
            }
        }
    }
    if let Some(id) = query.id {
        let effective_environment = locally_owned
            .as_ref()
            .and_then(|info| info.environment.clone())
            .unwrap_or_else(|| environment.clone());
        // A reattach re-announces the name the child was LAUNCHED under; the spec
        // is long gone, so the session record is where it comes from. Without
        // this a reload would blank the name on a console that still answers to it.
        let effective_labels = locally_owned
            .as_ref()
            .map(|info| SessionLabels {
                name: info.name.clone(),
                checkout: info.checkout.clone(),
            })
            .unwrap_or_default();
        // A watcher never touches the writer slot, so it is dispatched BEFORE the
        // attach branch and can never produce a `409`.
        if query.watch == Some(1) {
            return match sessions.watch(id) {
                Ok(att) => ws.on_upgrade(move |socket| {
                    session_ws(
                        socket,
                        att,
                        id,
                        daemon_id,
                        effective_environment,
                        effective_labels,
                        shutdown,
                    )
                }),
                // `watch` never yields `Busy`; matching the variant keeps that a
                // compile-time fact rather than a comment.
                Err(session::AttachError::Unknown) => {
                    (StatusCode::NOT_FOUND, "unknown session").into_response()
                }
                Err(session::AttachError::Busy) => {
                    (StatusCode::CONFLICT, "session busy").into_response()
                }
            };
        }
        return match sessions.attach_as(id, query.takeover == Some(1), query.holder()) {
            Ok(att) => ws.on_upgrade(move |socket| {
                session_ws(
                    socket,
                    att,
                    id,
                    daemon_id,
                    effective_environment,
                    effective_labels,
                    shutdown,
                )
            }),
            Err(session::AttachError::Unknown) => {
                (StatusCode::NOT_FOUND, "unknown session").into_response()
            }
            Err(session::AttachError::Busy) => {
                (StatusCode::CONFLICT, "session busy").into_response()
            }
        };
    }
    if query.console == Some(1) {
        let command = query
            .command
            .as_deref()
            .map(str::trim)
            .filter(|command| !command.is_empty())
            .map(str::to_owned);
        // The label the session list and the desk record carry: the command
        // for a startup-command console, `console` for the bare shell.
        let agent_label = command.clone().unwrap_or_else(|| "console".to_string());
        if let Some(repo_ref) = query.repo.clone() {
            let (descriptors, rejects) = read_peer_store(peers_dir).await;
            match fleet::route(&repo_ref, &daemon_id, &descriptors) {
                fleet::route::Route::Local { slug } => {
                    query.repo = Some(slug.to_string());
                }
                fleet::route::Route::Peer { peer, slug } => {
                    // A peer with no WSL distro is on another machine (ADR-0067
                    // §7): its free console runs THERE, through the same relay
                    // as an agent session, so it outlives this computer.
                    let Some(nudge) = peer.nudge.as_ref() else {
                        let me = peer::client::SelfRef {
                            port: bound_port,
                            daemon_id: &daemon_id,
                        };
                        let peer_query = peer_session_query(&query, slug);
                        return relay_to_peer(ws, peer, &peer_query, me, &refuser, shutdown).await;
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
                    let effective_environment = peer.environment.clone();
                    return match sessions
                        .spawn_attached(
                            repo_ref.clone(),
                            agent_label,
                            "console".to_string(),
                            Some(effective_environment.clone()),
                            None,
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
                                format!(
                                    "{} free-console launcher failed: {error}",
                                    peer.environment
                                ),
                            )
                        }
                    };
                }
                fleet::route::Route::UnknownDaemon { daemon_id } => {
                    if let Some((peer_environment, theirs)) = rejects
                        .iter()
                        .find_map(|reject| reject.version_mismatch_for(daemon_id))
                    {
                        let status = peer::client::PeerStatus::VersionMismatch {
                            theirs,
                            ours: peer::PEER_PROTOCOL_VERSION,
                        };
                        return refuser.refuse(
                            ws,
                            StatusCode::BAD_GATEWAY,
                            status.diagnosis(peer_environment),
                        );
                    }
                    return refuser.refuse(
                        ws,
                        StatusCode::BAD_GATEWAY,
                        "the environment of this project is not in the list".to_string(),
                    );
                }
            }
        }
        let repo_path = match query.repo.as_deref() {
            Some(slug) => {
                let store = match registry::load_from(&registry_path) {
                    Ok(store) => store,
                    Err(e) => {
                        tracing::warn!(error = %e, "failed to load repo registry for a console session");
                        return refuser.refuse(
                            ws,
                            StatusCode::BAD_REQUEST,
                            "repo registry unreadable",
                        );
                    }
                };
                let Some(entry) = store.entry(slug) else {
                    return refuser.refuse(ws, StatusCode::BAD_REQUEST, "unknown repo");
                };
                Some(PathBuf::from(&entry.path))
            }
            None => None,
        };
        let cwd = session::console_cwd(repo_path);
        let spec = session::console_spec(cwd, 24, 80, command.as_deref());
        let repo_label = query.repo.clone().unwrap_or_else(|| "~".to_string());
        return match sessions
            .spawn_attached(
                repo_label,
                agent_label,
                "console".to_string(),
                None,
                None,
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
                    environment,
                    SessionLabels::default(),
                    shutdown,
                )
            }),
            Err(e) => {
                tracing::warn!(error = %e, "failed to spawn a console session");
                refuser.refuse(
                    ws,
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "failed to spawn session",
                )
            }
        };
    }
    let Some(agent_str) = query.agent.as_deref() else {
        return refuser.refuse(ws, StatusCode::BAD_REQUEST, "unknown agent");
    };
    let Some(agent) = session::Agent::from_query(agent_str) else {
        return refuser.refuse(ws, StatusCode::BAD_REQUEST, "unknown agent");
    };
    let Some(repo) = query.repo.as_deref() else {
        return refuser.refuse(ws, StatusCode::BAD_REQUEST, "unknown repo");
    };
    let store = match registry::load_from(&registry_path) {
        Ok(store) => store,
        Err(e) => {
            tracing::warn!(error = %e, "failed to load repo registry for a session");
            return refuser.refuse(ws, StatusCode::BAD_REQUEST, "repo registry unreadable");
        }
    };
    let Some(entry) = store.entry(repo) else {
        return refuser.refuse(ws, StatusCode::BAD_REQUEST, "unknown repo");
    };
    let root = PathBuf::from(&entry.path);
    // ADR-0063 §3: the selected checkout, resolved with the same resolver and
    // confinement as every git-backed verb (`spawn_cwd`), off the runtime.
    // INVARIANT: this returns BEFORE the Cursor gate, the Gemini gate, `spec_for`
    // and every spawn — a bad name writes nothing and spawns nothing.
    let checkout = match query.checkout.as_deref() {
        None => None,
        Some(name) => {
            let (name, primary) = (name.to_string(), root.clone());
            let resolved = blocking_read(move || {
                let c = checkout::resolve(&name, |n| checkout::is_linked(&primary, n))
                    .map_err(|_| ())?;
                confine::confine(&primary, &c.prefix("")).map_err(|_| ())?;
                Ok::<_, ()>(c)
            })
            .await;
            match resolved {
                Some(Ok(c)) => Some(c),
                Some(Err(())) => {
                    return refuser.refuse(ws, StatusCode::BAD_REQUEST, checkout::UNKNOWN)
                }
                None => {
                    return refuser.refuse(ws, StatusCode::INTERNAL_SERVER_ERROR, "unavailable")
                }
            }
        }
    };
    let cwd = checkout
        .as_ref()
        .map_or_else(|| root.clone(), |c| c.dir(&root));
    // ADR-0042 D6: an ordinary Cursor run uploads the enclosing repository. The
    // run path is gated in the adapter, but this interactive launch spawns
    // `cursor-agent` directly — so the gate has to run here too, BEFORE the spec
    // is built and anything is spawned: it writes `.cursorindexingignore` into the
    // unprotected repo (announced on the daemon log) and then proceeds. A write
    // failure (read-only tree) is the only way it stops the launch. The gate
    // walks up from `cwd`, so a checkout gets its own opt-out and the primary
    // keeps its own (ADR-0063 §3); the opt-in is the primary's `.ralphy/`.
    if agent == session::Agent::Cursor {
        if let Err(e) =
            ralphy_proc_util::cursor::indexing_gate(&cwd, session::cursor_indexing_allowed(&root))
        {
            return refuser.refuse(ws, StatusCode::BAD_REQUEST, e.to_string());
        }
    }
    // ADR-0043 D4/D6: a Gemini child is contained by an owned configuration root
    // AND the policy document inside it. The daemon may not import the adapter
    // that writes them (ADR-0032 §10), so `gemini_root::ensure` asks the CLI to
    // (ADR-0040 Amendment 3). INVARIANT: this refusal precedes `spec_for` and
    // every spawn path, so no Gemini child is ever created outside the owned root.
    if agent == session::Agent::Gemini {
        if let Err(why) = gemini_root::ensure(&root, &daemon_id).await {
            return refuser.refuse(ws, StatusCode::BAD_REQUEST, why);
        }
    }
    // The id first: the agent-state files are named by it and must exist
    // before the child that reads them is launched (ADR-0059 §5). Only a
    // vendor with hooks gets the slot; the store dir failing to resolve means
    // no hooks, never no console.
    let id = sessions.reserve_id();
    let status = match agent {
        session::Agent::Claude => auth::store_dir()
            .ok()
            .map(|d| agent_state::StatusFiles::for_session(&d.join("sessions"), id)),
        _ => None,
    };
    // The fallback `--name` is built from the project NAME: a remoteless repo's
    // slug is a `path-<hash>` key, and the operator reads this name.
    let project = registry::project_name(repo, &entry.path);
    let spec = session::spec_with_status(agent, &root, cwd, &project, query.name(), 24, 80, status);
    // Lifted before the spec moves into the spawn: the bridge announces the name
    // in `session-open`, which is how the shell learns it without deriving the
    // format a second time.
    let labels = SessionLabels {
        name: spec.name.clone(),
        checkout: checkout.as_ref().map(|c| c.name().to_string()),
    };
    match sessions
        .spawn_attached_as(
            id,
            repo.to_string(),
            agent_str.to_string(),
            "agent".to_string(),
            None,
            labels.checkout.clone(),
            spec,
        )
        .inspect(|(_, att)| hold(att, holder.as_deref()))
    {
        Ok((id, att)) => ws.on_upgrade(move |socket| {
            session_ws(socket, att, id, daemon_id, environment, labels, shutdown)
        }),
        Err(e) => {
            tracing::warn!(error = %e, "failed to spawn a workbench session");
            refuser.refuse(
                ws,
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to spawn session",
            )
        }
    }
}
