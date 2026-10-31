//! The OS-level headless runner: spawn a child `Command`, drain stdout/stderr
//! without deadlocking, poll to completion-or-timeout, kill on the deadline,
//! and collect the captured output.

use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, ExitStatus};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};

use crate::degraded::{DegradedAction, DegradedWatch};
use crate::idle::{IdleWatch, ProgressBeat};

mod lines;
use lines::{spawn_reader, DegradedSwitch, KillSwitch, LinePredicate, LineWatch};

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
    /// `true` when the kill was the operator's own stop (docs/adr/0054) rather
    /// than any clock. Unlike `idle_killed`, `timed_out` is deliberately left
    /// FALSE alongside it — the same treatment the early-kill switch gets, and
    /// for the same reason: this is an explicit terminal signal, not a deadline.
    ///
    /// It is a discriminator for the caller, never an input to classification:
    /// it must not reach `CompletionSignals`. What the run reports about a stop
    /// is the `StopReason`, which the operator's own act earned; what the
    /// ADR-0023 ladder reports about the ATTEMPT is unchanged (`Stuck` — the
    /// child ended with no verdict, which is exactly what happened).
    pub stopped: bool,
    /// The child's exit status, or `None` when it was killed (deadline or signal).
    pub exit: Option<ExitStatus>,
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
    drive_headless(cmd, prompt, timeout, Guards::default())
}

/// [`run_headless`] with the prompt held behind `gate`; the shared spawn of the
/// held one-shot sessions (`json_session`).
pub(crate) fn run_headless_held(
    cmd: Command,
    prompt: &str,
    timeout: Duration,
    gate: PromptGate<'_>,
) -> Result<HeadlessOutput> {
    let guards = Guards {
        gate: Some(gate),
        ..Guards::default()
    };
    drive_headless(cmd, prompt, timeout, guards)
}

/// A check that runs after the child is spawned and before its prompt is
/// written. `Ok` releases the prompt. `Err` kills the child, sends no prompt,
/// and is returned to the caller of the run.
pub type PromptGate<'a> = Box<dyn FnOnce() -> Result<()> + 'a>;

/// The context of a gate error. Its type lets a caller in this crate tell a
/// refusal by the gate apart from a failure to spawn the child.
#[derive(Debug, thiserror::Error)]
#[error("the prompt was held and not sent")]
pub(crate) struct PromptHeld;

/// The optional guards of one headless drive: the streamed log, the stderr
/// early-kill, the idle watchdog, the degraded-line watch and the prompt gate.
#[derive(Default)]
struct Guards<'a> {
    log: Option<Arc<Mutex<fs::File>>>,
    switch: Option<Arc<KillSwitch>>,
    idle: IdleWatch,
    degraded: Option<Arc<DegradedSwitch>>,
    gate: Option<PromptGate<'a>>,
}

/// The shared spawn/drain/poll/kill/collect core behind [`run_headless`] and the
/// logged variants. `log`, when set, receives every stdout+stderr line as it
/// arrives (streamed tee); `switch`, when set, early-kills the child the moment a
/// stderr line matches its predicate.
fn drive_headless(
    cmd: Command,
    prompt: &str,
    timeout: Duration,
    guards: Guards<'_>,
) -> Result<HeadlessOutput> {
    let Guards {
        log,
        switch,
        idle,
        degraded,
        gate,
    } = guards;
    let (mut child, stdin, stdout, stderr) = spawn_piped(cmd)?;

    // Spawn the stdout/stderr reader threads *before* writing stdin, so a prompt
    // larger than the pipe buffer (~64KB) can't deadlock against a child that
    // starts emitting output before it finishes draining stdin.
    let (tx_out, rx_out) = mpsc::channel::<Vec<u8>>();
    let (tx_err, rx_err) = mpsc::channel::<Vec<u8>>();
    // The idle beacon starts now: the spawn itself counts as progress, so a child
    // slow to emit its first line is not declared idle on arrival. `None` when the
    // watchdog is off, which keeps the per-line cost at exactly zero.
    let beat = idle.window().map(|_| ProgressBeat::new(Instant::now()));
    // The early-kill switch watches stderr only (see `KillSwitch`); stdout is teed
    // to the same log but never trips the switch.
    // The degraded switch watches BOTH streams (see `DegradedSwitch`): the same
    // Arc is handed to each reader, but each writes its OWN stream's slot (the
    // `false`/`true` on_stderr flag) so a healthy companion stream can't reset the
    // clock the other stream's degraded stretch is accumulating.
    let out_handle = spawn_reader(
        stdout,
        tx_out,
        LineWatch {
            log: log.clone(),
            switch: None,
            beat: beat.clone(),
            degraded: degraded.clone(),
            on_stderr: false,
        },
    );
    let err_handle = spawn_reader(
        stderr,
        tx_err,
        LineWatch {
            log,
            switch: switch.clone(),
            beat: beat.clone(),
            degraded: degraded.clone(),
            on_stderr: true,
        },
    );

    if let Err(e) = release_prompt(gate, stdin, prompt, &mut child) {
        // The kill came first, so the readers reach EOF; collect them so no
        // thread outlives the call.
        collect_both(&rx_out, out_handle, &rx_err, err_handle);
        return Err(e);
    }

    let deadline = Instant::now() + timeout;
    // The API-degraded clock, only when a predicate was supplied — otherwise the
    // poll loop pays nothing for it. It never kills the child; it only turns a
    // persistent degraded stretch into a matched pair of tracing events.
    let mut degraded_watch = degraded.as_ref().map(|_| DegradedWatch::new());
    let (exit, reap) = loop {
        match child.try_wait() {
            Ok(Some(s)) => break (Some(s), None),
            Ok(None) => {}
            Err(e) => {
                let e = anyhow::Error::new(e).context("polling the headless child");
                return Err(teardown_after_error(
                    &mut child, &rx_out, out_handle, &rx_err, err_handle, e,
                ));
            }
        }
        if let Some(reap) = reap_reason(switch.as_deref(), deadline, beat.as_deref(), &idle) {
            break (None, Some(reap));
        }
        poll_degraded(degraded.as_deref(), degraded_watch.as_mut());
        thread::sleep(Duration::from_millis(500));
    };

    // Teardown BEFORE collection, on every exit path — including a NATURAL exit.
    // An agent CLI that backgrounded a helper leaves that descendant holding the
    // inherited stdout/stderr write-ends open after the CLI itself exits; without
    // this kill the readers never reach EOF and the collect grace below returned
    // EMPTY — silently dropping the very output the limit/auth detectors scan
    // (the cursor #244 shape: a live run classified Stuck with its evidence
    // discarded). On the early-kill/timeout/idle paths this is where the tree
    // dies; on a natural exit it reaps whatever the CLI leaked.
    ralphy_proc_util::kill_tree(&mut child);

    let (stdout_bytes, stderr_bytes) = collect_both(&rx_out, out_handle, &rx_err, err_handle);
    Ok(HeadlessOutput {
        stdout: String::from_utf8_lossy(&stdout_bytes).into_owned(),
        stderr: String::from_utf8_lossy(&stderr_bytes).into_owned(),
        timed_out: matches!(reap, Some(Reap::Wall | Reap::Idle)),
        idle_killed: matches!(reap, Some(Reap::Idle)),
        stopped: matches!(reap, Some(Reap::Stopped)),
        exit,
    })
}

/// Kill the child tree and collect both readers, so an error that ends the drive
/// early leaves no process and no thread behind. Returns `error` for the caller.
fn teardown_after_error(
    child: &mut Child,
    rx_out: &mpsc::Receiver<Vec<u8>>,
    out_handle: thread::JoinHandle<()>,
    rx_err: &mpsc::Receiver<Vec<u8>>,
    err_handle: thread::JoinHandle<()>,
    error: anyhow::Error,
) -> anyhow::Error {
    ralphy_proc_util::kill_tree(child);
    collect_both(rx_out, out_handle, rx_err, err_handle);
    error
}

/// Spawn `cmd` with its three pipes taken.
fn spawn_piped(mut cmd: Command) -> Result<(Child, ChildStdin, ChildStdout, ChildStderr)> {
    // On Unix, run the child in its own process group so a timeout can signal the
    // whole tree, not just the direct child. An agent CLI that spawned helpers
    // would otherwise leave a grandchild holding the stdout pipe open, blocking the
    // reader forever and forcing the collect grace to return empty — silently
    // dropping the very output the limit/auth detectors scan. Shared with the
    // verify gate via `ralphy-proc-util` so both set up a killable tree identically.
    ralphy_proc_util::own_process_group(&mut cmd);
    // An agent CLI resolved from a directory that is not on `PATH` (nvm's global
    // bin, `~/.local/bin`) may need its interpreter from that same directory —
    // an npm shim's `#!/usr/bin/env node` finds nothing otherwise. Keeps
    // `locate_program`'s "detection and execution can never disagree".
    ralphy_proc_util::program_dir_on_path(&mut cmd);
    // Hidden console on Windows: this child (an agent CLI) has its stdio piped and
    // may run under the console-less daemon-dispatched `ralphy`, where a bare
    // console child would flash a visible window. No-op off Windows.
    ralphy_proc_util::no_window(&mut cmd);

    let mut child = cmd
        .spawn()
        .context("failed to spawn the headless child process")?;
    // A misconfigured `Command` (no piped stdio) degrades to a run error, not a
    // panic. The child is already running, so it is killed before the error returns.
    match take_pipes(&mut child) {
        Ok((stdin, stdout, stderr)) => Ok((child, stdin, stdout, stderr)),
        Err(e) => {
            ralphy_proc_util::kill_tree(&mut child);
            Err(e)
        }
    }
}

/// The three pipes of a spawned child, or an error that names the first one that
/// was not piped.
fn take_pipes(child: &mut Child) -> Result<(ChildStdin, ChildStdout, ChildStderr)> {
    let stdin = child
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
    Ok((stdin, stdout, stderr))
}

/// Why the poll loop reaped a live child.
enum Reap {
    EarlyKill,
    Stopped,
    Wall,
    Idle,
}

/// The reason to reap the child on this tick, if any. The order is the contract:
/// early-kill, operator stop, wall deadline, idle watchdog.
fn reap_reason(
    switch: Option<&KillSwitch>,
    deadline: Instant,
    beat: Option<&ProgressBeat>,
    idle: &IdleWatch,
) -> Option<Reap> {
    // An early-kill line landed on stderr: reap now instead of idling in the
    // child's own silent backoff until the wall deadline. `timed_out` stays
    // false — this is an explicit terminal signal from the child, not a timeout.
    if switch.is_some_and(|sw| sw.fired.load(Ordering::Acquire)) {
        tracing::info!(
            "headless child emitted an early-kill line on stderr — reaping now instead of waiting out the wall timeout"
        );
        return Some(Reap::EarlyKill);
    }
    // The operator's stop (docs/adr/0054). Checked BEFORE the clocks so a
    // stop landing on the same tick as a deadline is reported as the button,
    // and placed here — in `drive_headless` rather than in each adapter —
    // because every child-driving entry point, plan phases included, funnels
    // through this one loop. That is the whole payoff of the flag being process-global:
    // every vendor gains a working stop without a builder change.
    if ralphy_core::stop::requested() {
        tracing::info!(
            "operator stop — reaping the headless child instead of waiting out its budget"
        );
        return Some(Reap::Stopped);
    }
    if Instant::now() >= deadline {
        return Some(Reap::Wall);
    }
    // The idle watchdog (docs/adr/0038): the child has emitted nothing on
    // either stream for the whole window, so it is wedged rather than slow —
    // a silently-retried provider quota block, a hung request, a deadlock.
    // Reaped as a timeout so classification is unchanged; only the log says
    // which clock fired.
    if let Some(b) = beat {
        if idle.expired(b, Instant::now()) {
            ralphy_core::emit::idle_reaped(idle.window().map(|w| w.as_secs() / 60).unwrap_or(0));
            return Some(Reap::Idle);
        }
    }
    None
}

/// The API-degraded clock: a persistent degraded stretch (≥ ping) emits the
/// shared degraded/recovered events, so the operator sees the same signal
/// the PTY path surfaces. Advisory only — never kills the child.
fn poll_degraded(degraded: Option<&DegradedSwitch>, watch: Option<&mut DegradedWatch>) {
    if let (Some(sw), Some(w)) = (degraded, watch) {
        match w.poll(Instant::now(), sw.any_active()) {
            DegradedAction::Degraded => ralphy_core::emit::api_degraded(),
            DegradedAction::Recovered => ralphy_core::emit::api_recovered(),
            DegradedAction::None => {}
        }
    }
}

/// Collect the captured output of both streams. The tree-kill closed every
/// write-end, so the readers hit EOF and deliver the full buffer; the grace is a
/// backstop against a kill that failed to close a handle — then that stream's
/// capture is dropped and the stuck reader leaked rather than blocking the whole
/// run.
fn collect_both(
    rx_out: &mpsc::Receiver<Vec<u8>>,
    out_handle: thread::JoinHandle<()>,
    rx_err: &mpsc::Receiver<Vec<u8>>,
    err_handle: thread::JoinHandle<()>,
) -> (Vec<u8>, Vec<u8>) {
    let grace = Duration::from_secs(5);
    let stdout = recv_and_join(rx_out, out_handle, grace, "stdout");
    let stderr = recv_and_join(rx_err, err_handle, grace, "stderr");
    (stdout, stderr)
}

// Run the gate, then write the prompt and close stdin. A gate error kills the
/// process tree BEFORE stdin drops: a child that sees EOF on an empty stdin
/// could otherwise act on it before the kill lands. No prompt is sent then.
///
/// A broken pipe on the write means the child exited before draining stdin: its
/// own signal, not a fatal error. It is logged, and the poll loop reaps the
/// child.
fn release_prompt(
    gate: Option<PromptGate<'_>>,
    mut stdin: ChildStdin,
    prompt: &str,
    child: &mut Child,
) -> Result<()> {
    if let Some(gate) = gate {
        if let Err(e) = gate() {
            ralphy_proc_util::kill_tree(child);
            drop(stdin);
            return Err(e.context(PromptHeld));
        }
    }
    let stdin_result = stdin.write_all(prompt.as_bytes());
    drop(stdin); // close stdin so the child sees EOF
    if let Err(e) = stdin_result {
        tracing::warn!(error = %e, "writing the prompt to the headless child failed (it likely exited early)");
    }
    Ok(())
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
    /// `true` when the operator's stop reaped the child — see
    /// [`HeadlessOutput::stopped`]. Diagnostics only, and `timed_out` is NOT set
    /// alongside it, so a stop never reads as a timeout to a caller or a test.
    pub stopped: bool,
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
    gate: Option<PromptGate<'a>>,
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
            gate: None,
        }
    }

    /// Hold the prompt behind `gate`: the child is spawned and its output is
    /// read, but the prompt is written only after the gate returns `Ok`. On
    /// `Err` the child is killed, no prompt is sent, and [`run`](Self::run)
    /// returns that error.
    pub fn hold_prompt(mut self, gate: PromptGate<'a>) -> Self {
        self.gate = Some(gate);
        self
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
        run_headless_logged_impl(self)
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

fn run_headless_logged_impl(call: HeadlessCall<'_>) -> Result<HeadlessRun> {
    let HeadlessCall {
        cmd,
        prompt,
        timeout,
        log_path,
        kill_on_stderr_line,
        idle,
        degraded_line,
        gate,
    } = call;
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
            stdout_active: AtomicBool::new(false),
            stderr_active: AtomicBool::new(false),
            pred,
        })
    });

    let guards = Guards {
        log: sink,
        switch,
        idle,
        degraded,
        gate,
    };
    let r = drive_headless(cmd, prompt, timeout, guards)?;
    let stdout = r.stdout;
    let mut log = stdout.clone();
    log.push_str(&r.stderr);
    // Rewrite the log once in the canonical stdout-then-stderr order. The streamed
    // file was interleaved by arrival; this final write keeps the persisted file
    // byte-identical to the returned `log` the detectors scan (and deterministic for
    // tests). A mid-run crash leaves the interleaved partial, which is fine for
    // forensics — the canonical rewrite only matters once the run completed.
    #[expect(
        clippy::let_underscore_must_use,
        reason = "the file is a diagnostic copy; the caller gets the same text in the return value"
    )]
    let _ = fs::write(log_path, &log);
    let exit = r.exit;
    let exited_cleanly = exit.map(|s| s.success()).unwrap_or(false);
    Ok(HeadlessRun {
        stdout,
        log,
        exited_cleanly,
        timed_out: r.timed_out,
        idle_killed: r.idle_killed,
        stopped: r.stopped,
        exit_code: exit.and_then(|s| s.code()),
    })
}

/// Await one reader thread's captured bytes within `grace`, then join it. The
/// caller tree-killed the child first, so the pipe hits EOF and the thread sends
/// promptly — the join is immediate. The reader sends its buffer exactly once,
/// at EOF, so if the grace elapses (a write-end the kill could not close) there
/// is nothing partial to recover: this stream's capture is DROPPED, and the
/// stuck thread leaked instead of blocking the run on a join that would hang.
fn recv_and_join(
    rx: &mpsc::Receiver<Vec<u8>>,
    handle: thread::JoinHandle<()>,
    grace: Duration,
    stream: &str,
) -> Vec<u8> {
    match rx.recv_timeout(grace) {
        Ok(buf) => {
            #[expect(
                clippy::let_underscore_must_use,
                reason = "the reader already sent its bytes; a panic in it leaves nothing to keep"
            )]
            let _ = handle.join();
            buf
        }
        Err(_) => {
            tracing::warn!(
                stream,
                "headless reader still blocked after the tree-kill — this stream's captured output was dropped"
            );
            Vec::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_held_keeps_its_text() {
        let err: &dyn std::error::Error = &PromptHeld;
        assert_eq!(err.to_string(), "the prompt was held and not sent");
    }
}
