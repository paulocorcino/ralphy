//! The foreground listener: bind, announce this daemon to its peers, serve
//! until the shutdown signal (docs/adr/0032, ADR-0052 §3).

use std::net::{Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::time::Instant;

use anyhow::{Context, Result};

use crate::router;
use crate::routes::{poll_releases, RELEASE_POLL_EVERY};
use crate::StorePaths;
use crate::{auth, autostart, epoch, identity, peer, pidfile, registry, usage};

pub(crate) async fn serve(
    addr: SocketAddr,
    allowed_hosts: Vec<String>,
    peer_stores: Vec<PathBuf>,
) -> Result<()> {
    // Captured at daemon start so every presence heartbeat reports process
    // uptime, not per-connection age.
    let start = Instant::now();
    // Fired when the operator asks the daemon to stop. Every `/ws` presence loop
    // watches this so a held-open connection cannot stall graceful shutdown.
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("binding the daemon listener on {addr}"))?;
    // Log the *bound* address, not the requested one, so a future `port: 0`
    // (OS-assigned) still reports something a browser can open.
    let addr = listener.local_addr().context("reading the bound address")?;
    tracing::info!(%addr, "daemon listening — open http://{addr} (Ctrl+C to stop)");
    if !addr.ip().is_loopback() {
        // TLS-aware, not TLS-enforcing (ADR-0032 §4 and its audit amendment):
        // the daemon never terminates TLS, and never refuses the bind either —
        // it says so once, where the operator who chose the bind will read it.
        tracing::warn!(
            %addr,
            "this listener speaks plain HTTP — put it behind a TLS front (dev tunnel, ngrok) or a tailnet"
        );
    }

    // Record which process is serving, so `ralphy daemon restart` can end it —
    // there is no other way to name it (ADR-0056 §8). Advisory, never a lock: a
    // failure to write it must not stop a daemon that is otherwise ready.
    let store = auth::store_dir().ok();
    if let Some(dir) = store.as_deref() {
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

    // The release watch (ADR-0056 §6). A daemon-lifetime concern, so it lives
    // here and not in `router`: a router is also built by tests, and this task
    // reaches the network and writes a cache.
    if let Some(dir) = store.as_ref().cloned() {
        let mut release_shutdown = shutdown_rx.clone();
        tokio::spawn(async move {
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
        });
    }

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
    // Resolve the effective access token, then the bind policy. INVARIANT:
    // `for_bind` returns Err and aborts startup on a non-loopback bind with no
    // token — the daemon must never begin serving an unauthenticated network
    // socket (ADR-0032 §4).
    let token = auth::effective_token()?;
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
    if !peer_stores.is_empty() {
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
    // INVARIANT: strip the token from the process env on the boot path BEFORE any
    // child can be spawned, so every subsequent `dispatch`/`session` child
    // inherits a token-free env on ALL paths (mirrors RALPHY_EVENTS_TOKEN,
    // ADR-0019). The auth state already holds the effective token as its
    // signing-key fallback.
    auth::strip_token_from_env();

    // Resolve the registry path once and hand it to the router. `/api/repos`
    // reads it FRESH from disk on each request, so the resident daemon sees
    // writes made by separate `ralphy run` processes (ADR-0032).
    let registry_path = registry::repos_toml_path()?;
    // Hold every nudgeable peer's distro open from the start (ADR-0052 §4
    // amendment): a WSL peer whose keepalive died with the previous daemon
    // would otherwise stay `asleep` until a chip is clicked. Windows only —
    // the handle is a `wsl.exe`. AFTER `strip_token_from_env`: the keepalive is
    // a child like any other, and the token invariant above covers it. The
    // store is read fresh, the same way the fleet route reads it.
    if cfg!(windows) {
        let peers_dir = registry_path.with_file_name("peers");
        let self_id = id.as_ref().map(|i| i.id.to_string());
        tokio::spawn(wake_fleet_at_start(peers_dir, self_id));
    }
    let usage_dir = usage::usage_dir_path()?;
    let stores = StorePaths {
        claude_projects_dir: usage::claude_projects_dir_path()?,
        codex_dir: usage::codex_dir_path()?,
        opencode_db: usage::opencode_db_path()?,
        kimi_dir: usage::kimi_dir_path()?,
        kimi_code_dir: usage::kimi_code_dir_path()?,
        copilot_db: usage::copilot_db_path()?,
        cursor_dir: usage::cursor_dir_path()?,
        gemini_dir: usage::gemini_dir_path()?,
    };
    axum::serve(
        listener,
        router(
            id,
            registry_path,
            usage_dir,
            stores,
            start,
            shutdown_rx,
            auth_state,
        ),
    )
    .with_graceful_shutdown(async move {
        shutdown_signal().await;
        // Break every live `/ws` loop so graceful shutdown does not wait on
        // a long-lived heartbeat connection.
        let _ = shutdown_tx.send(true);
    })
    .await
    .context("serving the daemon listener")?;
    if let Some(dir) = store.as_deref() {
        pidfile::clear_in(dir);
    }
    tracing::info!("daemon stopped");
    Ok(())
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
        environment: peer::environment_label(wsl_distro, std::env::consts::OS),
        token,
        protocol_version: peer::PEER_PROTOCOL_VERSION,
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
    if !addr.ip().is_loopback() && !addr.ip().is_unspecified() {
        tracing::warn!(
            bind = %addr.ip(),
            "--peer-store was given but this daemon does not listen on loopback — a peer              is reached over loopback only (ADR-0052 §2); announcing nothing"
        );
        return;
    }
    let Some(id) = id else {
        tracing::warn!(
            "--peer-store was given but this daemon is un-baptized — run `ralphy daemon setup`              to mint its identity, then restart; announcing nothing"
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
                tracing::warn!(error = %e, "could not resolve this daemon's access token; announcing nothing");
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
    if let Err(e) = tokio::signal::ctrl_c().await {
        tracing::error!(error = %e, "failed to listen for Ctrl+C; running until killed");
        std::future::pending::<()>().await;
    }
    tracing::info!("shutdown requested (Ctrl+C)");
}
