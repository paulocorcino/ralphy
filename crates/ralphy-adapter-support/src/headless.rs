//! The OS-level headless runner: spawn a child `Command`, drain stdout/stderr
//! without deadlocking, poll to completion-or-timeout, kill on the deadline,
//! and collect the captured output.

use std::fs;
use std::io::{BufReader, Read, Write};
use std::path::Path;
use std::process::{Command, ExitStatus, Stdio};
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
    // On Unix, run the child in its own process group so a timeout can signal the
    // whole tree, not just the direct child. An agent CLI that spawned helpers
    // would otherwise leave a grandchild holding the stdout pipe open, blocking the
    // reader forever and forcing the collect grace to return empty — silently
    // dropping the very output the limit/auth detectors scan.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }

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
    let out_handle = thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = BufReader::new(stdout).read_to_end(&mut buf);
        let _ = tx_out.send(buf);
    });
    let err_handle = thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = BufReader::new(stderr).read_to_end(&mut buf);
        let _ = tx_err.send(buf);
    });

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
    let exit = loop {
        if let Some(s) = child.try_wait().context("polling the headless child")? {
            break Some(s);
        }
        if Instant::now() >= deadline {
            kill_tree(&mut child);
            timed_out = true;
            break None;
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
    /// The raw numeric exit code, `None` when killed on the timeout. For adapters
    /// like Kimi that map a specific code (e.g. 75 → `Limit`), the boolean
    /// `exited_cleanly` erases this — keep both.
    pub exit_code: Option<i32>,
}

/// [`run_headless`] plus the post-run shell every headless adapter repeats: combine
/// stdout+stderr into one log, persist it at `log_path`, and recover
/// `exited_cleanly` from the exit status. Returns a [`HeadlessRun`]; the adapter
/// keeps its own `classify_*` and (for Claude) its own `exited = !timed_out`
/// recovery from `timed_out`.
pub fn run_headless_logged(
    cmd: Command,
    prompt: &str,
    timeout: Duration,
    log_path: &Path,
) -> Result<HeadlessRun> {
    let r = run_headless(cmd, prompt, timeout)?;
    let stdout = r.stdout;
    let mut log = stdout.clone();
    log.push_str(&r.stderr);
    let _ = fs::write(log_path, &log);
    let exit = r.exit;
    let exited_cleanly = exit.map(|s| s.success()).unwrap_or(false);
    Ok(HeadlessRun {
        stdout,
        log,
        exited_cleanly,
        timed_out: r.timed_out,
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

/// Kill the child and every descendant it spawned. `child.kill()` signals only the
/// direct child, so a helper process started by an agent CLI would survive and
/// hold the stdout pipe open. Best-effort on every arm; always reaps the child.
fn kill_tree(child: &mut std::process::Child) {
    let pid = child.id();
    #[cfg(windows)]
    {
        // `taskkill /T` terminates the whole tree rooted at PID.
        let _ = Command::new("taskkill")
            .args(["/F", "/T", "/PID", &pid.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    #[cfg(unix)]
    {
        // The child leads its own process group (set at spawn), so a negative pgid
        // signals the whole tree. Dependency-free via the `kill` utility.
        let _ = Command::new("kill")
            .args(["-KILL", &format!("-{pid}")])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    let _ = child.kill(); // direct child / fallback
    let _ = child.wait(); // reap so no zombie lingers
}
