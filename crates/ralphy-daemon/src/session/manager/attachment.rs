//! A client's hold on a session: the replay it starts from, the live
//! stream, and the guard that releases its place on drop.

use std::sync::Arc;

use anyhow::Result;
use tokio::sync::{broadcast, watch};

use super::super::EvictToken;
use super::{replay_and_subscribe, ManagedSession, Subscribed};

/// Why an [`attach`] failed. `Unknown` → the id names no live session (`404`);
/// `Busy` → a single writer is already attached and `takeover` was not set (`409`);
/// `Superseded` → the slot's own holder already claimed it with a higher
/// attempt, so this claim (not a take over) is a late socket ([`attach_as`]).
///
/// [`attach`]: super::SessionManager::attach
/// [`attach_as`]: super::SessionManager::attach_as
#[derive(Debug)]
pub enum AttachError {
    Unknown,
    Busy,
    Superseded,
}

/// A live attachment to a session: the replay `snapshot` to send first (the
/// screen snapshot, or the raw ring once the screen is broken), a `rx`
/// for the live stream, a `size` receiver that holds the session size `(rows,
/// cols)` and changes when a resize by the writer changes it, and an `evict` token the
/// bridge waits on to learn WHY it ended (taken over, child exited, daemon
/// shutting down). `writer` is the role:
/// `true` for the single writer, `false` for a watcher. Dropping it releases the
/// writer slot (or deregisters the watcher) WITHOUT closing the session (the tmux
/// detach).
pub struct Attachment {
    pub snapshot: Vec<u8>,
    /// `snapshot` is the ring bytes after the `since` the attach named, for a
    /// terminal that keeps what it shows (ADR-0079 D8). `false`: it is the
    /// screen snapshot (or the raw ring), for a terminal that starts empty.
    pub resume: bool,
    /// The count of the session's output bytes at the end of `snapshot`. The
    /// live stream starts at this count.
    pub offset: u64,
    pub rx: broadcast::Receiver<Vec<u8>>,
    pub size: watch::Receiver<(u16, u16)>,
    /// The session size `(rows, cols)` `snapshot` was written at. `size` may
    /// already hold a later one.
    pub(crate) replay_size: (u16, u16),
    pub evict: Arc<EvictToken>,
    pub writer: bool,
    _guard: AttachGuard,
    pub(super) sess: Arc<ManagedSession>,
}

impl Attachment {
    pub(super) fn new(
        subscribed: Subscribed,
        token: Arc<EvictToken>,
        writer: bool,
        sess: Arc<ManagedSession>,
    ) -> Attachment {
        Attachment {
            snapshot: subscribed.replay,
            resume: subscribed.resume,
            offset: subscribed.offset,
            rx: subscribed.rx,
            size: subscribed.size,
            replay_size: subscribed.replay_size,
            evict: token.clone(),
            writer,
            _guard: AttachGuard {
                sess: sess.clone(),
                token,
                writer,
            },
            sess,
        }
    }

    /// Feed a client keystroke to the child. The single-writer policy makes this
    /// race-free with any other browser.
    ///
    /// A WATCHER's input is dropped here and reports `Ok(())` — a deliberate
    /// policy no-op, not a swallowed error: nothing failed, the client simply
    /// does not hold the baton, and returning `Err` would tear its bridge down
    /// for typing into a window it is allowed to keep watching.
    pub fn write(&self, bytes: &[u8]) -> Result<()> {
        if !self.writer {
            return Ok(());
        }
        self.sess.write(bytes)
    }

    /// Propagate a client resize to the PTY so the child's TUI reflows. A
    /// watcher's resize is dropped for the same reason as its keystrokes (see
    /// [`write`]) — one PTY has one geometry, and it belongs to the writer.
    ///
    /// [`write`]: Attachment::write
    pub fn resize(&self, rows: u16, cols: u16) -> Result<()> {
        if !self.writer {
            return Ok(());
        }
        self.sess.resize(rows, cols)
    }

    /// Start again from the screen as it is now, for a bridge whose live
    /// receiver lagged and lost bytes: a new `snapshot` (never a resume), its
    /// `offset`, a new `rx` and a new `size`, taken under the `kept` lock as an
    /// attach takes them, so the EXACTLY-ONCE REPLAY INVARIANT holds from here.
    pub fn resync(&mut self) {
        let subscribed = replay_and_subscribe(&self.sess.kept, &self.sess.tx, None);
        self.snapshot = subscribed.replay;
        self.resume = subscribed.resume;
        self.offset = subscribed.offset;
        self.rx = subscribed.rx;
        self.size = subscribed.size;
        self.replay_size = subscribed.replay_size;
    }

    /// Record `holder` and its `attempt` on the writer slot this attachment
    /// holds — how a fresh launch, which claims its slot inside the spawn,
    /// names its holder. A no-op for a watcher and for a slot another
    /// attachment has since taken.
    pub fn hold_as(&self, holder: &str, attempt: Option<u64>) {
        if !self.writer {
            return;
        }
        let mut slot = self.sess.attached.lock().expect("attached mutex");
        if let Some(writer) = slot.as_mut() {
            if Arc::ptr_eq(&writer.token, &self._guard.token) {
                writer.holder = Some(holder.to_owned());
                writer.attempt = attempt;
            }
        }
    }
}

/// Deregisters this attachment on drop. A WRITER clears the single-writer slot —
/// but ONLY when the slot still holds THIS attachment's token, so an evicted
/// incumbent's guard-drop does not clobber the taker's slot (`ptr_eq` mismatch),
/// which is what makes takeover race-free. A WATCHER removes exactly its own
/// token from the watcher list, by the same identity test.
struct AttachGuard {
    sess: Arc<ManagedSession>,
    token: Arc<EvictToken>,
    writer: bool,
}

impl Drop for AttachGuard {
    fn drop(&mut self) {
        if self.writer {
            let mut slot = self.sess.attached.lock().expect("attached mutex");
            if let Some(existing) = slot.as_ref() {
                if Arc::ptr_eq(&existing.token, &self.token) {
                    *slot = None;
                }
            }
        } else {
            self.sess
                .watchers
                .lock()
                .expect("watchers mutex")
                .retain(|w| !Arc::ptr_eq(w, &self.token));
        }
    }
}
