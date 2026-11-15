//! Spawning the `ralphy` child: the [`Spawner`]/[`Child`] seam, the streaming
//! `dispatch` and the collecting `collect` (docs/adr/0036 §2).

use std::ffi::{OsStr, OsString};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, OnceLock};
use std::time::Duration;

use anyhow::{Context, Result};
use tokio::sync::Semaphore;

/// How long a Query or Mutate command may run before the browser gets an
/// answer anyway: the "still running" error. The child is not stopped. A peer
/// runs the same deadline, so its relay waits a little longer than this.
pub const REPLY_DEADLINE: Duration = Duration::from_secs(60);

/// Query and Mutate children that may run at once. Opening one project sends
/// about 6 reads at once (changes, sync, branches, worktrees, board, config), so
/// 8 lets one opening run without a queue. Each child is a `ralphy` that spawns
/// git or gh, and process creation is the slow part on Windows.
pub(crate) const MAX_COLLECT_CHILDREN: usize = 8;

/// The slots of [`MAX_COLLECT_CHILDREN`], shared by every Query and Mutate.
pub(crate) static COLLECT_SLOTS: LazyLock<Arc<Semaphore>> =
    LazyLock::new(|| Arc::new(Semaphore::new(MAX_COLLECT_CHILDREN)));

/// The output of a collected child after the drain saw no new bytes for this
/// long. A grandchild that inherited the pipe can hold it open after the child
/// exits; the child's own answer is already in the pipe by then.
const IDLE_AFTER_EXIT: Duration = Duration::from_millis(200);

/// What [`collect_within`] got before its deadline.
#[derive(Debug)]
pub(crate) enum Collected {
    /// The child exited: its code and its output.
    Done(Option<i32>, CollectedOutput),
    /// The spawn, the wait, or the blocking task failed.
    Failed(anyhow::Error),
    /// The deadline passed first. The child still runs and still holds its slot.
    StillRunning,
    /// The deadline passed before a slot was free: nothing was started.
    NoSlot,
}

/// [`collect`] off the runtime, with a slot from `slots` and an answer within
/// `deadline`. INVARIANT on every path: nothing kills the child, and the slot is
/// released only inside the blocking task, when the child has exited — a
/// deadline does not free it, because the child still runs.
#[expect(
    clippy::too_many_arguments,
    reason = "the spawner, the command, the cwd, the daemon id, the slots and the deadline"
)]
pub(crate) async fn collect_within(
    spawner: Arc<dyn Spawner>,
    program: OsString,
    argv: Vec<String>,
    cwd: PathBuf,
    daemon_id: Option<String>,
    slots: Arc<Semaphore>,
    deadline: Duration,
) -> Collected {
    let started = tokio::time::Instant::now();
    let permit = match tokio::time::timeout(deadline, slots.acquire_owned()).await {
        Ok(Ok(permit)) => permit,
        Ok(Err(e)) => {
            return Collected::Failed(anyhow::Error::new(e).context("waiting for a slot"))
        }
        Err(_) => {
            tracing::warn!(
                held = MAX_COLLECT_CHILDREN,
                "every command slot is held by a command that still runs"
            );
            return Collected::NoSlot;
        }
    };
    let run = async move {
        let joined = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let argv_refs: Vec<&str> = argv.iter().map(String::as_str).collect();
            collect_apart(
                spawner.as_ref(),
                &program,
                &argv_refs,
                &cwd,
                daemon_id.as_deref(),
            )
        })
        .await;
        match joined {
            Ok(Ok((code, output))) => Collected::Done(code, output),
            Ok(Err(e)) => Collected::Failed(e),
            Err(e) => Collected::Failed(anyhow::Error::new(e).context("joining the collect task")),
        }
    };
    tokio::time::timeout(deadline.saturating_sub(started.elapsed()), run)
        .await
        .unwrap_or(Collected::StillRunning)
}

/// What a collected child wrote: stdout and stderr in two buffers. A child of a
/// spawner with one merged pipe has all its output in `stdout`.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct CollectedOutput {
    /// The bytes the child wrote to its standard output.
    pub stdout: Vec<u8>,
    /// The bytes the child wrote to its standard error.
    pub stderr: Vec<u8>,
}

impl CollectedOutput {
    /// Stdout, then stderr on a line of its own: the text an error reply relays.
    pub fn merged(&self) -> Vec<u8> {
        let mut bytes = self.stdout.clone();
        if !bytes.is_empty() && !self.stderr.is_empty() && !bytes.ends_with(b"\n") {
            bytes.push(b'\n');
        }
        bytes.extend_from_slice(&self.stderr);
        bytes
    }
}

/// [`collect_apart`], with the two outputs as one buffer: stdout, then stderr.
pub fn collect(
    spawner: &dyn Spawner,
    program: &OsStr,
    argv: &[&str],
    cwd: &Path,
    daemon_id: Option<&str>,
) -> Result<(Option<i32>, Vec<u8>)> {
    collect_apart(spawner, program, argv, cwd, daemon_id).map(|(code, out)| (code, out.merged()))
}

/// Which output pipe of a collected child a chunk came from.
#[derive(Debug, Clone, Copy)]
enum Pipe {
    Stdout,
    Stderr,
}

/// Spawn a Query/Mutate child and COLLECT its output, returning its exit
/// code and its stdout and stderr bytes verbatim (distinct from the streaming Spawn path:
/// a Query/Mutate answer is a single collected reply, not a live stream). Blocking;
/// callers on the runtime use `collect_within`. Each output pipe is drained on its
/// own thread, so a full pipe never blocks the read of the other; a drain reads
/// to EOF even after this returns (the module OUTPUT STREAMING note): a grandchild
/// that holds a pipe delays only that thread, and this returns once the child
/// exited and the pipes were idle for `IDLE_AFTER_EXIT`.
pub fn collect_apart(
    spawner: &dyn Spawner,
    program: &OsStr,
    argv: &[&str],
    cwd: &Path,
    daemon_id: Option<&str>,
) -> Result<(Option<i32>, CollectedOutput)> {
    let mut child = spawner.spawn(program, argv, cwd, daemon_id)?;
    let (tx, rx) = std::sync::mpsc::channel::<(Pipe, Vec<u8>)>();
    start_drain(child.take_output(), Pipe::Stdout, &tx)?;
    start_drain(child.take_stderr(), Pipe::Stderr, &tx)?;
    drop(tx);
    let code = child.wait()?;
    let mut output = CollectedOutput::default();
    while let Ok((pipe, chunk)) = rx.recv_timeout(IDLE_AFTER_EXIT) {
        match pipe {
            Pipe::Stdout => output.stdout.extend_from_slice(&chunk),
            Pipe::Stderr => output.stderr.extend_from_slice(&chunk),
        }
    }
    Ok((code, output))
}

/// Drain `reader`, when there is one, on its own thread into `tx`.
fn start_drain(
    reader: Option<Box<dyn Read + Send>>,
    pipe: Pipe,
    tx: &std::sync::mpsc::Sender<(Pipe, Vec<u8>)>,
) -> Result<()> {
    let Some(reader) = reader else {
        return Ok(());
    };
    let tx = tx.clone();
    std::thread::Builder::new()
        .name("collect-drain".into())
        .spawn(move || drain_to_eof(reader, pipe, &tx))
        .context("starting the output drain")?;
    Ok(())
}

/// Read `reader` to EOF, sending each chunk while someone receives and
/// discarding it after.
fn drain_to_eof(
    mut reader: Box<dyn Read + Send>,
    pipe: Pipe,
    tx: &std::sync::mpsc::Sender<(Pipe, Vec<u8>)>,
) {
    let mut buf = [0u8; 8192];
    let mut open = true;
    loop {
        match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                if open
                    && tx
                        .send((pipe, buf.get(..n).unwrap_or_default().to_vec()))
                        .is_err()
                {
                    open = false;
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => {
                tracing::warn!(error = %e, "reading a collected child's output failed");
                break;
            }
        }
    }
}

/// A spawned child the dispatcher can await but NEVER kill (see the module
/// teardown invariant). `wait` blocks until the child exits and yields its exit
/// code (`None` when terminated by a signal with no code).
pub trait Child: Send {
    /// The OS process id, when known.
    fn pid(&self) -> Option<u32>;
    /// Block until the child exits; yield its exit code.
    fn wait(&mut self) -> Result<Option<i32>>;
    /// Take the child's merged stdout+stderr reader, ONCE. Yields `Some(reader)`
    /// on the first call and `None` thereafter — the caller owns the reader and
    /// must drain it to EOF (see the module OUTPUT STREAMING note). `wait` no
    /// longer owns the reader, so the drain and the wait can proceed concurrently.
    fn take_output(&mut self) -> Option<Box<dyn Read + Send>>;
    /// Take the child's stderr reader, ONCE, when stderr has a pipe of its own;
    /// `None` when stderr is merged into [`Child::take_output`].
    fn take_stderr(&mut self) -> Option<Box<dyn Read + Send>> {
        None
    }
}

/// Spawns a blessed child. The seam that keeps [`dispatch`] testable: production
/// uses [`ProcessSpawner`]; tests use a fake that records the argv.
pub trait Spawner: Send + Sync + 'static {
    /// Spawn `program` with `args` in `cwd`, detached from the daemon's lifecycle.
    /// When `daemon_id` is `Some`, inject it as `RALPHY_DAEMON_ID` on the child so
    /// its emitter carries the daemon identity (dispatch path only).
    fn spawn(
        &self,
        program: &OsStr,
        args: &[&str],
        cwd: &Path,
        daemon_id: Option<&str>,
    ) -> Result<Box<dyn Child>>;
}

/// Spawn a blessed child: `program` (the resolved `ralphy` exe) with `argv`, in
/// `cwd`. `argv` is composed by [`super::spawn_argv`] from the verb + closed-enum params
/// — never client free-text — and `program` is a real exe run without a shell.
pub fn dispatch(
    spawner: &dyn Spawner,
    program: &OsStr,
    argv: &[&str],
    cwd: &Path,
    daemon_id: Option<&str>,
) -> Result<Box<dyn Child>> {
    spawner.spawn(program, argv, cwd, daemon_id)
}

/// The daemon's own binary, resolved once, when the daemon starts. On Linux
/// `current_exe()` reads `/proc/self/exe`, which names `… (deleted)` after the
/// file is replaced under a running daemon: a later read names a file that is
/// gone, and every child spawned and hook written with it fails.
static OWN_EXE: OnceLock<OsString> = OnceLock::new();

/// Test seam pointing the dispatcher at a stand-in exe: `RALPHY_EXE_OVERRIDE`
/// when set (an integration test's `command_test_child`), else the daemon's own
/// binary — which IS `ralphy` in production. Mirrors `session`'s
/// `RALPHY_DAEMON_AGENT_OVERRIDE`.
pub fn ralphy_exe() -> OsString {
    if let Some(over) = std::env::var_os("RALPHY_EXE_OVERRIDE") {
        return over;
    }
    OWN_EXE
        .get_or_init(|| {
            ralphy_proc_util::exe::running_exe()
                .map_or_else(|_| OsString::from("ralphy"), Into::into)
        })
        .clone()
}

/// The production spawner: a real detached OS process with null stdio.
#[derive(Debug)]
pub struct ProcessSpawner;

impl Spawner for ProcessSpawner {
    fn spawn(
        &self,
        program: &OsStr,
        args: &[&str],
        cwd: &Path,
        daemon_id: Option<&str>,
    ) -> Result<Box<dyn Child>> {
        Self::start(program, args, cwd, daemon_id, Io::merged(None))
    }
}

/// [`ProcessSpawner`] in its collected mode: stdout and stderr in two pipes, so
/// [`collect_apart`] keeps them apart. The streamed path keeps one merged pipe.
#[derive(Debug)]
pub struct TwoPipeSpawner;

impl Spawner for TwoPipeSpawner {
    fn spawn(
        &self,
        program: &OsStr,
        args: &[&str],
        cwd: &Path,
        daemon_id: Option<&str>,
    ) -> Result<Box<dyn Child>> {
        let io = Io {
            input: None,
            pipes: Pipes::Apart,
        };
        ProcessSpawner::start(program, args, cwd, daemon_id, io)
    }
}

/// Whether a child's stderr shares the stdout pipe or has its own.
#[derive(Debug, Clone, Copy)]
enum Pipes {
    Merged,
    Apart,
}

/// The standard streams of a started child: its input, and how its output is piped.
#[derive(Debug, Clone, Copy)]
struct Io<'a> {
    input: Option<&'a [u8]>,
    pipes: Pipes,
}

impl<'a> Io<'a> {
    fn merged(input: Option<&'a [u8]>) -> Self {
        Self {
            input,
            pipes: Pipes::Merged,
        }
    }
}

impl ProcessSpawner {
    /// [`Spawner::spawn`], with `input` written to the child's standard input,
    /// which is then closed. `input` is small (a password), so it fits in the
    /// pipe and the write does not wait for the child.
    pub fn spawn_with_input(
        program: &OsStr,
        args: &[&str],
        cwd: &Path,
        daemon_id: Option<&str>,
        input: &[u8],
    ) -> Result<Box<dyn Child>> {
        Self::start(program, args, cwd, daemon_id, Io::merged(Some(input)))
    }

    fn start(
        program: &OsStr,
        args: &[&str],
        cwd: &Path,
        daemon_id: Option<&str>,
        io: Io<'_>,
    ) -> Result<Box<dyn Child>> {
        use std::io::Write;
        use std::process::{Command, Stdio};
        let Io { input, pipes } = io;
        let mut cmd = Command::new(program);
        // Merge stdout+stderr into ONE pipe so the handler streams a single
        // ordered log (issue #180). The child ignores SIGPIPE, so a daemon crash
        // that drops the reader gives it a non-fatal broken-pipe write, never a
        // kill — but a LIVE daemon must drain the reader to EOF or the pipe fills
        // and stalls the child (the detached drain task of `stream_child` does).
        let (reader, writer) = std::io::pipe()?;
        let (stderr_writer, stderr) = match pipes {
            Pipes::Merged => (writer.try_clone()?, None),
            Pipes::Apart => {
                let (stderr, stderr_writer) = std::io::pipe()?;
                (stderr_writer, Some(stderr))
            }
        };
        cmd.args(args)
            .current_dir(cwd)
            // Null stdin (no console) unless there is input; piped
            // stdout+stderr for live streaming.
            .stdin(if input.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::from(writer))
            .stderr(Stdio::from(stderr_writer));
        // Cross-process wire contract read by ralphy-cli `emitter::DAEMON_ID_ENV`:
        // identity, NOT a credential. Set per-child here (the dispatch path) rather
        // than process-globally, so console/session children — which never get this
        // injection — truthfully lack it (a `ralphy run` typed in a free console).
        if let Some(id) = daemon_id {
            cmd.env("RALPHY_DAEMON_ID", id);
        }
        crate::child_env::remove_secrets(&mut cmd);
        // Detach so daemon shutdown never reaches the run. On Unix its own process
        // group isolates it from the daemon's group.
        ralphy_proc_util::own_process_group(&mut cmd);
        // On Windows `own_process_group` is a no-op, so a same-console child
        // would receive the daemon's console control events. Two flags detach it:
        // CREATE_NEW_PROCESS_GROUP (0x200) stops CTRL_C/CTRL_BREAK reaching it,
        // and DETACHED_PROCESS (0x8) gives it no console at all — otherwise a
        // CTRL_CLOSE/LOGOFF/SHUTDOWN event, delivered to every process on the
        // daemon's console regardless of group, would still kill the run. The
        // child is null-stdio, so it needs no console. Together they honor the
        // "spawned runs keep their own lifecycle" invariant on Windows.
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0000_0208);
        }
        let mut child = cmd.spawn()?;
        if let Some(input) = input {
            let mut stdin = child
                .stdin
                .take()
                .ok_or_else(|| anyhow::anyhow!("the child has no stdin pipe"))?;
            stdin.write_all(input)?;
        }
        Ok(Box::new(ProcessChild {
            child,
            output: Some(reader),
            stderr,
        }))
    }
}

/// A real OS child. `wait`-only: no kill method exists, and dropping it does not
/// kill (std semantics) — the dispatched run outlives the daemon. `output` holds
/// the merged stdout+stderr reader until [`Child::take_output`] hands it off.
struct ProcessChild {
    child: std::process::Child,
    output: Option<std::io::PipeReader>,
    stderr: Option<std::io::PipeReader>,
}

impl Child for ProcessChild {
    fn pid(&self) -> Option<u32> {
        Some(self.child.id())
    }

    fn wait(&mut self) -> Result<Option<i32>> {
        Ok(self.child.wait()?.code())
    }

    fn take_output(&mut self) -> Option<Box<dyn Read + Send>> {
        self.output
            .take()
            .map(|r| -> Box<dyn Read + Send> { Box::new(r) })
    }

    fn take_stderr(&mut self) -> Option<Box<dyn Read + Send>> {
        self.stderr
            .take()
            .map(|r| -> Box<dyn Read + Send> { Box::new(r) })
    }
}

#[cfg(test)]
mod tests;
