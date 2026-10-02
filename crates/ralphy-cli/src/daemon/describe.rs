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

/// What a connection to the store's socket showed.
#[cfg_attr(not(unix), allow(dead_code))]
struct SocketProbe {
    path: std::path::PathBuf,
    answers: bool,
}

/// Try the daemon socket in `dir`. `None` when there is no probe to make: on
/// Windows, or when the path is too long for a socket address.
#[cfg(unix)]
fn probe_socket(dir: &Path) -> Option<SocketProbe> {
    let path = ralphy_daemon::socket::socket_path(dir);
    if !ralphy_daemon::socket::fits(&path) {
        return None;
    }
    let path = std::fs::canonicalize(dir)
        .map(|d| ralphy_daemon::socket::socket_path(&d))
        .unwrap_or(path);
    let answers = std::os::unix::net::UnixStream::connect(&path).is_ok();
    Some(SocketProbe { path, answers })
}

#[cfg(not(unix))]
fn probe_socket(_dir: &Path) -> Option<SocketProbe> {
    None
}

/// Whether the daemon is running. A socket that answers says yes. When the
/// socket was probed and does not answer, the TCP port is not asked, because
/// another account's daemon may hold it; the process in this store's pid file
/// decides, so an older daemon with no socket still counts. Without a probe
/// (Windows, or a path over the socket limit) the TCP connect decides.
fn liveness(
    socket_answers: Option<bool>,
    own_daemon_alive: impl FnOnce() -> bool,
    tcp_answers: impl FnOnce() -> bool,
) -> bool {
    match socket_answers {
        Some(true) => true,
        Some(false) => own_daemon_alive(),
        None => tcp_answers(),
    }
}

/// Whether the pid file of `dir` names a live process that runs the program it
/// recorded. The same proof `daemon restart` asks for before it ends a process.
fn own_daemon_alive(dir: &Path) -> bool {
    let Some(pid) = pidfile::read_in(dir) else {
        return false;
    };
    if !ralphy_proc_util::pid::pid_is_alive(pid) {
        return false;
    }
    match (
        pidfile::read_exe_in(dir),
        ralphy_proc_util::pid::exe_of_pid(pid),
    ) {
        (Some(recorded), Some(running)) => {
            // Linux reports a replaced binary as `<path> (deleted)`.
            let running = running.to_string_lossy();
            let running = running.strip_suffix(" (deleted)").unwrap_or(&running);
            super::restart::same_program(&recorded, Path::new(running))
        }
        _ => false,
    }
}

fn tcp_answers(port: u16) -> bool {
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    TcpStream::connect_timeout(&addr, Duration::from_millis(500)).is_ok()
}

/// Whether the daemon of the store `dir` answers: on its socket when the socket
/// can be probed, else on its TCP port.
pub(crate) fn answers(dir: &Path, port: u16) -> bool {
    liveness(
        probe_socket(dir).map(|s| s.answers),
        || false,
        || tcp_answers(port),
    )
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
    let socket = probe_socket(dir);
    let socket_answers = socket.as_ref().map(|s| s.answers);
    let running = liveness(
        socket_answers,
        || own_daemon_alive(dir),
        || tcp_answers(port),
    );
    let socket = socket
        .filter(|s| s.answers)
        .map(|s| s.path.to_string_lossy().into_owned());
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
        socket,
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
    fn running_follows_the_socket_on_unix() {
        assert!(liveness(Some(true), || false, || false));
        assert!(
            !liveness(Some(false), || false, || true),
            "another account may hold the port"
        );
        assert!(
            liveness(Some(false), || true, || false),
            "an older daemon of this account has no socket"
        );
    }

    #[test]
    fn a_path_over_the_limit_falls_back_to_the_port() {
        assert!(liveness(None, || false, || true));
        assert!(!liveness(None, || true, || false));
    }

    #[cfg(unix)]
    #[test]
    fn a_listening_socket_is_reported_with_its_path() {
        let store = tempfile::tempdir().unwrap();
        let path = ralphy_daemon::socket::socket_path(store.path());
        let listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
        let probe = probe_socket(store.path()).unwrap();
        assert!(probe.answers);
        assert_eq!(probe.path.file_name(), path.file_name());
        drop(listener);
        // The file stays but nothing listens: a refused connect is not an answer.
        assert!(!probe_socket(store.path()).unwrap().answers);
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
