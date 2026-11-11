//! The logged entry points over [`run_headless`](super::run_headless): the
//! [`HeadlessCall`] builder, its [`HeadlessRun`] result, and the thin wrappers
//! that keep the older call sites.

use std::fs;
use std::path::Path;
use std::process::Command;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Result;

use super::lines::{DegradedSwitch, KillSwitch, LinePredicate};
use super::{drive_headless, Guards, PromptGate};
use crate::idle::IdleWatch;

/// The post-processed result of [`run_headless_logged`]: the raw stdout kept
/// apart (`OpenCode` parses its JSON event stream from stdout alone), the combined
/// `log` exactly as persisted, and the two recovered flags every headless adapter
/// derives from a [`HeadlessOutput`](super::HeadlessOutput).
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
    /// [`HeadlessOutput::idle_killed`](super::HeadlessOutput::idle_killed).
    /// Diagnostics only; `timed_out` is set too,
    /// so every classifier behaves exactly as before.
    pub idle_killed: bool,
    /// `true` when the operator's stop reaped the child — see
    /// [`HeadlessOutput::stopped`](super::HeadlessOutput::stopped).
    /// Diagnostics only, and `timed_out` is NOT set
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

/// [`run_headless`](super::run_headless) plus the post-run shell every headless adapter
/// repeats: combine
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
/// `OpenCode` adapter passes its usage-limit matcher here so a provider quota block
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
    let exited_cleanly = r.exited_cleanly();
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
