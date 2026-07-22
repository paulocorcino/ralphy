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

use regex::Regex;

/// The parsed `## Verify` plan section (ADR-0011).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerifySpec {
    /// `none` on its own line — the planner judged nothing is machine-verifiable.
    /// The only explicit opt-out; it skips the settings fallback.
    None,
    /// One or more commands, each tokenized into an argv.
    Commands(Vec<Vec<String>>),
    /// The section is present but malformed — a markdown checklist masquerading as
    /// commands (a leading `-`/`*`/`+` bullet, `- [ ]` checkbox, or backtick-wrapped
    /// command), or a backslash-escaped quote the tokenizer cannot honor (`\"`/`\'`).
    /// Tokenizing the former would spawn a bogus `-`/`` ` `` program (#181); the latter
    /// mis-splits into a garbage argv the gate spawn-fails on (#268). Either way the
    /// real command would never run, so it resolves to this arm instead. Carries a
    /// clear, operator-facing error naming the malformed line(s).
    Invalid(String),
    /// Section absent or present-but-empty — a planner omission. The runner falls
    /// back to `settings.json` `verify.command`.
    Unspecified,
}

/// Parse the `## Verify` section of a plan markdown into a [`VerifySpec`].
///
/// One command per line, code-fence-tolerant (` ``` ` lines are ignored), with
/// quote-aware argv tokenization so `sh -c "cargo test"` survives as three
/// tokens. A lone `none` line (case-insensitive) is the explicit opt-out. An
/// absent or whitespace-only section is [`VerifySpec::Unspecified`]. A section
/// authored as a markdown checklist (a leading bullet, checkbox, or
/// backtick-wrapped command) is rejected as [`VerifySpec::Invalid`] rather than
/// tokenized into a bogus `-`/`` ` `` program that spawn-fails (#181), as is a line
/// with a backslash-escaped quote (`\"`/`\'`) the tokenizer cannot honor (#268).
pub fn parse_verify(md: &str) -> VerifySpec {
    let heading_re = Regex::new(r"(?im)^##\s+Verify\s*$").expect("valid regex");
    let section = crate::markdown::section_after_heading(md, &heading_re);

    let lines: Vec<&str> = section
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with("```"))
        .collect();

    if lines.is_empty() {
        return VerifySpec::Unspecified;
    }
    // `none` is only an opt-out when it stands alone — a `none` among real
    // commands is tokenized like any other line (and would simply fail to run).
    if lines.len() == 1 && lines[0].eq_ignore_ascii_case("none") {
        return VerifySpec::None;
    }

    // Reject a markdown checklist before tokenizing: a leading `-`/`*`/`+` bullet,
    // `- [ ]` checkbox, or backtick-wrapped command would tokenize its marker into a
    // bogus `-`/`` ` `` `argv[0]` the gate spawn-fails on, so the real command never
    // runs (#181). Enforces ADR-0011's "one bare command per line" contract.
    let malformed: Vec<&str> = lines
        .iter()
        .copied()
        .filter(|l| looks_like_markdown_list(l))
        .collect();
    if !malformed.is_empty() {
        return VerifySpec::Invalid(malformed_error(&malformed));
    }

    // Reject a backslash-escaped quote before tokenizing: [`tokenize`] does not honor
    // `\"`/`\'`, so a nested escaped quote (`sh -c "test \"$x\" = y"`) is mis-split
    // into a garbage argv the gate spawn-runs, failing with a confusing shell syntax
    // error that also spends the repair budget (#268). Catch it here with a clear,
    // actionable message rather than let it fail opaquely at runtime.
    let escaped: Vec<&str> = lines
        .iter()
        .copied()
        .filter(|l| has_escaped_quote(l))
        .collect();
    if !escaped.is_empty() {
        return VerifySpec::Invalid(escaped_quote_error(&escaped));
    }

    let commands: Vec<Vec<String>> = lines
        .iter()
        .map(|l| tokenize(l))
        .filter(|argv| !argv.is_empty())
        .collect();

    if commands.is_empty() {
        VerifySpec::Unspecified
    } else {
        VerifySpec::Commands(commands)
    }
}

/// Whether a `## Verify` line is markdown prose rather than a bare command: a
/// leading list bullet (`-`/`*`/`+`), which also covers a `- [ ]` checkbox, or a
/// leading backtick (a backtick-wrapped command). A real command never begins with
/// any of these — `argv[0]` is a program name, not a flag or a fence — so their
/// presence is an unambiguous authoring mistake (#181).
fn looks_like_markdown_list(line: &str) -> bool {
    matches!(line.chars().next(), Some('-' | '*' | '+' | '`'))
}

/// The operator-facing error for a malformed `## Verify` section: state the
/// contract and quote each offending line so the plan author can find and fix it.
fn malformed_error(malformed: &[&str]) -> String {
    let offenders = malformed
        .iter()
        .map(|l| format!("`{l}`"))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "`## Verify` must be one bare command per line, not a markdown list \
         (no `-`/`*`/`+` bullet, `- [ ]` checkbox, or backtick-wrapped command). \
         Offending line(s): {offenders}"
    )
}

/// Whether a `## Verify` line contains a backslash-escaped quote that [`tokenize`]
/// will misread (#268). The tokenizer does not honor backslash escapes, so a `\"`
/// inside a double-quoted region — or a `\'` inside a single-quoted region, or
/// either outside quotes — is taken as a real quote toggle, splitting the line into
/// a garbage argv. Walking tokenize's own quote state machine, this flags the first
/// backslash that escapes a quote the tokenizer would otherwise toggle. A backslash
/// before any non-quote char (a Windows path `C:\foo`) or an escaped backslash
/// (`\\`) is left alone, so real commands are not false-flagged.
fn has_escaped_quote(line: &str) -> bool {
    let mut in_single = false;
    let mut in_double = false;
    let mut prev_backslash = false;
    for ch in line.chars() {
        if prev_backslash {
            prev_backslash = false;
            // A quote the tokenizer would toggle, reached via `\`: the author meant
            // an escaped literal quote the tokenizer cannot honor.
            if (ch == '"' && !in_single) || (ch == '\'' && !in_double) {
                return true;
            }
            // The char was escaped (a literal — including a literal `\`): it neither
            // toggles a quote nor starts a new escape.
            continue;
        }
        match ch {
            '\\' => prev_backslash = true,
            '\'' if !in_double => in_single = !in_single,
            '"' if !in_single => in_double = !in_double,
            _ => {}
        }
    }
    false
}

/// The operator-facing error for a `## Verify` line whose backslash-escaped quote the
/// tokenizer cannot honor (#268): the gate runs argv with no shell, so name the
/// offending line(s) and point at the two shapes that DO tokenize cleanly — single
/// outer quotes or a single bare command (e.g. a `python -c` one-liner).
fn escaped_quote_error(offenders: &[&str]) -> String {
    let list = offenders
        .iter()
        .map(|l| format!("`{l}`"))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "`## Verify` runs each command as argv with no shell, and its tokenizer does \
         not honor backslash-escaped quotes (`\\\"`/`\\'`). Rewrite with single outer \
         quotes (e.g. `sh -c 'test \"$x\" = y'`) or as one bare command per line (e.g. \
         a `python -c \"...\"` one-liner). Offending line(s): {list}"
    )
}

/// Split one command line into argv tokens, honoring single and double quotes so
/// an argument with spaces (`sh -c "cargo test"`) stays one token. Whitespace
/// outside quotes separates tokens; an unterminated quote closes at end of line
/// (best-effort — the parser never fails). No shell metacharacter handling: this
/// is argv tokenization, not a shell.
pub fn tokenize(line: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut cur = String::new();
    let mut in_single = false;
    let mut in_double = false;
    let mut has_token = false;

    for ch in line.chars() {
        match ch {
            '\'' if !in_double => {
                in_single = !in_single;
                has_token = true;
            }
            '"' if !in_single => {
                in_double = !in_double;
                has_token = true;
            }
            c if c.is_whitespace() && !in_single && !in_double => {
                if has_token {
                    tokens.push(std::mem::take(&mut cur));
                    has_token = false;
                }
            }
            c => {
                cur.push(c);
                has_token = true;
            }
        }
    }
    if has_token {
        tokens.push(cur);
    }
    tokens
}

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

/// Grace for collecting a command's output after its exit status is known. A
/// descendant that inherited the pipes (a dev server a `## Verify` command
/// backgrounded) can hold the write-end open past the foreground exit, so the
/// reader never sees EOF; we wait only this long, then leak the stuck reader
/// instead of blocking the gate forever (#156). Mirrors the headless runner's
/// 5s collect grace.
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
                if Instant::now() >= deadline {
                    // Kill the whole tree, not just the direct `sh`/`bash`, so a
                    // detached grandchild can't survive to hold the pipe open.
                    ralphy_proc_util::kill_tree(&mut child);
                    timed_out = true;
                    break None;
                }
                thread::sleep(Duration::from_millis(50));
            }
            Err(_) => break None,
        }
    };

    // Collect the captured output with a bounded grace. Once the exit status is
    // known, a descendant that inherited the pipes keeps the write-end open so the
    // reader never sees EOF — waiting on the join unboundedly is what hung the gate
    // for ~43 min (#156). Leak a stuck reader (warned, tail may be truncated)
    // rather than block.
    let mut combined = String::new();
    if let Some(h) = out_handle {
        combined.push_str(&recv_and_join(&rx_out, h, "stdout"));
    }
    if let Some(h) = err_handle {
        combined.push_str(&recv_and_join(&rx_err, h, "stderr"));
    }

    // Teardown: kill any descendant that outlived the foreground command (a leaked
    // dev server holding a port), so a self-leaking `## Verify` command can't poison
    // later gates. The direct child's exit code stays the outcome. Best-effort; on
    // the timeout path the tree is already gone.
    ralphy_proc_util::kill_tree(&mut child);

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
/// join it and return the bytes as lossy UTF-8. On a natural exit or after a
/// [`ralphy_proc_util::kill_tree`] the pipe hits EOF and the thread sends promptly,
/// so the join is immediate; if the grace elapses a descendant still holds the
/// write-end — warn that the tail may be truncated and leak that one thread instead
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
                "verify gate reader did not finish within the collect grace — output tail may be truncated"
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

/// One command's status line in an honesty artifact: a ✓/✗ marker, the argv, and
/// why it failed. A spawn failure ("could not spawn — program not found") reads
/// distinctly from a signal kill ("exit killed"), a timeout, and a non-zero exit,
/// so every artifact names an unrunnable command the same way (#182). Shared by
/// [`comment`], [`repair_brief`], and [`spawn_failure_comment`].
fn status_line(cmd: &CommandOutcome) -> String {
    let line = cmd.argv.join(" ");
    if cmd.passed() {
        format!("\u{2713} {line}    exit 0")
    } else if cmd.spawn_failed {
        format!("\u{2717} {line}    could not spawn — program not found")
    } else if cmd.timed_out {
        format!("\u{2717} {line}    timed out")
    } else {
        let code = cmd
            .exit_code
            .map(|c| c.to_string())
            .unwrap_or_else(|| "killed".into());
        format!("\u{2717} {line}    exit {code}")
    }
}

/// Render the honesty artifact comment for a gate run (ADR-0011): one line per
/// command with a ✓/✗ marker and its exit code, plus a tail of the failing
/// command's output. This is what the operator reads in the morning to see why an
/// issue did or did not close.
pub fn comment(stamp: &str, report: &VerifyReport) -> String {
    let mut out = format!("## Verify (Ralphy run {stamp})\n\n");
    let header = if report.passed {
        "All verify commands passed — the runner saw the gate go green on the committed state.\n\n"
    } else {
        "Verify gate FAILED — the issue was left open and the branch handed back.\n\n"
    };
    out.push_str(header);

    out.push_str("```\n");
    for cmd in &report.commands {
        out.push_str(&status_line(cmd));
        out.push('\n');
    }
    out.push_str("```\n");

    // On failure, show the tail of the last (failing) command's output.
    if !report.passed {
        if let Some(last) = report.commands.last() {
            if !last.output_tail.is_empty() {
                out.push_str("\n<details><summary>Output tail</summary>\n\n```\n");
                out.push_str(&last.output_tail);
                out.push_str("\n```\n\n</details>\n");
            }
        }
    }

    out
}

/// Render the honesty artifact comment for a malformed `## Verify` section (#181):
/// the gate never ran — no command output to show — so the comment carries the
/// parse error naming the offending line(s) and states plainly that the issue was
/// left open. Mirrors [`comment`]'s framing so the operator reads one consistent
/// artifact shape whether the gate failed or could not run at all.
pub fn invalid_comment(stamp: &str, error: &str) -> String {
    format!(
        "## Verify (Ralphy run {stamp})\n\n\
         Verify gate could NOT run — the plan's `## Verify` section is malformed, \
         so no command was executed and the issue was left open.\n\n{error}\n"
    )
}

/// Render the honesty artifact for a gate whose command could not be spawned
/// (#182): the program was never found (a typo'd binary, a missing tool), so it
/// never ran. This is a spec/spawn problem, not a test failure — re-running the
/// SAME argv can never make it pass — so the runner skips the issue immediately
/// WITHOUT spending the repair budget. The comment says so, lists the commands
/// (marking the one that could not spawn), and shows the spawn error detail. Same
/// heading shape as [`comment`] so the operator reads one consistent artifact.
pub fn spawn_failure_comment(stamp: &str, report: &VerifyReport) -> String {
    let mut out = format!("## Verify (Ralphy run {stamp})\n\n");
    out.push_str(
        "Verify gate could NOT run — a `## Verify` command could not be spawned \
         (program not found), so it never executed. This is a spec/spawn problem, \
         not a test failure: re-running the same command cannot fix it, so the issue \
         was left open WITHOUT spending any repair attempts. Fix the command name in \
         the plan's `## Verify` section (or install the missing tool).\n\n",
    );

    out.push_str("```\n");
    for cmd in &report.commands {
        out.push_str(&status_line(cmd));
        out.push('\n');
    }
    out.push_str("```\n");

    // Show the spawn error detail (which program, what OS error) from the
    // command that could not run — the actionable part for the plan author.
    if let Some(failure) = report.first_failure() {
        if !failure.output_tail.is_empty() {
            out.push_str("\n<details><summary>Spawn error</summary>\n\n```\n");
            out.push_str(&failure.output_tail);
            out.push_str("\n```\n\n</details>\n");
        }
    }

    out
}

/// Render the repair brief the runner drops in the workspace after a failed gate
/// (ADR-0011 amendment). The executor's charter reads it to fix the root cause
/// and re-signal done, after which the runner re-runs the SAME commands. It names
/// the failing command(s) and shows the output tail, and is blunt that the gate —
/// not weakening the commands — is the only way through. `done_signal` is the
/// completion token the active adapter's charter defines, received as data
/// (ADR-0002) so the brief speaks the agent's own protocol.
pub fn repair_brief(stamp: &str, report: &VerifyReport, done_signal: &str) -> String {
    let mut out = format!("# Verify gate failed — repair required (Ralphy run {stamp})\n\n");
    out.push_str(&format!(
        "A previous session emitted `{done_signal}`, but the runner re-ran the \
         plan's `## Verify` commands over your committed work and the gate did NOT \
         pass. The repo is handed back to you to REPAIR.\n\n\
         Fix the ROOT CAUSE of the failure below, commit the fix, then emit \
         `{done_signal}` again so the runner re-checks the gate. Do NOT make the \
         gate pass by weakening, deleting, or skipping a verify command or by \
         editing the plan's `## Verify` section — the runner re-runs the SAME \
         commands and the gate is the authority.\n\n",
    ));

    out.push_str("Gate commands (✗ marks where it failed):\n\n```\n");
    for cmd in &report.commands {
        out.push_str(&status_line(cmd));
        out.push('\n');
    }
    out.push_str("```\n");

    if let Some(last) = report.commands.last() {
        if !last.output_tail.is_empty() {
            out.push_str("\nOutput tail of the failing command:\n\n```\n");
            out.push_str(&last.output_tail);
            out.push_str("\n```\n");
        }
    }

    out
}

#[cfg(test)]
mod tests;
