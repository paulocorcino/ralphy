//! `ralphy daemon`: run the resident daemon in the foreground (docs/adr/0032).
//! The CLI is only the composition root — it installs a plain tracing stack for
//! readable foreground logs and hands off to `ralphy-daemon`, where the async
//! runtime lives. `install`/`status`/`uninstall` (OS autostart, mirroring
//! `schedule`) come in later slices.

use anyhow::Result;
use clap::Args;

#[derive(Args)]
pub(crate) struct DaemonArgs {
    /// TCP port for the local listener. The daemon binds 127.0.0.1 only; a
    /// non-localhost bind is a future explicit opt-in (docs/adr/0032 §4).
    #[arg(long, default_value_t = ralphy_daemon::DEFAULT_PORT)]
    pub(crate) port: u16,
}

pub(crate) fn run(args: &DaemonArgs) -> Result<()> {
    init_tracing();
    ralphy_daemon::run(ralphy_daemon::DaemonConfig { port: args.port })
}

/// Foreground logs to stderr: raw INFO `fmt` lines with local timestamps (the
/// same shape `run --verbose` prints), overridable via `RUST_LOG`/`RALPHY_LOG`.
/// No presenter — a resident process wants a scrollable log, not animation.
fn init_tracing() {
    use tracing_subscriber::fmt::time::ChronoLocal;
    use tracing_subscriber::EnvFilter;

    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_timer(ChronoLocal::new("%Y-%m-%d %H:%M:%S".to_string()))
        .with_writer(std::io::stderr)
        .init();
}
