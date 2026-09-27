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

use std::ffi::{OsStr, OsString};
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
    env_remove: Vec<OsString>,
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
            env_remove: Vec::new(),
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

    /// Unset an environment variable the child would otherwise INHERIT from this
    /// process. The child's base environment is a copy of ours, so a variable a
    /// caller must not pass on — one that speaks for the shell that launched us
    /// rather than for the terminal we are giving the child — can only be
    /// removed here. Applied before [`env`], so setting the same key afterwards
    /// wins.
    ///
    /// [`env`]: PtyCommand::env
    pub fn env_remove(mut self, key: impl Into<OsString>) -> Self {
        self.env_remove.push(key.into());
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
    ///
    /// On Windows, a batch program (`.cmd`, `.bat`, or a bare name that may
    /// resolve to one) is refused, before any PTY is opened, when its command
    /// line would not reach it as text: an argument holds a character that
    /// cmd.exe reads as a command character, or the quotes around the program
    /// path would be removed.
    pub fn spawn(cmd: PtyCommand) -> Result<Self> {
        if cfg!(windows) && may_run_through_cmd(&cmd.program) {
            if let Some(reason) = batch_refusal(&cmd.program, &cmd.args) {
                anyhow::bail!("refusing to start {:?}: {reason}", cmd.program);
            }
        }
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
        for k in &cmd.env_remove {
            builder.env_remove(k);
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

    /// The child's OS process id, if it is still known. `kill` signals only this
    /// direct child; a consumer that must reach a whole tree (a child that spawned
    /// its own helpers) uses this pid with a platform tree-kill.
    pub fn process_id(&self) -> Option<u32> {
        self.child.process_id()
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
        let status = self.child.try_wait().context("polling the PTY child")?;
        Ok(status.map(|s| PtyExit {
            success: s.success(),
            code: s.exit_code(),
        }))
    }
}

/// Whether Windows may start `program` through cmd.exe: true unless its
/// extension is `exe` or `com`. A bare name counts, because `portable-pty`
/// searches `PATHEXT` and can turn `gemini` into `gemini.cmd`.
fn may_run_through_cmd(program: &OsStr) -> bool {
    let program = program.to_string_lossy();
    let file = program.rsplit(['/', '\\']).next().unwrap_or(&program);
    match file.rsplit_once('.') {
        Some((_, ext)) => !ext.eq_ignore_ascii_case("exe") && !ext.eq_ignore_ascii_case("com"),
        None => true,
    }
}

// `portable-pty` 0.9 quotes an argument only when it is empty or holds a space,
// tab, `\n`, `\x0b` or `"`, and it escapes `"` as `\"`. cmd.exe re-reads the
// whole line of a batch program (and its `%*`) with its own rules: `%` (and `!`
// with delayed expansion) expands even inside quotes, `\"` is not an escape so
// it breaks quote pairing, and `& | < > ^ ( )` are text only inside quotes.
// Windows starts a batch program as `cmd.exe /c <line>`, and `/c` keeps the
// quotes of a quoted program path only when they are the line's only two quotes
// and hold none of `& < > ( ) @ ^ |`; otherwise it removes the first and last
// quote of the line, and a quoted `&` becomes a command (measured on Windows 11
// 26200 with `tests/pty.rs`'s `.cmd` shim).
/// Why cmd.exe would not read `program` and `args` as text, once
/// `portable-pty` has written them into a command line.
fn batch_refusal(program: &OsStr, args: &[OsString]) -> Option<String> {
    let tokens = std::iter::once(program).chain(args.iter().map(OsString::as_os_str));
    if let Some((i, ch)) = tokens
        .enumerate()
        .find_map(|(i, arg)| cmd_hazard(arg).map(|ch| (i, ch)))
    {
        let token = match i {
            0 => "the program path".to_owned(),
            i => format!("argument {i}"),
        };
        return Some(format!(
            "{token} contains {ch:?}, which cmd.exe reads as a command character, not as text"
        ));
    }
    if !is_quoted(program) {
        return None;
    }
    if let Some(ch) = program
        .to_string_lossy()
        .chars()
        .find(|c| matches!(c, '&' | '<' | '>' | '(' | ')' | '@' | '^' | '|'))
    {
        return Some(format!(
            "the program path holds a space and {ch:?}, so cmd.exe removes its quotes"
        ));
    }
    let i = args.iter().position(|arg| is_quoted(arg))?;
    Some(format!(
        "the program path holds a space and argument {} is quoted, so cmd.exe removes \
         the outer quotes of the line",
        i + 1
    ))
}

/// Whether `portable-pty` writes `arg` between quotes.
fn is_quoted(arg: &OsStr) -> bool {
    let arg = arg.to_string_lossy();
    arg.is_empty()
        || arg
            .chars()
            .any(|c| matches!(c, ' ' | '\t' | '\n' | '\x0b' | '"'))
}

/// The first character of `arg` that cmd.exe would read as a command character
/// once `portable-pty` has written `arg` into a command line.
fn cmd_hazard(arg: &OsStr) -> Option<char> {
    if let Some(ch) = arg
        .to_string_lossy()
        .chars()
        .find(|c| matches!(c, '%' | '!' | '"' | '\r' | '\n'))
    {
        return Some(ch);
    }
    if is_quoted(arg) {
        return None;
    }
    arg.to_string_lossy()
        .chars()
        .find(|c| matches!(c, '&' | '|' | '<' | '>' | '^' | '(' | ')'))
}

/// How a PTY child finished.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PtyExit {
    /// Whether the process reported success (exit code 0 on most platforms).
    pub success: bool,
    /// The raw exit code.
    pub code: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hazard(arg: &str) -> Option<char> {
        cmd_hazard(OsStr::new(arg))
    }

    fn through_cmd(program: &str) -> bool {
        may_run_through_cmd(OsStr::new(program))
    }

    #[test]
    fn cmd_hazard_refuses_bare_ampersand() {
        assert_eq!(hazard("R&D"), Some('&'));
    }

    #[test]
    fn cmd_hazard_accepts_a_quoted_ampersand() {
        assert_eq!(hazard("R &D"), None);
    }

    #[test]
    fn cmd_hazard_refuses_percent_even_quoted() {
        assert_eq!(hazard("a b%PATH%"), Some('%'));
    }

    #[test]
    fn cmd_hazard_refuses_exclamation_even_quoted() {
        assert_eq!(hazard("a b!x!"), Some('!'));
    }

    #[test]
    fn cmd_hazard_refuses_double_quote() {
        assert_eq!(hazard("a\"b"), Some('"'));
    }

    #[test]
    fn cmd_hazard_refuses_a_line_break() {
        assert_eq!(hazard("a\r\nb"), Some('\r'));
    }

    #[test]
    fn cmd_hazard_refuses_bare_redirects_and_parens() {
        for (arg, ch) in [
            ("a|b", '|'),
            ("a<b", '<'),
            ("a>b", '>'),
            ("a^b", '^'),
            ("f(x)", '('),
        ] {
            assert_eq!(hazard(arg), Some(ch), "{arg}");
        }
    }

    #[test]
    fn cmd_hazard_accepts_quoted_parens() {
        assert_eq!(hazard(r"C:\Program Files (x86)\x.json"), None);
    }

    #[test]
    fn cmd_hazard_accepts_a_plain_path() {
        assert_eq!(
            hazard(r"C:\Dev\repo\.ralphy\runs\1\ralphy.settings.json"),
            None
        );
    }

    #[test]
    fn cmd_hazard_accepts_the_exec_charter_shape() {
        assert_eq!(
            hazard("Read .ralphy/exec.md and follow it. Emit RALPHY_DONE_EXIT when finished."),
            None
        );
    }

    fn refusal(program: &str, args: &[&str]) -> Option<String> {
        let args: Vec<OsString> = args.iter().map(OsString::from).collect();
        batch_refusal(OsStr::new(program), &args)
    }

    #[test]
    fn batch_refusal_names_the_argument_index() {
        let reason = refusal(r"C:\npm\x.cmd", &["ok", "R&D"]).expect("refused");
        assert!(reason.starts_with("argument 2 contains '&'"), "{reason}");
    }

    #[test]
    fn batch_refusal_names_the_program_path() {
        let reason = refusal(r"C:\R&D\x.cmd", &[]).expect("refused");
        assert!(
            reason.starts_with("the program path contains '&'"),
            "{reason}"
        );
    }

    #[test]
    fn batch_refusal_accepts_a_quoted_program_with_bare_arguments() {
        assert_eq!(refusal(r"C:\a b\x.cmd", &["R", "--name", "wb-x-1"]), None);
    }

    #[test]
    fn batch_refusal_refuses_a_quoted_program_with_a_quoted_argument() {
        let reason = refusal(r"C:\a b\x.cmd", &["R", "a b"]).expect("refused");
        assert!(reason.contains("argument 2 is quoted"), "{reason}");
    }

    #[test]
    fn batch_refusal_refuses_a_quoted_program_with_a_special_character() {
        let reason = refusal(r"C:\Program Files (x86)\x.cmd", &[]).expect("refused");
        assert!(reason.contains("'('"), "{reason}");
    }

    #[test]
    fn batch_refusal_accepts_quoted_arguments_after_a_bare_program() {
        assert_eq!(refusal(r"C:\npm\x.cmd", &["a b&c", r"C:\x y\"]), None);
    }

    #[test]
    fn may_run_through_cmd_for_batch_and_bare_names() {
        assert!(through_cmd("gemini.CMD"));
        assert!(through_cmd("x.bat"));
        assert!(through_cmd("gemini"));
        assert!(through_cmd(r"C:\tools.v2\gemini"));
    }

    #[test]
    fn may_run_through_cmd_not_for_native_programs() {
        assert!(!through_cmd(r"C:\x\cmd.exe"));
        assert!(!through_cmd("PWSH.EXE"));
        assert!(!through_cmd("tool.com"));
    }
}
