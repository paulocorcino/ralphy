//! The runner-enforced verify gate (ADR-0011).
//!
//! Between the executor returning [`crate::Outcome::Done`] and the runner closing
//! the issue, the runner re-runs a set of commands the plan declared, over the
//! committed state, and only closes if they pass. Green stops meaning "the agent
//! said so" and starts meaning "the runner *saw* the verification pass".
//!
//! Two halves live here, both vendor- and ecosystem-neutral:
//!   - [`parse_verify`] reads the `## Verify` plan section into a [`VerifySpec`]
//!     — the same `section_after_heading` molecule the acceptance ledger uses.
//!   - [`run`] executes a list of argv commands directly (no shell), sequentially,
//!     in `repo_root`, stopping on the first non-zero exit, within a bounded
//!     timeout (a timeout counts as a failure).

use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

mod comment;
mod parse;

pub use comment::{comment, invalid_comment, repair_brief, spawn_failure_comment};
pub use parse::{parse_verify, tokenize, VerifySpec};

/// One command's outcome inside a gate run: the argv it ran, its exit code (when
/// the process exited normally), whether it timed out, and a tail of its combined
/// stdout+stderr (kept short for the issue comment).
#[derive(Debug, Clone)]
pub struct CommandOutcome {
    pub argv: Vec<String>,
    /// The process exit code, or `None` when it timed out, was killed by a
    /// signal, or never spawned (no numeric code).
    pub exit_code: Option<i32>,
    /// The command exceeded the gate's remaining time budget and was killed.
    pub timed_out: bool,
    /// The command never ran: the program could not be spawned (not found on
    /// PATH, a typo'd binary, an empty argv). Distinct from a signal kill or a
    /// non-zero exit — re-running the SAME argv can never make it pass, so the
    /// gate treats it as a non-repairable spec/spawn problem (#182).
    pub spawn_failed: bool,
    /// Last few lines of combined stdout+stderr — empty on success when there was
    /// no output. Captured on every command so the artifact comment can show it.
    pub output_tail: String,
    /// Measured wall-clock seconds — feeds the durable command-cost knowledge
    /// (`cmdcost`) the verification-cost gate reads.
    pub secs: f64,
}

impl CommandOutcome {
    /// A command passed when it exited with code 0 and did not time out.
    pub fn passed(&self) -> bool {
        !self.timed_out && self.exit_code == Some(0)
    }
}

/// The result of a gate run: each command attempted (in order, stopping at the
/// first failure) and whether the whole gate passed.
#[derive(Debug, Clone)]
pub struct VerifyReport {
    pub commands: Vec<CommandOutcome>,
    pub passed: bool,
}

impl VerifyReport {
    /// The first command that did not pass, or `None` when the gate passed. The
    /// gate stops at the first failure, so this is the command that decided the
    /// gate — and the only one whose failure *kind* matters.
    pub fn first_failure(&self) -> Option<&CommandOutcome> {
        self.commands.iter().find(|c| !c.passed())
    }

    /// Whether the gate's deciding failure is a spawn failure: the command never
    /// ran (program not found / empty argv), so re-running the SAME argv can never
    /// make it pass. The runner short-circuits such a gate — skipping the issue
    /// without spending the repair budget on a structural failure it can already
    /// see is non-repairable (#182).
    pub fn spawn_failed(&self) -> bool {
        self.first_failure().is_some_and(|c| c.spawn_failed)
    }
}

/// Run `commands` as direct argv in `repo_root`, sequentially, stopping at the
/// first that does not exit 0. The whole sequence shares a single `timeout`
/// budget; a command that would run past it is killed and counts as a failure
/// (a hung verification cannot become green by silence). Each command's exit code
/// and an output tail are captured for the honesty artifact.
pub fn run(commands: &[Vec<String>], repo_root: &Path, timeout: Duration) -> VerifyReport {
    let deadline = Instant::now() + timeout;
    let mut outcomes = Vec::new();
    let mut passed = true;

    for argv in commands {
        let outcome = run_one(argv, repo_root, deadline);
        let ok = outcome.passed();
        outcomes.push(outcome);
        if !ok {
            passed = false;
            break;
        }
    }

    VerifyReport {
        commands: outcomes,
        passed,
    }
}

/// How many trailing characters of combined output to keep for the comment tail.
const TAIL_BYTES: usize = 4000;

/// Grace for collecting a command's output after the tree-kill. The kill closes
/// every inherited write-end, so the readers normally hit EOF and deliver at
/// once; the grace is a backstop for a kill that failed to close a handle —
/// then that stream's capture is dropped and the stuck reader leaked instead of
/// blocking the gate forever (#156). Mirrors the headless runner's 5s collect
/// grace.
const OUTPUT_COLLECT_GRACE: Duration = Duration::from_secs(5);

/// Run a single command, draining its output through threads (so a chatty command
/// never deadlocks on a full pipe) and killing it if the shared `deadline` passes.
fn run_one(argv: &[String], repo_root: &Path, deadline: Instant) -> CommandOutcome {
    let started = Instant::now();
    // An empty argv cannot be run; treat it as a spawn failure so the gate stops.
    let Some((program, rest)) = argv.split_first() else {
        return CommandOutcome {
            argv: argv.to_vec(),
            exit_code: None,
            timed_out: false,
            spawn_failed: true,
            output_tail: "empty command".into(),
            secs: 0.0,
        };
    };

    // Resolve the program so the gate's no-shell argv spawn (ADR-0011) still finds
    // Windows shell shims: `pnpm`/`npm`/`yarn`/`npx` are `.cmd` scripts a bare
    // `CreateProcess` never locates (it only appends `.exe`) and cannot execute
    // even if named. On Unix this is a pass-through. See [`spawn_command`].
    let (spawn_program, spawn_args) = spawn_command(program, rest);
    let mut cmd = Command::new(&spawn_program);
    cmd.args(&spawn_args)
        .current_dir(repo_root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // Run the child in its own process group (Unix) so a timeout — or the teardown
    // below — can signal the whole tree, not just the direct child. Windows walks
    // the tree by PID at kill time and needs nothing here. See [`kill_tree`] (#156).
    ralphy_proc_util::own_process_group(&mut cmd);
    // Hidden console on Windows: the verify child's stdio is piped and it may run
    // under the console-less daemon child, where it would otherwise flash a window.
    ralphy_proc_util::no_window(&mut cmd);
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            return CommandOutcome {
                argv: argv.to_vec(),
                exit_code: None,
                timed_out: false,
                spawn_failed: true,
                output_tail: format!("failed to spawn `{program}`: {e}"),
                secs: 0.0,
            };
        }
    };

    // Drain stdout+stderr concurrently so neither pipe can fill and wedge the
    // child while we poll for the deadline. Each reader sends its buffer over a
    // channel so the collect below can bound how long it waits (a descendant that
    // inherited the pipe can hold it open past the foreground exit; the join must
    // not block on that — #156).
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let (tx_out, rx_out) = mpsc::channel::<Vec<u8>>();
    let (tx_err, rx_err) = mpsc::channel::<Vec<u8>>();
    let out_handle = stdout.map(|mut s| {
        thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = s.read_to_end(&mut buf);
            let _ = tx_out.send(buf);
        })
    });
    let err_handle = stderr.map(|mut s| {
        thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = s.read_to_end(&mut buf);
            let _ = tx_err.send(buf);
        })
    });

    let mut timed_out = false;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {
                // The operator's stop (docs/adr/0054). The gate is the run's OTHER
                // long-lived child — a full test suite here is routinely minutes —
                // so without this check a stop would be swallowed until
                // `verify_timeout` elapsed, which is the one wait an operator
                // watching a Stop button will not forgive.
                //
                // Reported as `timed_out` because [`CommandOutcome`] has no third
                // state and a stopped gate genuinely did not pass. Nothing
                // downstream misreads it: the run is unwinding on a `StopReason`
                // by the time this outcome is folded, so the gate's verdict never
                // becomes the reason the run halted.
                if crate::stop::requested() || Instant::now() >= deadline {
                    timed_out = true;
                    break None;
                }
                thread::sleep(Duration::from_millis(50));
            }
            Err(_) => break None,
        }
    };

    // Teardown BEFORE collection, on every path: kill the whole tree — not just
    // the direct `sh`/`bash` — so a descendant that inherited the pipes (a dev
    // server a `## Verify` command backgrounded) can't hold the write-end open
    // and starve the readers of EOF. Collecting first with only a bounded grace
    // dropped the whole stream whenever such a descendant existed, which on a
    // FAILED gate handed the repair executor an empty output tail. The kill also
    // reaps leaked descendants (a dev server holding a port) so a self-leaking
    // command can't poison later gates. The direct child's exit code, already in
    // hand, stays the outcome.
    ralphy_proc_util::kill_tree(&mut child);

    // Collect the captured output. The tree-kill closed every write-end, so the
    // readers hit EOF and deliver the full capture promptly; the grace is a
    // backstop against a kill that failed to close a handle — the unbounded join
    // it replaces is what hung the gate for ~43 min (#156).
    let mut combined = String::new();
    if let Some(h) = out_handle {
        combined.push_str(&recv_and_join(&rx_out, h, "stdout"));
    }
    if let Some(h) = err_handle {
        combined.push_str(&recv_and_join(&rx_err, h, "stderr"));
    }

    CommandOutcome {
        argv: argv.to_vec(),
        exit_code: status.and_then(|s| s.code()),
        timed_out,
        // The program spawned — whatever happened next (a non-zero exit, a
        // timeout kill, a signal) is a real run, not a spawn failure.
        spawn_failed: false,
        output_tail: tail(&combined),
        secs: started.elapsed().as_secs_f64(),
    }
}

/// Bridge the gate's deliberate no-shell argv execution (ADR-0011) to Windows'
/// shell-only program shims. On Windows, `pnpm`/`npm`/`yarn`/`npx` are `.cmd`
/// scripts that a bare `CreateProcess` never finds (it only appends `.exe`) and
/// could not execute even if named (a `.cmd` is not an executable image). We
/// resolve the name through `PATHEXT` and, for a `.cmd`/`.bat`, route it through
/// `cmd /C` — the args still pass as separate argv entries, so no user `&&`/pipe is
/// reintroduced; only the one resolved script runs. On Unix the program and args
/// pass through unchanged.
#[cfg(windows)]
fn spawn_command(program: &str, rest: &[String]) -> (std::ffi::OsString, Vec<std::ffi::OsString>) {
    // A path-qualified name is used as-is (it resolves against the spawn's
    // current_dir); only a bare name is searched on PATH/PATHEXT. The
    // `.COM;.EXE;.BAT;.CMD` fallback preserves prior behavior when PATHEXT is
    // unset (the shared primitive defaults to `.EXE` only).
    let resolved = if program.contains('/') || program.contains('\\') {
        Some(std::path::PathBuf::from(program))
    } else {
        ralphy_proc_util::find_program(
            program,
            std::env::var_os("PATH"),
            std::env::var_os("PATHEXT")
                .or_else(|| Some(std::ffi::OsString::from(".COM;.EXE;.BAT;.CMD"))),
        )
    };
    spawn_argv(resolved, program, rest)
}

/// Decide the argv for a resolved program: a `.cmd`/`.bat` script routes through
/// `cmd /C` (a batch file is not an executable image), a resolved `.exe` runs
/// directly, and an unresolved name passes through so the spawn surfaces the same
/// "program not found" failure as before. Pure over its inputs so it unit-tests
/// without touching PATH.
#[cfg(windows)]
fn spawn_argv(
    resolved: Option<std::path::PathBuf>,
    program: &str,
    rest: &[String],
) -> (std::ffi::OsString, Vec<std::ffi::OsString>) {
    use std::ffi::OsString;
    match resolved {
        Some(path) => {
            let is_batch = path
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| e.eq_ignore_ascii_case("cmd") || e.eq_ignore_ascii_case("bat"))
                .unwrap_or(false);
            if is_batch {
                let mut args: Vec<OsString> = vec![OsString::from("/C"), path.into_os_string()];
                args.extend(rest.iter().map(OsString::from));
                (OsString::from("cmd"), args)
            } else {
                (
                    path.into_os_string(),
                    rest.iter().map(OsString::from).collect(),
                )
            }
        }
        None => (
            OsString::from(program),
            rest.iter().map(OsString::from).collect(),
        ),
    }
}

#[cfg(not(windows))]
fn spawn_command(program: &str, rest: &[String]) -> (std::ffi::OsString, Vec<std::ffi::OsString>) {
    use std::ffi::OsString;
    (
        OsString::from(program),
        rest.iter().map(OsString::from).collect(),
    )
}

/// Await one reader thread's captured bytes within [`OUTPUT_COLLECT_GRACE`], then
/// join it and return the bytes as lossy UTF-8. The caller tree-killed the child
/// first, so the pipe hits EOF and the thread sends promptly — the join is
/// immediate. The reader sends its buffer exactly once, at EOF, so if the grace
/// elapses (a write-end the kill could not close) there is nothing partial to
/// recover: this stream's capture is DROPPED, and the stuck thread leaked instead
/// of blocking the gate on a join that would hang (#156). Mirrors
/// `ralphy-adapter-support`'s `recv_and_join`.
fn recv_and_join(
    rx: &mpsc::Receiver<Vec<u8>>,
    handle: thread::JoinHandle<()>,
    stream: &str,
) -> String {
    match rx.recv_timeout(OUTPUT_COLLECT_GRACE) {
        Ok(buf) => {
            let _ = handle.join();
            String::from_utf8_lossy(&buf).into_owned()
        }
        Err(_) => {
            tracing::warn!(
                stream,
                "verify gate reader still blocked after the tree-kill — this stream's captured output was dropped"
            );
            String::new()
        }
    }
}

/// Keep the last [`TAIL_BYTES`] of `s`, trimmed to a whole-line boundary so the
/// tail never starts mid-line. Empty stays empty.
fn tail(s: &str) -> String {
    let trimmed = s.trim_end();
    if trimmed.len() <= TAIL_BYTES {
        return trimmed.to_string();
    }
    let mut start = trimmed.len() - TAIL_BYTES;
    // The byte offset may land inside a multi-byte char (e.g. box-drawing '└' in
    // vitest output, #…); nudge it forward to a char boundary before slicing.
    while !trimmed.is_char_boundary(start) {
        start += 1;
    }
    // Advance to the next newline so we drop the partial leading line.
    let slice = &trimmed[start..];
    match slice.find('\n') {
        Some(nl) => slice[nl + 1..].to_string(),
        None => slice.to_string(),
    }
}

#[cfg(test)]
mod tests;
