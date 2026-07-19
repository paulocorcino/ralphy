//! The OS-level headless runner: spawn a child `Command`, drain stdout/stderr
//! without deadlocking, poll to completion-or-timeout, kill on the deadline,
//! and collect the captured output.

use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Command, ExitStatus};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};

use crate::degraded::{DegradedAction, DegradedWatch};
use crate::idle::{IdleWatch, ProgressBeat};

/// The raw result of driving one headless child to completion or timeout.
///
/// `stdout` and `stderr` are kept **separate** (each captured as lossy-UTF-8) so
/// every adapter can combine or slice them as it needs — the OpenCode adapter
/// parses the JSON event stream from stdout alone, while Codex and Claude
/// concatenate the two. `exit` is `Some(status)` on a natural exit and `None`
/// exactly when the child was killed — either on the timeout deadline (then
/// `timed_out` is set) or on an early-kill signal (then it is not) — letting each
/// caller recover its own `exited`/`exited_cleanly` flag from `std` types alone.
#[derive(Debug)]
pub struct HeadlessOutput {
    /// Everything the child wrote to stdout, captured complete (no truncation).
    pub stdout: String,
    /// Everything the child wrote to stderr, captured complete (no truncation).
    pub stderr: String,
    /// `true` when the child outlived `timeout` and was killed.
    pub timed_out: bool,
    /// `true` when the kill was the **idle watchdog** rather than the wall clock:
    /// the child went silent past its idle window (docs/adr/0038). `timed_out` is
    /// set alongside it on purpose — every downstream classifier keeps treating
    /// this as the timeout it already understands (ADR-0023 ladder untouched).
    ///
    /// The operator-facing signal is the `IDLE_REAPED_MSG` event, not this flag;
    /// this is the return-value counterpart, for a caller with no tracing
    /// subscriber attached (the integration tests assert on it directly).
    pub idle_killed: bool,
    /// The child's exit status, or `None` when it was killed (deadline or signal).
    pub exit: Option<ExitStatus>,
}

/// A shared early-kill signal. The stderr reader thread flips `fired` the moment a
/// line matches `pred`; the poll loop observes it on its next tick and kills the
/// process tree — so a child that has already emitted its terminal signal (a
/// provider usage-limit line on `--print-logs` stderr) is reaped in ~sub-second
/// instead of idling in silent backoff until the wall `timeout`. The predicate runs
/// against **stderr only**: that is where the provider's own quota/rate-limit lines
/// surface, whereas stdout carries the agent's own output (which may legitimately
/// mention "rate limit" and must not trip the switch).
/// A boxed early-kill predicate over a single (stderr) line.
type LinePredicate = Box<dyn Fn(&str) -> bool + Send + Sync>;

struct KillSwitch {
    fired: AtomicBool,
    pred: LinePredicate,
}

/// A shared **degraded-state** signal. Both reader threads flip `active` per line
/// — `true` when a line matches the vendor's degraded predicate, `false` on the
/// next healthy line — and the poll loop samples it to advance a [`DegradedWatch`]
/// clock. Unlike [`KillSwitch`] the predicate runs against **both** streams: a
/// vendor's retry/degraded banner can surface on stdout (the JSON event stream) or
/// stderr (`--print-logs`), and a failure banner is not progress on either. A
/// matched line also skips the idle beacon, so a child emitting *only* degraded
/// lines is still reaped by the idle watchdog rather than kept alive by its own
/// retry noise.
struct DegradedSwitch {
    active: AtomicBool,
    pred: LinePredicate,
}

/// Spawn one reader thread that drains `reader` line by line: it accumulates the
/// full bytes for the return value, optionally **tees** each line to `log` as it
/// arrives (so the on-disk log is live and survives a crash of this process), and
/// optionally runs an early-kill `switch` against each line. Reading `read_until`
/// a newline (rather than `read_to_end`) is what makes the tee and the switch fire
/// incrementally; a final partial line with no trailing newline is still captured
/// when the pipe reaches EOF.
fn spawn_reader<R: Read + Send + 'static>(
    reader: R,
    tx: mpsc::Sender<Vec<u8>>,
    log: Option<Arc<Mutex<fs::File>>>,
    switch: Option<Arc<KillSwitch>>,
    beat: Option<Arc<ProgressBeat>>,
    degraded: Option<Arc<DegradedSwitch>>,
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
            // Publish the degraded state for the poll loop: `true` on a matching
            // line, `false` on any other. Computed before the beat because a
            // degraded line is a failure banner, not progress.
            let is_degraded = degraded
                .as_ref()
                .is_some_and(|d| (d.pred)(&String::from_utf8_lossy(&line)));
            if let Some(d) = &degraded {
                d.active.store(is_degraded, Ordering::Release);
            }
            // Any output at all is the headless progress signal: a child still
            // talking is a child still working. Both streams feed the same beacon
            // (unlike the early-kill switch, which is stderr-only) — a child
            // narrating only on stdout is just as alive. A degraded line is the one
            // exception: it must NOT rearm the beacon, or a child stuck retrying an
            // API failure would look alive forever and never be idle-reaped.
            if !is_degraded {
                if let Some(b) = &beat {
                    b.beat(Instant::now());
                }
            }
            if let Some(f) = &log {
                if let Ok(mut f) = f.lock() {
                    let _ = f.write_all(&line);
                    let _ = f.flush();
                }
            }
            if let Some(sw) = &switch {
                if !sw.fired.load(Ordering::Relaxed) && (sw.pred)(&String::from_utf8_lossy(&line)) {
                    sw.fired.store(true, Ordering::Release);
                }
            }
        }
        let _ = tx.send(all);
    })
}

/// Spawn `cmd`, pipe `prompt` on its stdin, drain stdout/stderr to completion or
/// timeout, killing the child if it outlives `timeout`. `cmd` must already have
/// stdin/stdout/stderr set to [`Stdio::piped()`](std::process::Stdio::piped); the
/// adapter builds the rest (binary, flags, env scrub).
///
/// The reader threads start *before* the prompt is written so a prompt larger than
/// the pipe buffer (~64KB) can't deadlock against a child that begins emitting
/// output before it finishes draining stdin. The wall poll ticks every 500ms; on
/// the deadline the child is killed and reaped and `timed_out`/`exit = None` are
/// reported. Output is then collected with a 5s grace so a child that flushed late
/// is still captured complete.
pub fn run_headless(cmd: Command, prompt: &str, timeout: Duration) -> Result<HeadlessOutput> {
    drive_headless(cmd, prompt, timeout, None, None, IdleWatch::default(), None)
}

/// The shared spawn/drain/poll/kill/collect core behind [`run_headless`] and the
/// logged variants. `log`, when set, receives every stdout+stderr line as it
/// arrives (streamed tee); `switch`, when set, early-kills the child the moment a
/// stderr line matches its predicate.
fn drive_headless(
    mut cmd: Command,
    prompt: &str,
    timeout: Duration,
    log: Option<Arc<Mutex<fs::File>>>,
    switch: Option<Arc<KillSwitch>>,
    idle: IdleWatch,
    degraded: Option<Arc<DegradedSwitch>>,
) -> Result<HeadlessOutput> {
    // On Unix, run the child in its own process group so a timeout can signal the
    // whole tree, not just the direct child. An agent CLI that spawned helpers
    // would otherwise leave a grandchild holding the stdout pipe open, blocking the
    // reader forever and forcing the collect grace to return empty — silently
    // dropping the very output the limit/auth detectors scan. Shared with the
    // verify gate via `ralphy-proc-util` so both set up a killable tree identically.
    ralphy_proc_util::own_process_group(&mut cmd);
    // Hidden console on Windows: this child (an agent CLI) has its stdio piped and
    // may run under the console-less daemon-dispatched `ralphy`, where a bare
    // console child would flash a visible window. No-op off Windows.
    ralphy_proc_util::no_window(&mut cmd);

    let mut child = cmd
        .spawn()
        .context("failed to spawn the headless child process")?;

    // Spawn the stdout/stderr reader threads *before* writing stdin, so a prompt
    // larger than the pipe buffer (~64KB) can't deadlock against a child that
    // starts emitting output before it finishes draining stdin. A misconfigured
    // `Command` (no piped stdio) degrades to a run error, not a panic.
    let mut stdin = child
        .stdin
        .take()
        .context("headless child stdin was not piped")?;
    let stdout = child
        .stdout
        .take()
        .context("headless child stdout was not piped")?;
    let stderr = child
        .stderr
        .take()
        .context("headless child stderr was not piped")?;

    let (tx_out, rx_out) = mpsc::channel::<Vec<u8>>();
    let (tx_err, rx_err) = mpsc::channel::<Vec<u8>>();
    // The idle beacon starts now: the spawn itself counts as progress, so a child
    // slow to emit its first line is not declared idle on arrival. `None` when the
    // watchdog is off, which keeps the per-line cost at exactly zero.
    let beat = idle.window().map(|_| ProgressBeat::new(Instant::now()));
    // The early-kill switch watches stderr only (see `KillSwitch`); stdout is teed
    // to the same log but never trips the switch.
    // The degraded switch watches BOTH streams (see `DegradedSwitch`): the same
    // Arc is handed to each reader so a match on either stream feeds one clock.
    let out_handle = spawn_reader(
        stdout,
        tx_out,
        log.clone(),
        None,
        beat.clone(),
        degraded.clone(),
    );
    let err_handle = spawn_reader(
        stderr,
        tx_err,
        log,
        switch.clone(),
        beat.clone(),
        degraded.clone(),
    );

    // A broken pipe here means the child exited before draining stdin — its own
    // signal, not a fatal error. Warn and fall through to the poll loop, which
    // reaps the child, rather than `?`-returning with the child still unreaped.
    let stdin_result = stdin.write_all(prompt.as_bytes());
    drop(stdin); // close stdin so the child sees EOF
    if let Err(e) = stdin_result {
        tracing::warn!(error = %e, "writing the prompt to the headless child failed (it likely exited early)");
    }

    let deadline = Instant::now() + timeout;
    let mut timed_out = false;
    let mut idle_killed = false;
    // The API-degraded clock, only when a predicate was supplied — otherwise the
    // poll loop pays nothing for it. It never kills the child; it only turns a
    // persistent degraded stretch into a matched pair of tracing events.
    let mut degraded_watch = degraded.as_ref().map(|_| DegradedWatch::new());
    let exit = loop {
        if let Some(s) = child.try_wait().context("polling the headless child")? {
            break Some(s);
        }
        // An early-kill line landed on stderr: reap now instead of idling in the
        // child's own silent backoff until the wall deadline. `timed_out` stays
        // false — this is an explicit terminal signal from the child, not a timeout.
        if switch
            .as_ref()
            .is_some_and(|sw| sw.fired.load(Ordering::Acquire))
        {
            tracing::info!(
                "headless child emitted an early-kill line on stderr — reaping now instead of waiting out the wall timeout"
            );
            ralphy_proc_util::kill_tree(&mut child);
            break None;
        }
        if Instant::now() >= deadline {
            ralphy_proc_util::kill_tree(&mut child);
            timed_out = true;
            break None;
        }
        // The idle watchdog (docs/adr/0038): the child has emitted nothing on
        // either stream for the whole window, so it is wedged rather than slow —
        // a silently-retried provider quota block, a hung request, a deadlock.
        // Reaped as a timeout so classification is unchanged; only the log says
        // which clock fired.
        if let Some(b) = &beat {
            if idle.expired(b, Instant::now()) {
                tracing::info!(
                    idle_minutes = idle.window().map(|w| w.as_secs() / 60).unwrap_or(0),
                    "{}",
                    crate::idle::IDLE_REAPED_MSG
                );
                ralphy_proc_util::kill_tree(&mut child);
                timed_out = true;
                idle_killed = true;
                break None;
            }
        }
        // The API-degraded clock: a persistent degraded stretch (≥ ping) emits the
        // shared degraded/recovered events, so the operator sees the same signal
        // the PTY path surfaces. Advisory only — never kills the child.
        if let (Some(sw), Some(w)) = (&degraded, &mut degraded_watch) {
            match w.poll(Instant::now(), sw.active.load(Ordering::Acquire)) {
                DegradedAction::Degraded => tracing::info!("{}", crate::degraded::API_DEGRADED_MSG),
                DegradedAction::Recovered => {
                    tracing::info!("{}", crate::degraded::API_RECOVERED_MSG)
                }
                DegradedAction::None => {}
            }
        }
        thread::sleep(Duration::from_millis(500));
    };

    // Collect with a bounded grace so a child that flushed late is still captured.
    // After a natural exit (or `kill_tree`) the pipes reach EOF and the readers
    // finish, so this normally returns the full buffer; on the rare stuck reader we
    // warn (a truncated capture is observable) and leak that one thread rather than
    // block the whole run on it.
    let collect = Duration::from_secs(5);
    let stdout_bytes = recv_and_join(&rx_out, out_handle, collect, "stdout");
    let stderr_bytes = recv_and_join(&rx_err, err_handle, collect, "stderr");
    Ok(HeadlessOutput {
        stdout: String::from_utf8_lossy(&stdout_bytes).into_owned(),
        stderr: String::from_utf8_lossy(&stderr_bytes).into_owned(),
        timed_out,
        idle_killed,
        exit,
    })
}

/// The post-processed result of [`run_headless_logged`]: the raw stdout kept
/// apart (OpenCode parses its JSON event stream from stdout alone), the combined
/// `log` exactly as persisted, and the two recovered flags every headless adapter
/// derives from a [`HeadlessOutput`].
///
/// `exited_cleanly` is a **successful** exit (`exit.map(|s| s.success())`), `false`
/// when the child was killed on the wall timeout — distinct from Claude's `exited`
/// flag (`!timed_out`), which the Claude adapter still recovers itself from
/// `timed_out`.
#[derive(Debug)]
pub struct HeadlessRun {
    /// Everything the child wrote to stdout, captured complete (no truncation).
    pub stdout: String,
    /// stdout + stderr concatenated, exactly as written to `log_path`.
    pub log: String,
    /// `true` when the child exited with a success status (not killed, not failed).
    pub exited_cleanly: bool,
    /// `true` when the child outlived the timeout and was killed.
    pub timed_out: bool,
    /// `true` when the idle watchdog fired rather than the wall clock — see
    /// [`HeadlessOutput::idle_killed`]. Diagnostics only; `timed_out` is set too,
    /// so every classifier behaves exactly as before.
    pub idle_killed: bool,
    /// The raw numeric exit code, `None` when killed on the timeout. For adapters
    /// like Kimi that map a specific code (e.g. 75 → `Limit`), the boolean
    /// `exited_cleanly` erases this — keep both.
    pub exit_code: Option<i32>,
}

/// One configured headless call: the command plus the optional guards that drive
/// it (streamed log, stderr early-kill, idle watchdog).
///
/// A builder rather than another `run_headless_logged_*` free function: the two
/// existing entry points already encoded their options in the *name*, so each new
/// guard would double the constructors. The named functions remain as thin
/// wrappers, so existing call sites and import paths are untouched.
pub struct HeadlessCall<'a> {
    cmd: Command,
    prompt: &'a str,
    timeout: Duration,
    log_path: &'a Path,
    kill_on_stderr_line: Option<LinePredicate>,
    idle: IdleWatch,
    degraded_line: Option<LinePredicate>,
}

impl<'a> HeadlessCall<'a> {
    /// A call with only the wall `timeout` and the streamed log at `log_path`.
    pub fn new(cmd: Command, prompt: &'a str, timeout: Duration, log_path: &'a Path) -> Self {
        Self {
            cmd,
            prompt,
            timeout,
            log_path,
            kill_on_stderr_line: None,
            idle: IdleWatch::default(),
            degraded_line: None,
        }
    }

    /// Reap the child as soon as a **stderr** line matches — the child's own
    /// terminal signal, so the run classifies identically, only faster.
    pub fn kill_on_stderr_line(
        mut self,
        pred: impl Fn(&str) -> bool + Send + Sync + 'static,
    ) -> Self {
        self.kill_on_stderr_line = Some(Box::new(pred));
        self
    }

    /// Arm the idle watchdog: reap the child after `minutes` with no output on
    /// either stream. `0` leaves it disabled (the default).
    pub fn idle_minutes(mut self, minutes: u64) -> Self {
        self.idle = IdleWatch::from_minutes(minutes);
        self
    }

    /// [`idle_minutes`](Self::idle_minutes) with an exact window — used by tests
    /// to drive the real kill path in seconds.
    pub fn idle_window(mut self, window: Duration) -> Self {
        self.idle = IdleWatch::from_window(window);
        self
    }

    /// Supply the vendor's **degraded-line** predicate. A matched line (on either
    /// stream) is treated as a retry/failure banner rather than progress: it does
    /// not rearm the idle beacon, and a stretch persisting ≥3 min emits the shared
    /// `API_DEGRADED_MSG` / `API_RECOVERED_MSG` events (docs/adr/0038 normalization
    /// over the headless path). The predicate never kills the child — a false
    /// negative degrades gracefully to today's behaviour.
    pub fn degraded_line(mut self, pred: impl Fn(&str) -> bool + Send + Sync + 'static) -> Self {
        self.degraded_line = Some(Box::new(pred));
        self
    }

    /// Drive the call to completion, timeout, early-kill or idle-kill.
    pub fn run(self) -> Result<HeadlessRun> {
        run_headless_logged_impl(
            self.cmd,
            self.prompt,
            self.timeout,
            self.log_path,
            self.kill_on_stderr_line,
            self.idle,
            self.degraded_line,
        )
    }
}

/// [`run_headless`] plus the post-run shell every headless adapter repeats: combine
/// stdout+stderr into one log, persist it at `log_path`, and recover
/// `exited_cleanly` from the exit status. The log is **streamed to `log_path` as it
/// arrives** (so the run is observable live and survives a crash of this process),
/// then rewritten once at the end in the canonical stdout-then-stderr order.
/// Returns a [`HeadlessRun`]; the adapter keeps its own `classify_*` and (for
/// Claude) its own `exited = !timed_out` recovery from `timed_out`.
pub fn run_headless_logged(
    cmd: Command,
    prompt: &str,
    timeout: Duration,
    log_path: &Path,
) -> Result<HeadlessRun> {
    HeadlessCall::new(cmd, prompt, timeout, log_path).run()
}

/// [`run_headless_logged`] with an **early-kill** predicate over stderr lines. The
/// moment a stderr line matches `kill_on_stderr_line`, the child is reaped instead
/// of being left to idle in its own silent backoff until the wall `timeout` — the
/// OpenCode adapter passes its usage-limit matcher here so a provider quota block
/// (which only ever prints to `--print-logs` stderr and never reaches the JSON
/// stream) ends the call in ~sub-second rather than burning the full per-issue
/// budget. The predicate must match the same signal the caller's post-run
/// classifier keys on, so the early-killed run classifies identically to one that
/// ran to the deadline — only faster.
pub fn run_headless_logged_watched(
    cmd: Command,
    prompt: &str,
    timeout: Duration,
    log_path: &Path,
    kill_on_stderr_line: impl Fn(&str) -> bool + Send + Sync + 'static,
) -> Result<HeadlessRun> {
    HeadlessCall::new(cmd, prompt, timeout, log_path)
        .kill_on_stderr_line(kill_on_stderr_line)
        .run()
}

fn run_headless_logged_impl(
    cmd: Command,
    prompt: &str,
    timeout: Duration,
    log_path: &Path,
    kill_on_stderr_line: Option<LinePredicate>,
    idle: IdleWatch,
    degraded_line: Option<LinePredicate>,
) -> Result<HeadlessRun> {
    // Open the log up front so both streams can be teed to it as they arrive: the
    // run stays observable live and the partial output survives a crash of THIS
    // process, instead of the old capture-everything-in-RAM-then-write-once. A failed
    // open (missing dir, permissions) degrades to `None` → the in-memory capture and
    // the single final write below, exactly the prior behaviour.
    let sink = fs::File::create(log_path)
        .ok()
        .map(|f| Arc::new(Mutex::new(f)));
    let switch = kill_on_stderr_line.map(|pred| {
        Arc::new(KillSwitch {
            fired: AtomicBool::new(false),
            pred,
        })
    });
    let degraded = degraded_line.map(|pred| {
        Arc::new(DegradedSwitch {
            active: AtomicBool::new(false),
            pred,
        })
    });

    let r = drive_headless(cmd, prompt, timeout, sink, switch, idle, degraded)?;
    let stdout = r.stdout;
    let mut log = stdout.clone();
    log.push_str(&r.stderr);
    // Rewrite the log once in the canonical stdout-then-stderr order. The streamed
    // file was interleaved by arrival; this final write keeps the persisted file
    // byte-identical to the returned `log` the detectors scan (and deterministic for
    // tests). A mid-run crash leaves the interleaved partial, which is fine for
    // forensics — the canonical rewrite only matters once the run completed.
    let _ = fs::write(log_path, &log);
    let exit = r.exit;
    let exited_cleanly = exit.map(|s| s.success()).unwrap_or(false);
    Ok(HeadlessRun {
        stdout,
        log,
        exited_cleanly,
        timed_out: r.timed_out,
        idle_killed: r.idle_killed,
        exit_code: exit.and_then(|s| s.code()),
    })
}

/// Await one reader thread's captured bytes within `grace`, then join it. On a
/// natural exit or after [`kill_tree`] the pipe hits EOF and the thread sends
/// promptly, so the join is immediate; if the grace elapses the thread is still
/// blocked (a descendant survived) — warn that the capture may be truncated and
/// leak that one thread instead of blocking the run on a join that would hang.
fn recv_and_join(
    rx: &mpsc::Receiver<Vec<u8>>,
    handle: thread::JoinHandle<()>,
    grace: Duration,
    stream: &str,
) -> Vec<u8> {
    match rx.recv_timeout(grace) {
        Ok(buf) => {
            let _ = handle.join();
            buf
        }
        Err(_) => {
            tracing::warn!(
                stream,
                "headless reader did not finish within the collect grace — output may be truncated"
            );
            Vec::new()
        }
    }
}
