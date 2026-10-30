//! The per-line side effects of the reader threads of the headless runner: the
//! early-kill switch, the degraded-state switch, the streamed log and the idle
//! beacon.

use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::Instant;

use crate::idle::ProgressBeat;

/// A shared early-kill signal. The stderr reader thread flips `fired` the moment a
/// line matches `pred`; the poll loop observes it on its next tick and kills the
/// process tree — so a child that has already emitted its terminal signal (a
/// provider usage-limit line on `--print-logs` stderr) is reaped in ~sub-second
/// instead of idling in silent backoff until the wall `timeout`. The predicate runs
/// against **stderr only**: that is where the provider's own quota/rate-limit lines
/// surface, whereas stdout carries the agent's own output (which may legitimately
/// mention "rate limit" and must not trip the switch).
/// A boxed early-kill predicate over a single (stderr) line.
pub(super) type LinePredicate = Box<dyn Fn(&str) -> bool + Send + Sync>;

pub(super) struct KillSwitch {
    pub(super) fired: AtomicBool,
    pub(super) pred: LinePredicate,
}

/// A shared **degraded-state** signal. Each reader thread flips ITS OWN stream's
/// bool per line — `true` when a line matches the vendor's degraded predicate,
/// `false` on the next healthy line — and the poll loop samples the OR of the two
/// ([`any_active`](Self::any_active)) to advance a [`DegradedWatch`] clock. Unlike
/// [`KillSwitch`] the predicate runs against **both** streams: a vendor's
/// retry/degraded banner can surface on stdout (the JSON event stream) or stderr
/// (`--print-logs`), and a failure banner is not progress on either. A matched line
/// also skips the idle beacon, so a child emitting *only* degraded lines is still
/// reaped by the idle watchdog rather than kept alive by its own retry noise.
///
/// The state is kept PER STREAM (not one shared bool) so a healthy companion
/// stream cannot reset the clock: an opencode child that shows a stall on stdout
/// while its `--print-logs` stderr keeps heartbeating would otherwise store `false`
/// on every stderr line and the degraded stretch would never reach the 3-min ping.
pub(super) struct DegradedSwitch {
    pub(super) stdout_active: AtomicBool,
    pub(super) stderr_active: AtomicBool,
    pub(super) pred: LinePredicate,
}

impl DegradedSwitch {
    /// Record this stream's latest degraded-ness (one stream never clobbers the
    /// other's — see the struct doc).
    fn store(&self, on_stderr: bool, degraded: bool) {
        let slot = if on_stderr {
            &self.stderr_active
        } else {
            &self.stdout_active
        };
        slot.store(degraded, Ordering::Release);
    }

    /// Whether EITHER stream's most recent line was degraded.
    pub(super) fn any_active(&self) -> bool {
        self.stdout_active.load(Ordering::Acquire) || self.stderr_active.load(Ordering::Acquire)
    }
}

/// What one reader thread does with each line it reads, besides keeping the bytes.
pub(super) struct LineWatch {
    pub(super) log: Option<Arc<Mutex<fs::File>>>,
    pub(super) switch: Option<Arc<KillSwitch>>,
    pub(super) beat: Option<Arc<ProgressBeat>>,
    pub(super) degraded: Option<Arc<DegradedSwitch>>,
    pub(super) on_stderr: bool,
}

impl LineWatch {
    /// Run every per-line side effect, in this order: publish the degraded state,
    /// beat (not on a degraded line), tee, then the early-kill switch.
    fn on_line(&self, line: &[u8]) {
        let is_degraded = self.publish_degraded(line);
        // Any output at all is the headless progress signal: a child still
        // talking is a child still working. Both streams feed the same beacon
        // (unlike the early-kill switch, which is stderr-only) — a child
        // narrating only on stdout is just as alive. A degraded line is the one
        // exception: it must NOT rearm the beacon, or a child stuck retrying an
        // API failure would look alive forever and never be idle-reaped.
        if !is_degraded {
            if let Some(b) = &self.beat {
                b.beat(Instant::now());
            }
        }
        self.tee_line(line);
        self.check_switch(line);
    }

    /// Publish the degraded state for the poll loop: `true` on a matching line,
    /// `false` on any other. Computed before the beat because a degraded line is
    /// a failure banner, not progress.
    fn publish_degraded(&self, line: &[u8]) -> bool {
        let is_degraded = self
            .degraded
            .as_ref()
            .is_some_and(|d| (d.pred)(&String::from_utf8_lossy(line)));
        if let Some(d) = &self.degraded {
            d.store(self.on_stderr, is_degraded);
        }
        is_degraded
    }

    fn tee_line(&self, line: &[u8]) {
        if let Some(f) = &self.log {
            if let Ok(mut f) = f.lock() {
                #[expect(
                    clippy::let_underscore_must_use,
                    reason = "the streamed log is a live copy; the reader keeps the bytes in memory"
                )]
                let _ = f.write_all(line);
                #[expect(
                    clippy::let_underscore_must_use,
                    reason = "the streamed log is a live copy; the reader keeps the bytes in memory"
                )]
                let _ = f.flush();
            }
        }
    }

    fn check_switch(&self, line: &[u8]) {
        if let Some(sw) = &self.switch {
            if !sw.fired.load(Ordering::Relaxed) && (sw.pred)(&String::from_utf8_lossy(line)) {
                sw.fired.store(true, Ordering::Release);
            }
        }
    }
}

/// Spawn one reader thread that drains `reader` line by line: it accumulates the
/// full bytes for the return value, optionally **tees** each line to `log` as it
/// arrives (so the on-disk log is live and survives a crash of this process), and
/// optionally runs an early-kill `switch` against each line. Reading `read_until`
/// a newline (rather than `read_to_end`) is what makes the tee and the switch fire
/// incrementally; a final partial line with no trailing newline is still captured
/// when the pipe reaches EOF.
pub(super) fn spawn_reader<R: Read + Send + 'static>(
    reader: R,
    tx: mpsc::Sender<Vec<u8>>,
    watch: LineWatch,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let mut br = BufReader::new(reader);
        let mut all = Vec::new();
        let mut line = Vec::new();
        loop {
            line.clear();
            match br.read_until(b'\n', &mut line) {
                Ok(0) | Err(_) => break, // EOF, or a read error we cannot recover from
                Ok(_) => {}
            }
            all.extend_from_slice(&line);
            watch.on_line(&line);
        }
        #[expect(
            clippy::let_underscore_must_use,
            reason = "the receiver is gone once the run gave up on this stream"
        )]
        let _ = tx.send(all);
    })
}
