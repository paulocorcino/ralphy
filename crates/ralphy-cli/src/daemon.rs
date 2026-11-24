//! `ralphy daemon`: run the resident daemon in the foreground (docs/adr/0032),
//! plus `daemon setup` (baptism), `daemon status` and the store verbs. The CLI is
//! the composition root — it installs a plain tracing stack for readable
//! foreground logs and hands off to `ralphy-daemon`, where the async runtime
//! lives. Interactive baptism reads stdin, so it lives in `setup`, never in the
//! resident foreground process which must not block on stdin; `setup --name
//! --avatar` reads nothing, so a remote shell over SSH can run it (ADR-0067).

use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::{Args, Subcommand, ValueEnum};

use ralphy_daemon::identity::{self, avatar_by_number, format_status_line, validate_name, AVATARS};
use ralphy_daemon::registry;
use ralphy_daemon::{auth, password, totp};

mod bootstrap;
pub(crate) mod confirm;
pub(crate) mod describe;
mod first_start;
mod register;
pub(crate) mod restart;
mod setup;

pub(crate) use describe::port_from_args;
pub(crate) use register::register_repo;

#[derive(Args)]
pub(crate) struct DaemonArgs {
    /// The TCP port to listen on.
    #[arg(long, default_value_t = ralphy_daemon::DEFAULT_PORT)]
    pub(crate) port: u16,

    /// The network address to listen on. The default, 127.0.0.1, accepts
    /// connections from this computer only. Any other address needs an access
    /// token from `ralphy daemon setup`, or the daemon does not start. The
    /// connection is not encrypted, so to reach the daemon from another
    /// computer, put a service that encrypts in front of it: a dev tunnel,
    /// ngrok, or your tailnet.
    // ADR-0032 §4.
    #[arg(long, default_value = "127.0.0.1")]
    pub(crate) bind: std::net::IpAddr,

    /// A host name the daemon accepts, in addition to its address (repeat for
    /// more). Set it when you reach the daemon by a name: the public name of a
    /// tunnel or a proxy, or a VPN name. You do not need it for the IP address,
    /// or for a tunnel that changes `Host` and `Origin` to 127.0.0.1 (dev tunnels
    /// does this; ngrok and Cloudflare Tunnel do not, so they need it). The
    /// daemon refuses every other name, which blocks DNS rebinding attacks. Give
    /// the exact name, never a wildcard: `*.<provider>` would accept the tunnels
    /// of every other customer of that provider.
    // ADR-0032 §4.
    #[arg(long = "allowed-host", value_name = "HOST")]
    pub(crate) allowed_hosts: Vec<String>,

    /// A folder where this daemon announces itself, so a daemon in another
    /// environment can find it (repeat for more). Usually the other
    /// environment's `.ralphy` folder; from inside WSL, `--peer-store
    /// /mnt/c/Users/<user>/.ralphy`. Each daemon shares only its own access
    /// token, so when you remove one peer, the others keep working.
    // ADR-0052 §3.
    #[arg(long = "peer-store", value_name = "DIR")]
    pub(crate) peer_stores: Vec<PathBuf>,

    #[command(subcommand)]
    pub(crate) command: Option<DaemonCommand>,
}

#[derive(Subcommand)]
pub(crate) enum DaemonCommand {
    /// Give the daemon a name (the default comes from the computer name) and an
    /// avatar. The first time, this also creates its identity.
    ///
    /// With `--name` and `--avatar`, it asks nothing and sets only the name and
    /// the avatar: no access token, no sign-in code, no password.
    Setup {
        /// The daemon's name: lowercase letters, digits and hyphens. Needs
        /// `--avatar`.
        #[arg(long, value_name = "NAME", requires = "avatar")]
        name: Option<String>,

        /// The avatar's number in the list that `ralphy daemon setup` shows.
        /// Needs `--name`.
        #[arg(long, value_name = "N", requires = "name")]
        avatar: Option<usize>,
    },
    /// Show the daemon's name and avatar, and the address to open.
    Status,
    /// Add a repo to the daemon by its folder. Nothing changes when it is
    /// already there.
    Add {
        /// The repo's folder.
        #[arg(value_name = "PATH")]
        path: PathBuf,

        /// Create a git repository at PATH when there is none, without asking.
        /// Without this flag Ralphy asks, and the answer is no when it cannot
        /// ask (for example, when the input comes from a pipe).
        #[arg(long)]
        init: bool,

        /// Create the folder at PATH when it does not exist. Its parent folder
        /// must exist. Needs --init.
        #[arg(long, requires = "init")]
        create: bool,
    },
    /// Remove a repo from the daemon by its `owner/repo` name. Nothing changes
    /// when it is not there.
    Remove {
        /// The repo's `owner/repo` name.
        #[arg(value_name = "SLUG")]
        slug: String,
    },
    /// Start the daemon automatically when you log in: a shortcut in the
    /// Windows Startup folder, a systemd user unit on Linux, or a launchd agent
    /// on macOS.
    Install,
    /// Stop starting the daemon automatically when you log in. Nothing changes
    /// when it was not set up.
    Uninstall,
    /// Stop the running daemon and start this program in its place. Run it after
    /// an update, so the new version serves the workbench.
    // ADR-0056.
    Restart {
        /// Do not ask before closing the open consoles.
        #[arg(long)]
        yes: bool,
    },
    /// Ask for the access token on every request, also from this computer. Turn
    /// it on for a daemon that another computer reaches through an SSH tunnel:
    /// the tunnel arrives on 127.0.0.1, like a program on this computer. `on`
    /// creates the access token when there is none. Restart the daemon to apply
    /// the change.
    // ADR-0067 §5.
    RequireToken {
        #[arg(value_enum, value_name = "STATE")]
        state: OnOff,
    },
    /// Print this daemon's name, port and peer protocol as JSON, for a computer
    /// that adds this one as a host.
    #[command(hide = true)]
    Describe {
        /// Also print the access token.
        #[arg(long)]
        with_token: bool,
    },
    /// Change the access token. Every other computer that uses the old token is
    /// disconnected. Restart the daemon to apply the change.
    RotateToken,
}

/// A setting turned on or off.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub(crate) enum OnOff {
    On,
    Off,
}

pub(crate) fn run(args: &DaemonArgs) -> Result<()> {
    match &args.command {
        None => {
            init_tracing();
            ralphy_daemon::run(ralphy_daemon::DaemonConfig {
                port: args.port,
                bind: args.bind,
                allowed_hosts: args.allowed_hosts.clone(),
                peer_stores: args.peer_stores.clone(),
            })
        }
        Some(DaemonCommand::Setup {
            name: Some(name),
            avatar: Some(avatar),
        }) => setup_identity(
            &identity::daemon_toml_path()?,
            name,
            *avatar,
            &mut std::io::stdout(),
        ),
        Some(DaemonCommand::Setup { .. }) => setup::setup(args.port),
        Some(DaemonCommand::Status) => status(args.port),
        Some(DaemonCommand::Add { path, init, create }) => {
            let repo = bootstrap::resolve_or_init_repo(path, *init, *create)?;
            register::add_and_report(&repo)
        }
        Some(DaemonCommand::Remove { slug }) => {
            let removed = remove_repo_at(&registry::repos_toml_path()?, slug)?;
            if removed {
                outln!("removed {slug}");
            } else {
                outln!("{slug} was not registered");
            }
            Ok(())
        }
        Some(DaemonCommand::Install) => first_start::install(),
        Some(DaemonCommand::Uninstall) => {
            ralphy_daemon::autostart::uninstall()?;
            outln!("removed daemon autostart");
            Ok(())
        }
        Some(DaemonCommand::Restart { yes }) => restart::restart(*yes),
        Some(DaemonCommand::RequireToken { state }) => {
            require_token(&auth::store_dir()?, *state, &mut std::io::stdout())
        }
        Some(DaemonCommand::Describe { with_token }) => {
            describe::describe(&auth::store_dir()?, *with_token, &mut std::io::stdout())
        }
        Some(DaemonCommand::RotateToken) => {
            describe::rotate_token(&auth::store_dir()?, &mut std::io::stdout())
        }
    }
}

/// Baptism without questions: validate `name` and `avatar`, then persist the
/// identity at `path`. Mints no token, no TOTP seed and no password, and reads
/// no stdin.
fn setup_identity(path: &Path, name: &str, avatar: usize, out: &mut impl Write) -> Result<()> {
    let name = validate_name(name)?;
    let avatar = avatar_by_number(avatar)
        .with_context(|| format!("pick an avatar number from 1 to {}", AVATARS.len()))?;
    let id = identity::baptize(path, name, avatar.to_owned())?;
    writeln!(out, "baptized: {}", format_status_line(&id))?;
    Ok(())
}

/// Turn the require-token marker on or off in the store `dir`. `on` mints the
/// access token first when none exists, so the next start does not fail closed;
/// the token itself is never printed.
fn require_token(dir: &Path, state: OnOff, out: &mut impl Write) -> Result<()> {
    match state {
        OnOff::On => {
            let token_path = auth::token_path_in(dir);
            let (_, minted) = auth::ensure_token_at(&token_path)?;
            auth::set_require_token_in(dir, true)?;
            if minted {
                writeln!(
                    out,
                    "access token: created (stored in {})",
                    token_path.display()
                )?;
            } else {
                writeln!(out, "access token: already set")?;
            }
            writeln!(out, "require-token: on")?;
        }
        OnOff::Off => {
            auth::set_require_token_in(dir, false)?;
            writeln!(out, "require-token: off")?;
        }
    }
    writeln!(out, "run `ralphy daemon restart` to apply the change")?;
    Ok(())
}

/// Load the registry at `registry_path`, remove `slug`, and save it. Returns
/// whether an entry was actually removed (idempotent for callers).
fn remove_repo_at(registry_path: &Path, slug: &str) -> Result<bool> {
    let mut store = registry::load_from(registry_path)?;
    let removed = store.remove(slug);
    registry::save_to(&store, registry_path)?;
    Ok(removed)
}

/// Print the daemon's identity and listener hint, or a setup hint when the
/// daemon has not been baptized yet.
fn status(port: u16) -> Result<()> {
    match identity::load_current()? {
        Some(id) => outln!("{}", format_status_line(&id)),
        None => outln!("not set up — run `ralphy daemon setup`"),
    }
    outln!(
        "access token: {}",
        if auth::load_token()?.is_some() {
            "set"
        } else {
            "not set"
        }
    );
    outln!(
        "totp: {}",
        if totp::load_seed()?.is_some() {
            "enrolled"
        } else {
            "not enrolled"
        }
    );
    outln!(
        "password: {}",
        if password::load()?.is_some() {
            "set"
        } else {
            "not set"
        }
    );
    outln!("listener: http://127.0.0.1:{port}");
    match ralphy_daemon::autostart::status() {
        Ok(st) => {
            for line in autostart_lines(st) {
                outln!("{line}");
            }
        }
        Err(e) => {
            tracing::warn!(error = %e, "failed to probe daemon autostart status");
            outln!("autostart: unknown");
        }
    }
    Ok(())
}

/// The status lines of the autostart registration.
fn autostart_lines(st: ralphy_daemon::autostart::AutostartStatus) -> Vec<&'static str> {
    let mut lines = vec![match (st.registered, st.turned_off) {
        (true, true) => "autostart: turned off in Task Manager (Startup apps)",
        (true, false) => "autostart: registered",
        (false, _) => "autostart: not registered",
    }];
    if st.old_registration {
        lines.push(
            "autostart: an entry from an earlier version is still in the registry; \
             run `ralphy daemon install` to remove it",
        );
    }
    lines
}

/// Foreground logs to stderr: raw INFO `fmt` lines with local timestamps (the
/// same shape `run --verbose` prints), overridable via `RUST_LOG`/`RALPHY_LOG`.
/// No presenter — a resident process wants a scrollable log, not animation.
/// Colour only on a terminal: stderr is `daemon.log` under autostart and after
/// a restart, and escape codes in a file are noise.
fn init_tracing() {
    use std::io::IsTerminal;
    use tracing_subscriber::fmt::time::ChronoLocal;
    use tracing_subscriber::EnvFilter;

    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_timer(ChronoLocal::new("%Y-%m-%d %H:%M:%S".to_owned()))
        .with_ansi(std::io::stderr().is_terminal())
        .with_writer(std::io::stderr)
        .init();
}

#[cfg(test)]
mod tests;
