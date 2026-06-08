//! `ralphy-pty` — shared PTY crate backed by [`portable-pty`].
//!
//! It opens a child process inside a pseudo-terminal (ConPTY on Windows),
//! streams the TTY-rendered output, and feeds it input. This is the
//! load-bearing capability that replaces the ps1's "new console window" trick,
//! and the future home of the on-screen / Tauri terminal and supervised
//! sessions.
//!
//! It is a *shared* crate — consumed by adapters that drive an interactive CLI,
//! never by `ralphy-core`, which stays PTY-free by design (docs/adr/0002). The
//! public surface deliberately speaks only `std` traits ([`Read`]/[`Write`]) and
//! plain integers, so consumers never name `portable-pty` directly.

use std::ffi::OsString;
use std::io::{Read, Write};
use std::path::Path;

use anyhow::{Context, Result};
use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};

/// The bytes a console program emits to ask the terminal where the cursor is
/// (ANSI Device Status Report, `ESC [ 6 n`).
///
/// This matters on Windows: a freshly-spawned console program issues this query
/// during start-up and **blocks until the terminal answers**. A PTY consumer is
/// that terminal — if it never replies, the child hangs before running. Reply
/// with [`CURSOR_POSITION_REPLY`] when this sequence appears in the output.
pub const CURSOR_POSITION_REQUEST: &[u8] = b"\x1b[6n";

/// A canonical answer to [`CURSOR_POSITION_REQUEST`]: "cursor at row 1, col 1"
/// (`ESC [ 1 ; 1 R`). Write it back to the PTY so the child can proceed.
pub const CURSOR_POSITION_REPLY: &[u8] = b"\x1b[1;1R";

/// How a child is spawned inside a PTY: the program, its arguments, working
/// directory, environment additions, and the terminal's initial size.
///
/// Built fluently and consumed by [`PtySession::spawn`].
pub struct PtyCommand {
    program: OsString,
    args: Vec<OsString>,
    cwd: Option<OsString>,
    env: Vec<(OsString, OsString)>,
    rows: u16,
    cols: u16,
}

impl PtyCommand {
    /// Start a command for `program`, with a default 24×80 terminal.
    pub fn new(program: impl Into<OsString>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
            cwd: None,
            env: Vec::new(),
            rows: 24,
            cols: 80,
        }
    }

    /// Append one argument.
    pub fn arg(mut self, arg: impl Into<OsString>) -> Self {
        self.args.push(arg.into());
        self
    }

    /// Append several arguments.
    pub fn args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<OsString>,
    {
        self.args.extend(args.into_iter().map(Into::into));
        self
    }

    /// Run the child in `dir`.
    pub fn cwd(mut self, dir: impl AsRef<Path>) -> Self {
        self.cwd = Some(dir.as_ref().as_os_str().to_owned());
        self
    }

    /// Set an environment variable for the child.
    pub fn env(mut self, key: impl Into<OsString>, val: impl Into<OsString>) -> Self {
        self.env.push((key.into(), val.into()));
        self
    }

    /// Set the PTY's initial size in character cells.
    pub fn size(mut self, rows: u16, cols: u16) -> Self {
        self.rows = rows;
        self.cols = cols;
        self
    }
}

/// A child process running inside a live pseudo-terminal.
///
/// Hold the session to keep the PTY open: read output with [`reader`], send
/// input with [`write_all`], [`resize`] the window, and [`kill`]/[`wait`] the
/// process tree. Dropping the session closes the master and the writer.
///
/// The session is a raw PTY, not a terminal emulator: the consumer plays the
/// terminal. In particular it must answer queries the child makes — most
/// importantly the cursor-position request at start-up (see
/// [`CURSOR_POSITION_REQUEST`]), or the child blocks before it runs.
///
/// [`reader`]: PtySession::reader
/// [`write_all`]: PtySession::write_all
/// [`resize`]: PtySession::resize
/// [`kill`]: PtySession::kill
/// [`wait`]: PtySession::wait
pub struct PtySession {
    master: Box<dyn MasterPty + Send>,
    child: Box<dyn portable_pty::Child + Send + Sync>,
    writer: Box<dyn Write + Send>,
}

impl PtySession {
    /// Open a PTY and spawn `cmd` inside it. The slave side is closed once the
    /// child holds it, so the master sees EOF when the child's tree exits.
    pub fn spawn(cmd: PtyCommand) -> Result<Self> {
        let size = PtySize {
            rows: cmd.rows,
            cols: cmd.cols,
            pixel_width: 0,
            pixel_height: 0,
        };
        let pair = native_pty_system()
            .openpty(size)
            .context("opening a pseudo-terminal")?;

        let mut builder = CommandBuilder::new(&cmd.program);
        builder.args(&cmd.args);
        if let Some(dir) = &cmd.cwd {
            builder.cwd(dir);
        }
        for (k, v) in &cmd.env {
            builder.env(k, v);
        }

        let child = pair
            .slave
            .spawn_command(builder)
            .with_context(|| format!("spawning {:?} in the PTY", cmd.program))?;
        // Drop the slave handle: with the child as the only holder, the master
        // reader gets a clean EOF when the process tree finishes.
        drop(pair.slave);

        let writer = pair
            .master
            .take_writer()
            .context("taking the PTY input writer")?;

        Ok(Self {
            master: pair.master,
            child,
            writer,
        })
    }

    /// A fresh reader over the master output. Reading blocks until bytes arrive
    /// and returns 0 (EOF) once the child tree exits; drain it on its own
    /// thread to capture TTY-rendered output without deadlocking on input.
    pub fn reader(&self) -> Result<Box<dyn Read + Send>> {
        self.master
            .try_clone_reader()
            .context("cloning the PTY output reader")
    }

    /// Send raw bytes to the child as terminal input (include `\r` to submit a
    /// line, as a real terminal would).
    pub fn write_all(&mut self, bytes: &[u8]) -> Result<()> {
        self.writer.write_all(bytes).context("writing to the PTY")?;
        self.writer.flush().context("flushing PTY input")
    }

    /// Resize the terminal window, in character cells.
    pub fn resize(&self, rows: u16, cols: u16) -> Result<()> {
        self.master
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .context("resizing the PTY")
    }

    /// Terminate the child process tree.
    pub fn kill(&mut self) -> Result<()> {
        self.child.kill().context("killing the PTY child")
    }

    /// Block until the child exits and report how it ended.
    pub fn wait(&mut self) -> Result<PtyExit> {
        let status = self.child.wait().context("waiting on the PTY child")?;
        Ok(PtyExit {
            success: status.success(),
            code: status.exit_code(),
        })
    }

    /// Report the exit if the child has already finished, without blocking.
    pub fn try_wait(&mut self) -> Result<Option<PtyExit>> {
        let status = self
            .child
            .try_wait()
            .context("polling the PTY child")?;
        Ok(status.map(|s| PtyExit {
            success: s.success(),
            code: s.exit_code(),
        }))
    }
}

/// How a PTY child finished.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PtyExit {
    /// Whether the process reported success (exit code 0 on most platforms).
    pub success: bool,
    /// The raw exit code.
    pub code: u32,
}
