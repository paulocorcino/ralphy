//! The page sockets a daemon still serves. axum 0.8.9's graceful shutdown
//! does not wait for an upgraded socket, so `serve` waits here, bounded,
//! after its listeners stop: the last frames of a socket (a console's
//! `session-end`) leave before the runtime is dropped (#834).

use std::time::Duration;

use tokio::sync::mpsc;

/// Gives each page socket a [`Hold`]. A router built with no [`Drain`]
/// (a test's, or the public `router`) gives holds that track nothing.
#[derive(Clone, Default)]
pub(crate) struct Sockets(Option<mpsc::WeakSender<()>>);

impl Sockets {
    /// The hold of one page socket, kept until the socket ends. After the
    /// drain started, a new socket is not tracked.
    pub(crate) fn hold(&self) -> Hold {
        Hold {
            _sender: self.0.as_ref().and_then(mpsc::WeakSender::upgrade),
        }
    }
}

/// Kept by a page socket while it runs. Only its drop matters.
pub(crate) struct Hold {
    _sender: Option<mpsc::Sender<()>>,
}

impl Hold {
    /// A hold that tracks nothing.
    #[cfg(test)]
    pub(crate) fn none() -> Hold {
        Hold { _sender: None }
    }
}

/// Waits for every page socket that holds a [`Hold`] to end.
pub(crate) struct Drain {
    held: mpsc::Sender<()>,
    ended: mpsc::Receiver<()>,
}

/// A [`Sockets`] for the router, and the [`Drain`] that waits for its sockets.
pub(crate) fn tracked() -> (Sockets, Drain) {
    // Nothing is ever sent: the channel closes when the last sender drops.
    let (held, ended) = mpsc::channel(1);
    (Sockets(Some(held.downgrade())), Drain { held, ended })
}

impl Drain {
    /// Wait up to `within` for every held socket to end. `false` when one
    /// was still open at the bound.
    pub(crate) async fn wait(self, within: Duration) -> bool {
        let Drain { held, mut ended } = self;
        drop(held);
        tokio::time::timeout(within, ended.recv()).await.is_ok()
    }
}
