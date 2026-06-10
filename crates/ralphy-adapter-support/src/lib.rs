//! Adapter support: the shared, vendor-neutral machinery every Ralphy **adapter**
//! leans on. This crate owns the **OS-level headless plumbing** — spawn a child
//! `Command`, drain stdout/stderr without deadlocking, poll to
//! completion-or-timeout, kill on the deadline, and collect the captured output —
//! and nothing more.
//!
//! ## Why this does NOT reopen ADR-0004
//!
//! ADR-0004 states there is "deliberately no shared 'headless runner' that both
//! bend to fit." That prohibition is about a shared **`Outcome`-detection**
//! runner — the semantic completion protocol each vendor must shape itself. This
//! crate extracts **only the OS-level plumbing** (spawn / drain / poll / kill /
//! collect), which is identical by nature, not by imposition. It owns **no**
//! completion protocol and produces **no** `Outcome`: it hands back the raw,
//! still-separate stdout and stderr, and each adapter's `classify_*` function
//! still maps that captured output onto its own `Outcome`. The completion
//! semantics remain entirely per-adapter, so this extraction is the mechanical
//! floor *beneath* the seam ADR-0004 protects, not a violation of it. (This
//! rationale is recorded here so a future architecture review does not re-flag the
//! shared crate as an ADR-0004 violation.)
//!
//! The public surface speaks only `std` types ([`Command`], [`Duration`],
//! [`ExitStatus`], [`String`]) — no `portable-pty`, no vendor names. Building the
//! `Command` (binary, flags, env scrub) stays in each adapter, as does slicing the
//! returned [`HeadlessOutput`] into the adapter's own local return shape.

use std::io::{BufReader, Read, Write};
use std::process::{Command, ExitStatus};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};

/// The raw result of driving one headless child to completion or timeout.
///
/// `stdout` and `stderr` are kept **separate** (each captured as lossy-UTF-8) so
/// every adapter can combine or slice them as it needs — the OpenCode adapter
/// parses the JSON event stream from stdout alone, while Codex and Claude
/// concatenate the two. `exit` is `Some(status)` on a natural exit and `None`
/// exactly when the child was killed on the timeout deadline, letting each caller
/// recover its own `exited`/`exited_cleanly` flag from `std` types alone.
#[derive(Debug)]
pub struct HeadlessOutput {
    /// Everything the child wrote to stdout, captured complete (no truncation).
    pub stdout: String,
    /// Everything the child wrote to stderr, captured complete (no truncation).
    pub stderr: String,
    /// `true` when the child outlived `timeout` and was killed.
    pub timed_out: bool,
    /// The child's exit status, or `None` when it was killed on the deadline.
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
pub fn run_headless(mut cmd: Command, prompt: &str, timeout: Duration) -> Result<HeadlessOutput> {
    let mut child = cmd
        .spawn()
        .context("failed to spawn the headless child process")?;

    // Spawn the stdout/stderr reader threads *before* writing stdin, so a prompt
    // larger than the pipe buffer (~64KB) can't deadlock against a child that
    // starts emitting output before it finishes draining stdin.
    let mut stdin = child.stdin.take().expect("stdin was piped");
    let stdout = child.stdout.take().expect("stdout was piped");
    let stderr = child.stderr.take().expect("stderr was piped");

    let (tx_out, rx_out) = mpsc::channel::<Vec<u8>>();
    let (tx_err, rx_err) = mpsc::channel::<Vec<u8>>();
    thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = BufReader::new(stdout).read_to_end(&mut buf);
        let _ = tx_out.send(buf);
    });
    thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = BufReader::new(stderr).read_to_end(&mut buf);
        let _ = tx_err.send(buf);
    });

    stdin
        .write_all(prompt.as_bytes())
        .context("piping the prompt to the headless child")?;
    drop(stdin); // close stdin so the child sees EOF

    let deadline = Instant::now() + timeout;
    let mut timed_out = false;
    let exit = loop {
        if let Some(s) = child.try_wait().context("polling the headless child")? {
            break Some(s);
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            timed_out = true;
            break None;
        }
        thread::sleep(Duration::from_millis(500));
    };

    let collect = Duration::from_secs(5);
    let stdout_bytes = rx_out.recv_timeout(collect).unwrap_or_default();
    let stderr_bytes = rx_err.recv_timeout(collect).unwrap_or_default();
    Ok(HeadlessOutput {
        stdout: String::from_utf8_lossy(&stdout_bytes).into_owned(),
        stderr: String::from_utf8_lossy(&stderr_bytes).into_owned(),
        timed_out,
        exit,
    })
}
