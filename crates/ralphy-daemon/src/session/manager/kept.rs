//! What the daemon keeps of one session's output, and the two calls that
//! touch it under its lock: the pump's [`publish`] and an attach's
//! [`replay_and_subscribe`].

use std::collections::VecDeque;
use std::sync::Mutex;

use tokio::sync::{broadcast, watch};

use crate::session::screen::{self, Screen};
use crate::session::SessionId;

/// Append `bytes` to the scrollback `ring`, then drop from the FRONT until it is
/// no longer over `cap` — a byte-bounded ring so a chatty session cannot grow the
/// daemon's memory without bound (issue #166 AC2). Front-drop is intentional:
/// scrollback keeps the most RECENT output; the truncated seam is resynchronized
/// by the live stream that follows the replay.
pub(super) fn push_capped(ring: &mut VecDeque<u8>, bytes: &[u8], cap: usize) {
    ring.extend(bytes.iter().copied());
    while ring.len() > cap {
        ring.pop_front();
    }
}

/// Per-session scrollback cap. A byte bound (not a line bound) is the simplest
/// structure that satisfies the "chatty session cannot grow memory unboundedly"
/// AC; a byte cap may truncate an escape sequence at the replay seam, which
/// xterm.js resynchronizes on the live stream that follows.
pub(super) const SCROLLBACK_CAP_BYTES: usize = 512 * 1024;

/// What the daemon keeps of one session's output, under ONE lock: the raw
/// ring and the screen the emulator builds from the same bytes (ADR-0079 D1,
/// D12), and the byte counts a resume is checked against (D8). Only
/// [`publish`] feeds them, so all of them see every byte in the same order.
pub(super) struct KeptOutput {
    session: SessionId,
    pub(super) ring: VecDeque<u8>,
    /// `None` once the emulator panicked: every later attach then gets the
    /// raw ring (ADR-0079 D3).
    pub(super) screen: Option<Screen>,
    /// The session size `(rows, cols)`, clamped. Kept here and not read from
    /// `screen`, which a panic removes. Every attachment holds a receiver
    /// (ADR-0079 D6).
    size: watch::Sender<(u16, u16)>,
    /// Every byte the session wrote, also the ones the ring dropped. The ring
    /// holds the last `ring.len()` of them.
    pub(super) written: u64,
    /// `written` at the last resize that changed the size. The bytes after it were
    /// written at the size the session has now.
    resized_at: u64,
}

impl KeptOutput {
    /// Empty, with a screen of `rows` x `cols` (clamped).
    pub(super) fn new(session: SessionId, rows: u16, cols: u16) -> Self {
        KeptOutput {
            session,
            ring: VecDeque::new(),
            screen: Some(Screen::new(rows, cols, screen::SCROLLBACK_LINES)),
            size: watch::Sender::new(screen::clamp_size(rows, cols)),
            written: 0,
            resized_at: 0,
        }
    }

    pub(super) fn push(&mut self, chunk: &[u8]) {
        push_capped(&mut self.ring, chunk, SCROLLBACK_CAP_BYTES);
        self.written += chunk.len() as u64;
        self.on_screen(|screen| screen.feed(chunk));
    }

    /// Gives the screen the new size, and tells the attachments. A resize to
    /// the size the session has changes nothing: the screen keeps its scroll
    /// region, a resume stays valid, and nobody is told. Called only for a
    /// size the PTY accepted.
    pub(super) fn resize(&mut self, rows: u16, cols: u16) {
        let size = screen::clamp_size(rows, cols);
        if *self.size.borrow() == size {
            return;
        }
        self.resized_at = self.written;
        self.on_screen(|screen| screen.resize(rows, cols));
        self.size.send_replace(size);
    }

    /// The replay of an attach, and whether it resumes at `since` (ADR-0079
    /// D8). A resume is the ring bytes after `since`, when `since` is inside
    /// the ring, not before the last resize and not after the count.
    /// Otherwise it is the screen snapshot, or the raw ring once the screen is
    /// broken. Empty while the session has written nothing, so a fresh
    /// session sends no replay frame; an empty resume sends none either.
    pub(super) fn replay(&mut self, since: Option<u64>) -> (Vec<u8>, bool) {
        if let Some(tail) = since.and_then(|since| self.tail_after(since)) {
            return (tail, true);
        }
        if self.ring.is_empty() {
            return (Vec::new(), false);
        }
        if let Some(snapshot) = self.on_screen(Screen::snapshot) {
            return (snapshot, false);
        }
        let (front, back) = self.ring.as_slices();
        ([front, back].concat(), false)
    }

    /// The ring bytes after the byte count `since`, or `None` when they are
    /// not all in the ring or were not all written at the current size.
    fn tail_after(&self, since: u64) -> Option<Vec<u8>> {
        let start = self.written.saturating_sub(self.ring.len() as u64);
        if since < start || since < self.resized_at || since > self.written {
            return None;
        }
        // `since - start` is at most `ring.len()`, so it fits in a usize.
        let skip = usize::try_from(since - start).ok()?;
        let (front, back) = self.ring.as_slices();
        Some(if skip <= front.len() {
            [&front[skip..], back].concat()
        } else {
            back[skip - front.len()..].to_vec()
        })
    }

    /// Runs `f` on the screen, unless it is broken. A panic inside the
    /// emulator is caught and breaks the screen for good: the output is
    /// untrusted (ADR-0072 D11), and a panic must not end the pump. The lock
    /// is not poisoned, because the panic does not leave this call.
    pub(super) fn on_screen<R>(&mut self, f: impl FnOnce(&mut Screen) -> R) -> Option<R> {
        let screen = self.screen.as_mut()?;
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(screen))) {
            Ok(result) => Some(result),
            Err(_) => {
                tracing::warn!(
                    session = self.session,
                    "the console screen failed; a reattach of this session now gets the raw output"
                );
                self.screen = None;
                None
            }
        }
    }
}

/// Keep `chunk` and send it to every attachment.
///
/// EXACTLY-ONCE REPLAY INVARIANT: the `kept` lock is held across BOTH the
/// keep AND `tx.send`, mirrored by [`replay_and_subscribe`] holding it across
/// replay+subscribe. So an attach never gets a byte both in its replay and on
/// its live receiver, and never misses one in between. The byte count moves
/// with the keep, so the `offset` an attach gets is the count of exactly the
/// bytes that are not on its live receiver.
pub(super) fn publish(kept: &Mutex<KeptOutput>, tx: &broadcast::Sender<Vec<u8>>, chunk: Vec<u8>) {
    let mut kept = kept.lock().expect("kept output mutex");
    kept.push(&chunk);
    #[expect(
        clippy::let_underscore_must_use,
        reason = "a send with no receiver is a no-op: the kept bytes serve the next attach"
    )]
    let _ = tx.send(chunk);
    drop(kept);
}

/// What an attach gets under the `kept` lock.
pub(super) struct Subscribed {
    pub(super) replay: Vec<u8>,
    /// The replay is the ring tail after the attach's `since`, not a snapshot.
    pub(super) resume: bool,
    /// The byte count at the end of the replay.
    pub(super) offset: u64,
    pub(super) rx: broadcast::Receiver<Vec<u8>>,
    pub(super) size: watch::Receiver<(u16, u16)>,
    /// The session size `(rows, cols)` the replay was written at.
    pub(super) replay_size: (u16, u16),
}

/// The replay of an attach, its byte count, its live receiver and its size
/// receiver, taken under the `kept` lock (see [`publish`]). The writer and
/// every watcher come here, so with no `since` they get the same replay.
/// `replay_size` is the size the replay was written at; `size` may change
/// after the lock is released.
pub(super) fn replay_and_subscribe(
    kept: &Mutex<KeptOutput>,
    tx: &broadcast::Sender<Vec<u8>>,
    since: Option<u64>,
) -> Subscribed {
    let mut kept = kept.lock().expect("kept output mutex");
    let (replay, resume) = kept.replay(since);
    let offset = kept.written;
    #[cfg(test)]
    between_replay_and_subscribe::run();
    let rx = tx.subscribe();
    let size = kept.size.subscribe();
    let replay_size = *size.borrow();
    drop(kept);
    Subscribed {
        replay,
        resume,
        offset,
        rx,
        size,
        replay_size,
    }
}

/// A point inside [`replay_and_subscribe`], after the replay and before the
/// subscribe, where a test runs a step of its own on the attaching thread.
/// It lets a test try a publish at the one moment the lock must keep out.
#[cfg(test)]
pub(super) mod between_replay_and_subscribe {
    use std::cell::RefCell;

    thread_local! {
        static STEP: RefCell<Option<Box<dyn FnOnce()>>> = RefCell::new(None);
    }

    /// Runs `step` at the point, in the next attach on this thread only.
    pub(in crate::session::manager) fn set(step: impl FnOnce() + 'static) {
        STEP.with(|s| *s.borrow_mut() = Some(Box::new(step)));
    }

    pub(super) fn run() {
        if let Some(step) = STEP.with(|s| s.borrow_mut().take()) {
            step();
        }
    }
}
