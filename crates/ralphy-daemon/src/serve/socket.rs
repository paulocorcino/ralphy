//! `daemon.sock` — the daemon's second listener, a Unix socket inside its own
//! store. Several accounts on one host each run a daemon, and only one of them
//! can hold the TCP port; the socket path is unique per account, so a tunnel
//! from another computer can reach the right daemon by its store (#518, M1).
//!
//! The socket serves the same router as the TCP listener, so the auth policy
//! of the store applies to it unchanged. Its mode is `0600`: only the account
//! that owns the store can connect.

use std::io;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

/// The socket's file name inside the daemon store.
pub(crate) const SOCKET_FILE: &str = "daemon.sock";

/// The path of the daemon socket inside `store`.
pub fn socket_path(store: &Path) -> PathBuf {
    store.join(SOCKET_FILE)
}

/// The size of `sun_path` in `struct sockaddr_un` (`sys/un.h`): 108 bytes on
/// Linux, 104 on macOS and the BSDs. The path and its final NUL must fit.
fn max_len() -> usize {
    if cfg!(any(target_os = "linux", target_os = "android")) {
        108
    } else {
        104
    }
}

/// Whether `path` fits in `sun_path` with its final NUL.
fn fits(path: &Path) -> bool {
    path.as_os_str().len() < max_len()
}

/// The socket this process bound. It remembers the file's device and inode,
/// so that at exit the process removes its own file and never the file of a
/// daemon that started while this one was stopping.
pub struct BoundSocket {
    pub listener: tokio::net::UnixListener,
    path: PathBuf,
    dev: u64,
    ino: u64,
}

impl BoundSocket {
    /// The path the listener is bound to.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Remove the socket file if it is still the one this process bound. The
    /// same reasoning as `pidfile::clear_own_in`: when launchd relaunches its
    /// agent during a restart, the new daemon binds while the old one is still
    /// exiting, and a plain removal would delete the new daemon's socket.
    fn remove_own(path: &Path, dev: u64, ino: u64) {
        let Ok(meta) = std::fs::symlink_metadata(path) else {
            return;
        };
        if meta.dev() != dev || meta.ino() != ino {
            return;
        }
        if let Err(e) = std::fs::remove_file(path) {
            if e.kind() != io::ErrorKind::NotFound {
                tracing::warn!(path = %path.display(), error = %e, "could not remove the daemon socket");
            }
        }
    }

    /// Split into the listener and a cleanup that runs after the listener has
    /// stopped.
    pub fn into_parts(self) -> (tokio::net::UnixListener, impl FnOnce()) {
        let BoundSocket {
            listener,
            path,
            dev,
            ino,
        } = self;
        (listener, move || Self::remove_own(&path, dev, ino))
    }
}

/// Bind `daemon.sock` in `store`.
///
/// - `Ok(None)` when the path is too long for a Unix socket; the daemon then
///   serves TCP only, and the log says why.
/// - An error when a daemon of this account already answers on the socket.
/// - A socket file that nobody answers is left over from a daemon that did not
///   exit cleanly, and it is replaced.
pub fn bind(store: &Path) -> Result<Option<BoundSocket>> {
    let path = socket_path(store);
    if !fits(&path) {
        tracing::warn!(
            path = %path.display(),
            limit = max_len() - 1,
            "the store path is too long for a Unix socket; this daemon listens on TCP only"
        );
        return Ok(None);
    }
    crate::owner_only::create_owner_only_dir(store)?;
    if std::os::unix::net::UnixStream::connect(&path).is_ok() {
        bail!(
            "binding the daemon socket on {}: a daemon of this account already listens there",
            path.display()
        );
    }
    match std::fs::remove_file(&path) {
        Ok(()) => tracing::info!(path = %path.display(), "replaced a stale daemon socket"),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => {
            return Err(e).with_context(|| format!("removing the stale socket {}", path.display()))
        }
    }
    // Bind, then chmod, before the listener is handed to the server. The
    // process umask is not changed: it is process-wide, and the runtime's other
    // threads may create files at the same moment. The window between bind and
    // chmod is closed by the store itself: it was just made `0700`, so no other
    // account can reach the socket, and no connection is accepted before the
    // chmod.
    let std_listener = std::os::unix::net::UnixListener::bind(&path)
        .with_context(|| format!("binding the daemon socket on {}", path.display()))?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
        .with_context(|| format!("setting the mode of {}", path.display()))?;
    let meta =
        std::fs::symlink_metadata(&path).with_context(|| format!("reading {}", path.display()))?;
    std_listener
        .set_nonblocking(true)
        .context("making the daemon socket non-blocking")?;
    let listener = tokio::net::UnixListener::from_std(std_listener)
        .context("registering the daemon socket with the runtime")?;
    Ok(Some(BoundSocket {
        listener,
        path,
        dev: meta.dev(),
        ino: meta.ino(),
    }))
}

/// Whether a failed TCP bind stops the daemon. With the require-token marker a
/// busy port is not fatal: another account's daemon holds it, and this daemon
/// serves only its socket, which a tunnel reaches by path (#518, M4). Every
/// other failure, and a busy port without the marker, stays fatal.
pub(crate) fn tcp_bind_failure_is_fatal(kind: io::ErrorKind, require_token: bool) -> bool {
    !(kind == io::ErrorKind::AddrInUse && require_token)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_busy_port_is_fatal_only_without_the_marker() {
        assert!(!tcp_bind_failure_is_fatal(io::ErrorKind::AddrInUse, true));
        assert!(tcp_bind_failure_is_fatal(io::ErrorKind::AddrInUse, false));
        assert!(tcp_bind_failure_is_fatal(
            io::ErrorKind::PermissionDenied,
            true
        ));
    }

    #[test]
    fn a_path_over_the_limit_binds_nothing() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let long = dir.path().join("x".repeat(max_len()));
        let bound = bind(&long).expect("a long path is not an error");
        assert!(bound.is_none());
        assert!(
            !long.exists(),
            "nothing is created for a path that cannot bind"
        );
    }
}
