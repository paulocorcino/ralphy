//! Ralphy's resident daemon (docs/adr/0032): a foreground HTTP listener bound
//! to localhost, serving the embedded workbench UI. This is the tracer bullet —
//! no sessions, no command vocabulary yet — but the shape is the decided one:
//! a library crate wired by `ralphy-cli`, the workspace's async runtime (tokio +
//! axum) confined here, runs reached only by spawning `ralphy` processes (never
//! by importing the core).

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use anyhow::{Context, Result};
use axum::Router;
use include_dir::{include_dir, Dir};

pub mod agent_state;
pub mod assets;
pub mod auth;
pub mod autostart;
pub mod checkout;
pub mod clipboard;
pub mod confine;
pub mod cookie;
pub mod desk;
pub mod dispatch;
pub mod epoch;
pub mod fleet;
pub mod fswrite;
pub mod identity;
pub mod note;
pub mod password;
pub mod peer;
pub mod pidfile;
pub mod protocol;
pub mod registry;
mod rekey;
pub mod release;
pub mod roster;
pub mod session;
pub mod spend;
pub mod textcodec;
pub mod totp;
pub mod tree;
pub mod usage;
pub mod watch;

mod routes;
mod serve;

use routes::*;
use serve::serve;

/// The daemon's default TCP port. "ralphy" on a phone keypad starts 7-2-5-7.
pub const DEFAULT_PORT: u16 = 7257;

/// The embedded workbench UI, baked in at build time like `assets/prompts` — the
/// daemon reads no files from disk at runtime (ADR-0032 §4). Promoted to the
/// daemon's `/` in #200 (PRD #185); the SPA self-gates its login (see
/// [`routes::require_auth`]), so there is no separate server-rendered login page.
pub(crate) static UI: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/assets/ui");

/// What the composition root decides; everything else is the daemon's.
pub struct DaemonConfig {
    /// TCP port for the listener.
    pub port: u16,
    /// The interface to bind. Defaults to `127.0.0.1` (loopback only); a
    /// non-localhost bind is an explicit opt-in that REQUIRES a bearer access
    /// token, enforced at boot by [`auth::AuthPolicy::for_bind`] (ADR-0032 §4).
    pub bind: IpAddr,
    /// Extra host names this daemon answers as, beyond the ones its bind implies:
    /// a MagicDNS name, a reverse-proxy hostname. The cross-site gate refuses any
    /// other `Host`, which is what keeps DNS rebinding out — so reaching the
    /// daemon by NAME (rather than by the bound IP) is an explicit declaration.
    pub allowed_hosts: Vec<String>,
    /// Directories this daemon announces itself into as a peer descriptor
    /// (ADR-0052 §3) — typically the OTHER environment's `.ralphy` store, e.g.
    /// `/mnt/c/Users/<user>/.ralphy` from inside WSL. A directory is the only
    /// thing an announcer can know about its peer; empty means "a fleet of one".
    pub peer_stores: Vec<PathBuf>,
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            port: DEFAULT_PORT,
            bind: Ipv4Addr::LOCALHOST.into(),
            allowed_hosts: Vec::new(),
            peer_stores: Vec::new(),
        }
    }
}

/// Compose the bind address from an interface and port. Centralized so the
/// resolved interface flows through one place (the auth policy keys on
/// `addr.ip()`).
pub fn bind_addr(ip: IpAddr, port: u16) -> SocketAddr {
    SocketAddr::new(ip, port)
}

/// Run the daemon in the foreground until Ctrl+C. Blocking on purpose: the
/// tokio runtime is created and dropped inside, so callers (the sync CLI)
/// never see async types.
pub fn run(config: DaemonConfig) -> Result<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("building the daemon's tokio runtime")?;
    runtime.block_on(serve(
        bind_addr(config.bind, config.port),
        config.allowed_hosts,
        config.peer_stores,
    ))
}

/// The per-vendor interactive session-store paths resolved once at daemon boot
/// and handed to the `/api/usage` scan — one `PathBuf` per vendor store. Grouped
/// so onboarding a vendor is a new field, not another positional threaded through
/// every `router` call site (#267); eight adjacent same-typed paths were also
/// transposition-prone (the compiler can't catch two swapped stores). `Default`
/// yields empty paths — a "no store" set the scans tolerate (ADR-0040 C6) — which
/// the daemon's tests use as their all-missing base.
#[derive(Clone, Default)]
pub struct StorePaths {
    pub claude_projects_dir: PathBuf,
    pub codex_dir: PathBuf,
    pub opencode_db: PathBuf,
    pub kimi_dir: PathBuf,
    pub kimi_code_dir: PathBuf,
    pub copilot_db: PathBuf,
    pub cursor_dir: PathBuf,
    pub gemini_dir: PathBuf,
}

/// The daemon's HTTP surface. Real routes sit *before* the embedded-UI
/// fallback. `GET /api/identity` returns the loaded identity as JSON, or 404
/// when the daemon is un-baptized, so the static page can render "avatar name"
/// at runtime (the embedded HTML bakes in no identity).
pub fn router(
    identity: Option<identity::Identity>,
    registry_path: PathBuf,
    usage_dir: PathBuf,
    stores: StorePaths,
    start: Instant,
    shutdown: tokio::sync::watch::Receiver<bool>,
    auth: Arc<auth::AuthState>,
) -> Router {
    router_with_roster(
        identity,
        registry_path,
        usage_dir,
        stores,
        start,
        shutdown,
        RouterDependencies {
            auth,
            roster_locator: Arc::new(session::Agent::locate_program),
        },
    )
}

#[cfg(test)]
mod tests;
