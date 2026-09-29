//! The host side of pairing: `ralphy daemon describe` tells a computer that
//! adds this one as a host what it needs, and `ralphy daemon rotate-token`
//! changes the access token. Both run over SSH, so neither reads stdin.

use std::io::Write;
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};
use ralphy_daemon::peer::{self, DaemonDescription, PEER_PROTOCOL_VERSION};
use ralphy_daemon::{auth, identity, pidfile};

/// The port a daemon was started with: `--port N` or `--port=N` in its
/// recorded arguments, else the default.
pub(crate) fn port_from_args(args: &[String]) -> u16 {
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        let value = match arg.strip_prefix("--port=") {
            Some(v) => Some(v.to_string()),
            None if arg == "--port" => it.next().cloned(),
            None => None,
        };
        if let Some(port) = value.and_then(|v| v.parse().ok()) {
            return port;
        }
    }
    ralphy_daemon::DEFAULT_PORT
}

/// Print this daemon's [`DaemonDescription`] as one JSON line. The token is
/// included only with `with_token`.
pub(crate) fn describe(dir: &Path, with_token: bool, out: &mut impl Write) -> Result<()> {
    let id = identity::load_from(&dir.join("daemon.toml"))?;
    let port = port_from_args(&pidfile::read_args_in(dir));
    let autostart = match ralphy_daemon::autostart::status() {
        Ok(s) => s.registered,
        Err(e) => {
            tracing::warn!(error = %format!("{e:#}"), "could not read the daemon autostart");
            false
        }
    };
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let running = TcpStream::connect_timeout(&addr, Duration::from_millis(500)).is_ok();
    let token = if with_token {
        auth::load_token_from(&auth::token_path_in(dir))?
    } else {
        None
    };
    let description = DaemonDescription {
        daemon_id: id.as_ref().map(|i| i.id.to_string()),
        name: id.as_ref().map(|i| i.name.clone()),
        avatar: id.as_ref().map(|i| i.avatar.clone()),
        environment: peer::detect_environment(),
        os: std::env::consts::OS.to_string(),
        port,
        protocol_version: PEER_PROTOCOL_VERSION,
        require_token: auth::require_token_enabled_in(dir)?,
        autostart,
        running,
        token,
    };
    let line = serde_json::to_string(&description).context("serializing the daemon description")?;
    writeln!(out, "{line}")?;
    Ok(())
}

/// Replace the access token in the store `dir`. The new token is never printed.
pub(crate) fn rotate_token(dir: &Path, out: &mut impl Write) -> Result<()> {
    auth::save_token_to(&auth::generate_token(), &auth::token_path_in(dir))?;
    writeln!(out, "access token: changed")?;
    writeln!(out, "run `ralphy daemon restart` to apply the change")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn port_from_args_reads_both_spellings() {
        assert_eq!(port_from_args(&args(&["daemon", "--port", "7300"])), 7300);
        assert_eq!(port_from_args(&args(&["daemon", "--port=7301"])), 7301);
    }

    #[test]
    fn port_from_args_defaults() {
        assert_eq!(port_from_args(&[]), ralphy_daemon::DEFAULT_PORT);
        assert_eq!(
            port_from_args(&args(&["daemon", "--port"])),
            ralphy_daemon::DEFAULT_PORT
        );
        assert_eq!(
            port_from_args(&args(&["daemon", "--port", "x"])),
            ralphy_daemon::DEFAULT_PORT
        );
    }
}
