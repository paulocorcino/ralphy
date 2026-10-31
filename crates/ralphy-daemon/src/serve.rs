//! The foreground listener: bind, announce this daemon to its peers, serve
//! until the shutdown signal (docs/adr/0032, ADR-0052 §3).

use std::net::{Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Context, Result};

use crate::routes::{poll_releases, RELEASE_POLL_EVERY};
use crate::StorePaths;
use crate::{auth, autostart, epoch, identity, peer, pidfile, registry, usage};
use crate::{mux, router_tracking};

#[cfg(unix)]
pub mod socket;

pub(crate) async fn serve(
    addr: SocketAddr,
    allowed_hosts: Vec<String>,
    peer_stores: Vec<PathBuf>,
    env_token: Option<String>,
) -> Result<()> {
    // Captured at daemon start so every presence heartbeat reports process
    // uptime, not per-connection age.
    let start = Instant::now();
    // Fired when the operator asks the daemon to stop. Every `/ws` presence loop
    // watches this so a held-open connection cannot stall graceful shutdown.
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let store = auth::store_dir().ok();
    // Both listeners are bound before anything that depends on them: the pid
    // file, the announce, the auth state and the tunnels.
    let bound = tokio::net::TcpListener::bind(addr).await;
    #[cfg(unix)]
    let (tcp, socket) = bind_unix(bound, addr, store.as_deref())?;
    #[cfg(not(unix))]
    let tcp = Some(bound.with_context(|| format!("binding the daemon listener on {addr}"))?);
    // Log the *bound* address, not the requested one, so a future `port: 0`
    // (OS-assigned) still reports something a browser can open. A daemon that
    // serves only its socket keeps the requested address as a nominal one: the
    // auth state, the self-dial gate and the tunnels key on its port.
    let addr = match &tcp {
        Some(listener) => listener.local_addr().context("reading the bound address")?,
        None => addr,
    };
    #[cfg(unix)]
    let socket_path = socket.as_ref().map(|socket| socket.path().to_path_buf());
    #[cfg(not(unix))]
    let socket_path: Option<PathBuf> = None;
    log_listening(addr, tcp.is_some(), socket_path.as_deref());

    // Resolve the own binary now, while it is still the file this daemon
    // runs: a later first read can name a file replaced since.
    crate::dispatch::ralphy_exe();
    // The hook check runs the hook program and may take seconds, so it is
    // not awaited: the listener starts without it.
    drop(tokio::task::spawn_blocking(
        crate::agent_state::run_start_check,
    ));
    // Held to the end of this function: every return, an error included,
    // clears the pid file.
    let _pidfile = store.as_deref().map(|dir| {
        write_pidfile(dir);
        PidfileGuard(dir.to_path_buf())
    });

    // The release watch (ADR-0056 §6). A daemon-lifetime concern, so it lives
    // here and not in `router`: a router is also built by tests, and this task
    // reaches the network and writes a cache.
    if let Some(dir) = store.as_ref().cloned() {
        tokio::spawn(poll_releases_until_stop(dir, shutdown_rx.clone()));
    }

    let id = load_identity();
    // Resolve the effective access token, then the bind policy. INVARIANT:
    // `for_bind` returns Err and aborts startup on a non-loopback bind with no
    // token — the daemon must never begin serving an unauthenticated network
    // socket (ADR-0032 §4).
    // `run` read the env token and removed it from the environment before
    // the runtime started.
    let token = auth::effective_token_from(env_token)?;
    // Announce this daemon into every peer store (ADR-0052 §3). Done AFTER the
    // listener is bound so the descriptor carries the BOUND port — an OS-assigned
    // port (`--port 0`) is announced correctly, never the requested one — and
    // AFTER `effective_token`, so announcing REUSES whatever credential this
    // daemon already has (env override included) and mints only when there is
    // none. Minting first would displace a `RALPHY_DAEMON_TOKEN` env credential
    // and could flip a require-login daemon from `Localhost` to `Session`, which
    // AC5 ("the auth policy is unchanged") forbids.
    // INVARIANT: a store that cannot be written logs and is skipped — announcing
    // must never abort a listener that is already serving.
    // A daemon that serves only its socket announces nothing: a descriptor
    // carries a TCP port, and the port belongs to another daemon.
    if !peer_stores.is_empty() && tcp.is_some() {
        announce_peer(&peer_stores, id.as_ref(), addr, token.clone());
    }
    // The live session epoch (ADR-0032 amendment §B): mixed into every cookie so a
    // bump is an instant, total logout. Persisted beside the token.
    let session_epoch = epoch::SessionEpoch::load(epoch::epoch_path()?)?;
    // Boot the runtime auth state (amendment §A/§B): validates the
    // network-bind-needs-a-token invariant, reads the on-disk seed / password /
    // require-login flag, and computes the initial policy. The policy is now
    // runtime-swappable — a security toggle rebuilds it in place, no restart.
    let auth_state = auth::AuthState::boot(addr, token, session_epoch, &allowed_hosts)?;

    // Resolve the registry path once and hand it to the router. `/api/repos`
    // reads it FRESH from disk on each request, so the resident daemon sees
    // writes made by separate `ralphy run` processes (ADR-0032).
    let registry_path = registry::repos_toml_path()?;
    // The keepalives and the tunnels are children like any other: `run`
    // removed every secret from the environment before this function began.
    spawn_peer_tasks(
        registry_path.with_file_name("peers"),
        id.as_ref().map(|i| i.id.to_string()),
        addr.port(),
    );
    let usage_dir = usage::usage_dir_path()?;
    let stores = usage_stores()?;
    let stop = shutdown_rx.clone();
    let (page_sockets, drain) = mux::tracked();
    let app = router_tracking(
        id,
        registry_path,
        usage_dir,
        stores,
        start,
        shutdown_rx,
        auth_state,
        page_sockets,
    );
    spawn_shutdown_fanout(shutdown_tx);
    serve_until_stopped(
        tcp,
        #[cfg(unix)]
        socket,
        app,
        stop,
        drain,
    )
    .await?;
    tracing::info!("daemon stopped");
    Ok(())
}

/// How long the stop waits for the page sockets after the listeners stop.
/// Each socket ends within `mux::SHUTDOWN_WAIT` of the stop by itself; the
/// extra second covers the end of its channels.
const SOCKETS_WAIT: std::time::Duration =
    mux::SHUTDOWN_WAIT.saturating_add(std::time::Duration::from_secs(1));

/// Serve until `stop` says stop, then wait, bounded, for the page sockets
/// `drain` tracks: the caller drops the runtime next, and that would cut
/// their last frames.
async fn serve_until_stopped(
    tcp: Option<tokio::net::TcpListener>,
    #[cfg(unix)] socket: Option<socket::BoundSocket>,
    app: axum::Router,
    stop: tokio::sync::watch::Receiver<bool>,
    drain: mux::Drain,
) -> Result<()> {
    let served = serve_listeners(
        tcp,
        #[cfg(unix)]
        socket,
        app,
        stop,
    )
    .await;
    if !drain.wait(SOCKETS_WAIT).await {
        tracing::debug!("a page socket was still open when the daemon stopped");
    }
    served
}

/// Say where the daemon listens, and warn once when its TCP listener is not
/// on loopback.
fn log_listening(addr: SocketAddr, tcp_bound: bool, socket_path: Option<&Path>) {
    if tcp_bound {
        tracing::info!(%addr, "daemon listening — open http://{addr} (Ctrl+C to stop)");
    }
    if let Some(path) = socket_path {
        tracing::info!(path = %path.display(), "daemon listening on its socket");
    }
    if tcp_bound && !addr.ip().is_loopback() {
        // TLS-aware, not TLS-enforcing (ADR-0032 §4 and its audit amendment):
        // the daemon never terminates TLS, and never refuses the bind either —
        // it says so once, where the operator who chose the bind will read it.
        tracing::warn!(
            %addr,
            "this listener speaks plain HTTP — put it behind a TLS front (dev tunnel, ngrok) or a tailnet"
        );
    }
}

/// One task waits for the signal and fans it out on the watch: every `/ws`
/// loop and every listener's graceful shutdown wait on a receiver, so a
/// held-open heartbeat cannot stall the stop.
fn spawn_shutdown_fanout(shutdown_tx: tokio::sync::watch::Sender<bool>) {
    tokio::spawn(async move {
        shutdown_signal().await;
        if let Err(e) = shutdown_tx.send(true) {
            tracing::debug!(error = %e, "nothing was waiting for the shutdown");
        }
    });
}

/// Serve `app` on each listener that is bound until `stop` says stop. A
/// daemon with no listener of a kind serves nothing on it and returns at once.
async fn serve_listeners(
    tcp: Option<tokio::net::TcpListener>,
    #[cfg(unix)] socket: Option<socket::BoundSocket>,
    app: axum::Router,
    stop: tokio::sync::watch::Receiver<bool>,
) -> Result<()> {
    let served_tcp = async {
        match tcp {
            // The connection's address reaches the audit log (ADR-0074).
            Some(listener) => axum::serve(
                listener,
                app.clone()
                    .into_make_service_with_connect_info::<SocketAddr>(),
            )
            .with_graceful_shutdown(stopped(stop.clone()))
            .await
            .context("serving the daemon listener"),
            None => Ok(()),
        }
    };
    #[cfg(unix)]
    {
        let (listener, remove) = match socket.map(socket::BoundSocket::into_parts) {
            Some((listener, remove)) => (Some(listener), Some(remove)),
            None => (None, None),
        };
        let served_socket = async {
            match listener {
                Some(listener) => axum::serve(listener, app.clone())
                    .with_graceful_shutdown(stopped(stop.clone()))
                    .await
                    .context("serving the daemon socket"),
                None => Ok(()),
            }
        };
        let (tcp_result, socket_result) = tokio::join!(served_tcp, served_socket);
        if let Some(remove) = remove {
            remove();
        }
        tcp_result?;
        socket_result?;
    }
    #[cfg(not(unix))]
    served_tcp.await?;
    Ok(())
}

/// Record which process is serving, so `ralphy daemon restart` can end it —
/// there is no other way to name it (ADR-0056 §8). Advisory, never a lock: a
/// failure to write it must not stop a daemon that is otherwise ready.
fn write_pidfile(dir: &Path) {
    // The invocation, not just the pid: a daemon started with `--port 8080`
    // must come back on 8080, not on the default. And the program, so a
    // restart can tell this daemon from whatever reused its number after a
    // crash or a reboot (ADR-0056 §8).
    let args: Vec<String> = std::env::args().skip(1).collect();
    match std::env::current_exe() {
        Ok(exe) => {
            if let Err(e) = pidfile::write_in(dir, std::process::id(), &exe, &args) {
                tracing::warn!(error = %e, "could not record the daemon pid");
            }
        }
        Err(e) => tracing::warn!(
            error = %e,
            "could not resolve this executable; not recording a pid a restart cannot verify"
        ),
    }
}

/// Clears the pid file in its store when dropped, while the file still names
/// this process. A removal that fails is logged by `pidfile::clear_in`.
struct PidfileGuard(PathBuf);

impl Drop for PidfileGuard {
    fn drop(&mut self) {
        pidfile::clear_own_in(&self.0, std::process::id());
    }
}

/// Poll the releases now and every [`RELEASE_POLL_EVERY`] after, until the
/// daemon stops.
async fn poll_releases_until_stop(
    dir: PathBuf,
    mut release_shutdown: tokio::sync::watch::Receiver<bool>,
) {
    let mut interval = tokio::time::interval(RELEASE_POLL_EVERY);
    // The first tick is immediate: a daemon that has just come back on a
    // new build should not wait six hours to learn what it is.
    loop {
        tokio::select! {
            _ = release_shutdown.changed() => break,
            _ = interval.tick() => {}
        }
        let dir = dir.clone();
        // `ureq` is blocking, and this reactor drives every terminal.
        if let Err(e) = tokio::task::spawn_blocking(move || poll_releases(&dir)).await {
            // A panic in the poll, or a pool refusing the task at
            // shutdown: the loop keeps ticking either way, so the only
            // way to notice a cache that stopped refreshing is to say so.
            tracing::warn!(error = %e, "the release poll did not complete");
        }
    }
}

/// This daemon's identity, or none when it has none or it cannot be read.
fn load_identity() -> Option<identity::Identity> {
    // Log a load failure rather than masking a corrupt daemon.toml as
    // "un-baptized" — the operator needs to see the real fault, not a silent
    // fall-through to no-identity.
    let id = match identity::load_current() {
        Ok(id) => id,
        Err(e) => {
            tracing::warn!(error = %e, "failed to load daemon identity; serving without one");
            None
        }
    };
    if id.is_none() {
        tracing::info!("daemon has no identity yet — run `ralphy daemon setup` to baptize it");
    }
    id
}

/// Start the tasks that hold the peers: the WSL keepalives and the tunnels.
/// Each one starts a child process.
fn spawn_peer_tasks(peers_dir: PathBuf, self_id: Option<String>, port: u16) {
    // Hold every nudgeable peer's distro open from the start (ADR-0052 §4
    // amendment): a WSL peer whose keepalive died with the previous daemon
    // would otherwise stay `asleep` until a chip is clicked. Windows only —
    // the handle is a `wsl.exe`. The store is read fresh, the same way the
    // fleet route reads it.
    if cfg!(windows) {
        tokio::spawn(wake_fleet_at_start(peers_dir.clone(), self_id.clone()));
    }
    // Open every peer tunnel from the start (ADR-0067 §2), on every OS.
    tokio::spawn(hold_tunnels_at_start(peers_dir, self_id, port));
}

/// Where the usage store of each agent lives on this machine.
fn usage_stores() -> Result<StorePaths> {
    Ok(StorePaths {
        claude_projects_dir: usage::claude_projects_dir_path()?,
        codex_dir: usage::codex_dir_path()?,
        opencode_db: usage::opencode_db_path()?,
        kimi_dir: usage::kimi_dir_path()?,
        kimi_code_dir: usage::kimi_code_dir_path()?,
        copilot_db: usage::copilot_db_path()?,
        cursor_dir: usage::cursor_dir_path()?,
        gemini_dir: usage::gemini_dir_path()?,
        // A machine with no config directory sends no Telegram message; the
        // daemon still starts.
        telegram_config: crate::telegram_state::config_path().unwrap_or_else(|e| {
            tracing::warn!(error = %e, "no Telegram config path; no Telegram message is sent");
            PathBuf::new()
        }),
    })
}

/// Bind the daemon socket next to the TCP result. A busy port with the
/// require-token marker leaves the daemon on its socket alone (#518, M4); a
/// daemon with neither listener keeps the TCP error.
#[cfg(unix)]
fn bind_unix(
    bound: std::io::Result<tokio::net::TcpListener>,
    addr: SocketAddr,
    store: Option<&Path>,
) -> Result<(Option<tokio::net::TcpListener>, Option<socket::BoundSocket>)> {
    if let Err(e) = &bound {
        // A marker that cannot be read counts as absent: the TCP error stays.
        let marker =
            store.is_some_and(|dir| matches!(auth::require_token_enabled_in(dir), Ok(true)));
        if socket::tcp_bind_failure_is_fatal(e.kind(), marker) {
            return bound
                .map(|_| (None, None))
                .with_context(|| format!("binding the daemon listener on {addr}"));
        }
    }
    let socket = match store {
        Some(dir) => socket::bind(dir)?,
        None => None,
    };
    match (bound, socket) {
        (Ok(tcp), socket) => Ok((Some(tcp), socket)),
        (Err(e), None) => Err(e).with_context(|| format!("binding the daemon listener on {addr}")),
        (Err(e), Some(socket)) => {
            tracing::warn!(
                error = %e,
                "port {} is in use; this daemon serves only its socket",
                addr.port()
            );
            Ok((None, Some(socket)))
        }
    }
}

/// Resolves when the shutdown watch says stop.
async fn stopped(mut stop: tokio::sync::watch::Receiver<bool>) {
    if let Err(e) = stop.wait_for(|stop| *stop).await {
        // The sender is gone, so nothing can ask for a stop any more.
        tracing::warn!(error = %e, "the shutdown signal is gone; stopping");
    }
}

/// Open a keepalive for every announced peer that can be nudged, skipping this
/// daemon's own descriptor (a WSL daemon writes into the same store). Failures
/// are logged per peer: a distro that no longer exists must not keep the others
/// asleep, and none of this may abort a listener that is already serving.
async fn wake_fleet_at_start(peers_dir: PathBuf, self_id: Option<String>) {
    let (descriptors, _rejects) = crate::routes::read_peer_store(peers_dir).await;
    for d in descriptors {
        if self_id.as_deref() == Some(d.daemon_id.as_str()) {
            continue;
        }
        let Some(spec) = d.nudge.clone() else {
            continue;
        };
        if let Err(e) = crate::routes::hold_awake(spec).await {
            tracing::warn!(peer = %d.environment, error = %e, "could not hold the peer's distro awake");
        }
    }
}

/// Ensure every peer tunnel once at start, off the reactor. Nothing here may
/// abort a listener that is already serving.
async fn hold_tunnels_at_start(peers_dir: PathBuf, self_id: Option<String>, port: u16) {
    let (descriptors, _rejects) = crate::routes::read_peer_store(peers_dir).await;
    let held = tokio::task::spawn_blocking(move || {
        let me = peer::client::SelfRef {
            port,
            daemon_id: self_id.as_deref().unwrap_or(""),
        };
        peer::tunnel::tunnels().ensure_all(&descriptors, me);
    })
    .await;
    if let Err(e) = held {
        tracing::warn!(error = %e, "opening the peer tunnels did not complete");
    }
}

/// Build the descriptor this daemon announces. Pure: every input is a
/// parameter, so the three branches a live boot cannot easily exercise
/// (un-baptized, no WSL, an existing vs a freshly minted token) are unit-tested.
///
/// The announced address is always loopback: the peer transport is loopback-only
/// (ADR-0052 §2), and WSL2's `localhostForwarding` relay is what carries it
/// across the boundary.
pub(crate) fn announced_descriptor(
    id: &identity::Identity,
    port: u16,
    wsl_distro: Option<&str>,
    token: String,
) -> peer::PeerDescriptor {
    peer::PeerDescriptor {
        daemon_id: id.id.to_string(),
        name: id.name.clone(),
        avatar: id.avatar.clone(),
        address: Ipv4Addr::LOCALHOST.to_string(),
        port,
        environment: peer::environment_label(
            wsl_distro,
            std::env::consts::OS,
            wsl_distro
                .is_none()
                .then(peer::system_release)
                .flatten()
                .as_deref(),
        ),
        os: std::env::consts::OS.to_string(),
        token,
        protocol_version: peer::PEER_PROTOCOL_VERSION,
        tunnel: None,
        // Only a daemon inside WSL can be woken by `wsl.exe`, so only it
        // advertises how.
        nudge: wsl_distro.map(|distro| peer::NudgeSpec {
            distro: distro.to_string(),
            unit: autostart::UNIT_NAME.to_string(),
        }),
    }
}

/// Write this daemon's peer descriptor into every store in `stores` (ADR-0052
/// §3). Each daemon announces its OWN token — there is no shared secret, so
/// rotating one daemon's token revokes exactly that one peer.
///
/// `token` is the ALREADY-RESOLVED effective token; one is minted here only when
/// there is none, so announcing never displaces an env credential nor changes
/// the bind policy. An un-baptized daemon has no identity to announce and is
/// skipped with a warning naming the command that fixes it.
pub(crate) fn announce_peer(
    stores: &[PathBuf],
    id: Option<&identity::Identity>,
    addr: SocketAddr,
    token: Option<String>,
) {
    // A daemon that does not listen on loopback cannot be reached by a peer, and
    // announcing `127.0.0.1` for it would produce a descriptor that dials a port
    // nothing answers — an `Unreachable` that never says the descriptor is wrong.
    // Peers reach each other over loopback only: ADR-0052 §2.
    if !addr.ip().is_loopback() && !addr.ip().is_unspecified() {
        tracing::warn!(
            bind = %addr.ip(),
            "--peer-store is ignored: this daemon does not listen on 127.0.0.1, and \
             other daemons can reach it only there"
        );
        return;
    }
    let Some(id) = id else {
        tracing::warn!(
            "--peer-store is ignored: this daemon has no name yet — run \
             `ralphy daemon setup`, then restart the daemon"
        );
        return;
    };
    let token = match token {
        Some(token) if !token.is_empty() => token,
        // No credential yet: mint the daemon's one and only token now. On a
        // loopback bind this changes no policy (`AuthPolicy::for_bind`), and the
        // peer needs SOMETHING to present.
        _ => match auth::token_path().and_then(|p| auth::ensure_token_at(&p)) {
            Ok((token, _minted)) => token,
            Err(e) => {
                tracing::warn!(error = %e, "--peer-store is ignored: could not read this daemon's access token");
                return;
            }
        },
    };
    let distro = std::env::var("WSL_DISTRO_NAME")
        .ok()
        .filter(|d| !d.is_empty());
    let descriptor = announced_descriptor(id, addr.port(), distro.as_deref(), token);
    for store in stores {
        match peer::write_descriptor(store, &descriptor) {
            Ok(path) => tracing::info!(path = %path.display(), "announced this daemon as a peer"),
            Err(e) => {
                tracing::warn!(store = %store.display(), error = %e, "could not announce into this peer store")
            }
        }
    }
}

/// Resolves when the operator asks the foreground daemon to stop. Ctrl+C maps
/// to a console event on Windows and SIGINT on Unix — `tokio::signal::ctrl_c`
/// covers both, keeping shutdown cross-platform without cfg splits.
pub(crate) async fn shutdown_signal() {
    tokio::select! {
        () = ctrl_c() => tracing::info!("shutdown requested (Ctrl+C)"),
        () = STOP.notified() => tracing::info!("shutdown requested (update hand-over)"),
    }
}

async fn ctrl_c() {
    if let Err(e) = tokio::signal::ctrl_c().await {
        tracing::error!(error = %e, "failed to listen for Ctrl+C; running until killed");
        std::future::pending::<()>().await;
    }
}

/// A request from inside the process to stop serving. Process-wide, like
/// Ctrl+C: there is one listener per process, and `router` stays free of a
/// handle that only production uses.
static STOP: tokio::sync::Notify = tokio::sync::Notify::const_new();

/// Ask the daemon to shut down cleanly, the same way Ctrl+C does. `notify_one`
/// keeps the permit when nothing is waiting yet, so the request is never lost.
pub(crate) fn request_stop() {
    STOP.notify_one();
}

#[cfg(test)]
mod tests;
