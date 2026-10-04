//! `POST /api/fleet/nudge`: start a peer that is not answering, and wait until
//! it answers (ADR-0052 §4).

use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::Context as _;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;

use crate::peer;
use crate::routes::read_peer_store;

/// The `daemon_id` a nudge is aimed at.
#[derive(serde::Deserialize)]
pub(crate) struct NudgeQuery {
    pub(crate) daemon_id: String,
}

/// `POST /api/fleet/nudge?daemon_id=<id>`: ask the OS to start a peer that is not
/// answering (ADR-0052 §4). Fire-and-forget — the daemon spawns and never
/// parents, holds, or signals what it started. The keepalive comes first: it is
/// what boots a stopped distro AND what keeps it booted; the unit start after it
/// covers the other half of `unreachable`, a running distro whose unit is down.
pub(crate) async fn fleet_nudge_route(
    peers_dir: PathBuf,
    self_daemon_id: Option<String>,
    bound_port: u16,
    daemon_id: String,
) -> Response {
    let (descriptors, _rejects) = read_peer_store(peers_dir.clone()).await;
    let Some(d) = descriptors.into_iter().find(|d| d.daemon_id == daemon_id) else {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": format!("No peer is announced as {daemon_id}.") })),
        )
            .into_response();
    };
    // A tunnel peer is woken by reopening its tunnel (ADR-0067 §2); the daemon
    // on the other machine belongs to that machine's service manager.
    if let Some(spec) = d.tunnel.clone() {
        let me = peer::client::SelfRef {
            port: bound_port,
            daemon_id: self_daemon_id.as_deref().unwrap_or_default(),
        };
        if let Some(refused) = peer::client::classify_self_dial(&d.address, spec.local_port, me) {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": refused.diagnosis(&d.environment) })),
            )
                .into_response();
        }
        if let Err(e) = peer::tunnel::hold_open(d.daemon_id.clone(), spec).await {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": format!("{e:#}") })),
            )
                .into_response();
        }
        let (ready, waited, last) = nudge_await_ready(
            &d,
            self_daemon_id.as_deref(),
            bound_port,
            peer::nudge::READY_DEADLINE,
        )
        .await;
        return ready_body(ready, waited, &last, &d.environment);
    }
    let Some(spec) = d.nudge.as_ref() else {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": format!(
                    "Peer {} announced no way to wake it. A WSL peer needs `loginctl enable-linger` \
                     and a `ralphy-daemon.service` user unit.",
                    d.environment
                )
            })),
        )
            .into_response();
    };
    if let Err(e) = hold_awake(spec.clone()).await {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": format!("{e:#}") })),
        )
            .into_response();
    }
    let argv = peer::nudge::nudge_argv(spec);
    if let Err(e) = peer::nudge::spawn_detached(&argv) {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": format!("{e:#}") })),
        )
            .into_response();
    }
    let (ready, waited, last) = nudge_await_ready(
        &d,
        self_daemon_id.as_deref(),
        bound_port,
        peer::nudge::READY_DEADLINE,
    )
    .await;
    ready_body(ready, waited, &last, &d.environment)
}

/// The nudge's answer. 200 either way: the nudge itself succeeded, and whether
/// the peer came back is an OBSERVATION this reports, not a failure of the
/// request. `ready` is the field a caller acts on — the old `nudged` is kept so
/// an older workbench keeps working.
fn ready_body(
    ready: bool,
    waited: Duration,
    last: &peer::client::PeerStatus,
    environment: &str,
) -> Response {
    Json(serde_json::json!({
        "nudged": true,
        "ready": ready,
        "waited_ms": waited.as_millis() as u64,
        "state": last.state(),
        "diagnosis": last.diagnosis(environment),
    }))
    .into_response()
}

/// Hold `spec.distro` open from the reactor: the keepalive spawns a process, so
/// it runs on the blocking pool. Logs when a handle was actually opened, so a
/// daemon log reads which distros this process is holding.
pub(crate) async fn hold_awake(spec: peer::NudgeSpec) -> anyhow::Result<()> {
    let distro = spec.distro.clone();
    let spawned = tokio::task::spawn_blocking(move || peer::nudge::keepalives().ensure(&spec))
        .await
        .context("the keepalive task did not complete")??;
    // The keepalive is a session handle, not supervision: ADR-0052 §4.
    if spawned {
        tracing::info!(%distro, "keeping the WSL distro running so the daemon inside it stays up");
    }
    Ok(())
}

/// Poll a just-nudged peer until it answers the handshake, or until `deadline`.
/// Returns whether it came back, how long that took, and the last status seen.
///
/// Waiting is not supervising (ADR-0052 §4): nothing here parents, holds or
/// signals the process the nudge started — systemd inside the distro owns it. All
/// this does is answer the operator's own question, which used to be answered
/// with "spawned" while the honest answer was "not yet".
///
/// `deadline` is a parameter rather than the constant so a test can pin the
/// give-up path without spending [`peer::nudge::READY_DEADLINE`] on it.
pub(crate) async fn nudge_await_ready(
    d: &peer::PeerDescriptor,
    self_daemon_id: Option<&str>,
    bound_port: u16,
    deadline: Duration,
) -> (bool, Duration, peer::client::PeerStatus) {
    let me = peer::client::SelfRef {
        port: bound_port,
        daemon_id: self_daemon_id.unwrap_or_default(),
    };
    let started = Instant::now();
    loop {
        let status = peer::client::probe(d, me).await;
        if status == peer::client::PeerStatus::Reachable {
            return (true, started.elapsed(), status);
        }
        // A refusal is a verdict about the descriptor, not about a boot in
        // progress: no amount of waiting turns a self-dial or a routable address
        // into a reachable peer, and neither does a version mismatch.
        if matches!(
            status,
            peer::client::PeerStatus::Refused { .. }
                | peer::client::PeerStatus::VersionMismatch { .. }
        ) {
            return (false, started.elapsed(), status);
        }
        if started.elapsed() >= deadline {
            return (false, started.elapsed(), status);
        }
        tokio::time::sleep(peer::nudge::READY_POLL).await;
    }
}
