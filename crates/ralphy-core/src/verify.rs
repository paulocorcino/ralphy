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
    /// Section absent or present-but-empty — a planner omission. The runner falls
    /// back to `settings.json` `verify.command`.
    Unspecified,
}

/// Parse the `## Verify` section of a plan markdown into a [`VerifySpec`].
///
/// One command per line, code-fence-tolerant (` ``` ` lines are ignored), with
/// quote-aware argv tokenization so `sh -c "cargo test"` survives as three
/// tokens. A lone `none` line (case-insensitive) is the explicit opt-out. An
/// absent or whitespace-only section is [`VerifySpec::Unspecified`].
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
    /// The process exit code, or `None` when it timed out or was killed by a
    /// signal (no numeric code).
    pub exit_code: Option<i32>,
    /// The command exceeded the gate's remaining time budget and was killed.
    pub timed_out: bool,
    /// Last few lines of combined stdout+stderr — empty on success when there was
    /// no output. Captured on every command so the artifact comment can show it.
    pub output_tail: String,
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

/// Run a single command, draining its output through threads (so a chatty command
/// never deadlocks on a full pipe) and killing it if the shared `deadline` passes.
fn run_one(argv: &[String], repo_root: &Path, deadline: Instant) -> CommandOutcome {
    // An empty argv cannot be run; treat it as a spawn failure so the gate stops.
    let Some((program, rest)) = argv.split_first() else {
        return CommandOutcome {
            argv: argv.to_vec(),
            exit_code: None,
            timed_out: false,
            output_tail: "empty command".into(),
        };
    };

    // Resolve the program so the gate's no-shell argv spawn (ADR-0011) still finds
    // Windows shell shims: `pnpm`/`npm`/`yarn`/`npx` are `.cmd` scripts a bare
    // `CreateProcess` never locates (it only appends `.exe`) and cannot execute
    // even if named. On Unix this is a pass-through. See [`spawn_command`].
    let (spawn_program, spawn_args) = spawn_command(program, rest);
    let mut child = match Command::new(&spawn_program)
        .args(&spawn_args)
        .current_dir(repo_root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            return CommandOutcome {
                argv: argv.to_vec(),
                exit_code: None,
                timed_out: false,
                output_tail: format!("failed to spawn `{program}`: {e}"),
            };
        }
    };

    // Drain stdout+stderr concurrently so neither pipe can fill and wedge the
    // child while we poll for the deadline.
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let out_handle = stdout.map(|mut s| {
        thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = s.read_to_end(&mut buf);
            buf
        })
    });
    let err_handle = stderr.map(|mut s| {
        thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = s.read_to_end(&mut buf);
            buf
        })
    });

    let mut timed_out = false;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    timed_out = true;
                    break None;
                }
                thread::sleep(Duration::from_millis(50));
            }
            Err(_) => break None,
        }
    };

    let mut combined = String::new();
    if let Some(h) = out_handle {
        if let Ok(buf) = h.join() {
            combined.push_str(&String::from_utf8_lossy(&buf));
        }
    }
    if let Some(h) = err_handle {
        if let Ok(buf) = h.join() {
            combined.push_str(&String::from_utf8_lossy(&buf));
        }
    }

    CommandOutcome {
        argv: argv.to_vec(),
        exit_code: status.and_then(|s| s.code()),
        timed_out,
        output_tail: tail(&combined),
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
    use std::ffi::OsString;
    match resolve_program(program) {
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
        // Unresolved: keep the original name so the spawn surfaces the same
        // "program not found" failure as before, naming the real command.
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

/// Resolve `program` against the live `PATH`/`PATHEXT`. Windows-only; Unix needs no
/// such resolution. Split from [`resolve_in`] so the resolution logic unit-tests
/// without mutating the process environment.
#[cfg(windows)]
fn resolve_program(program: &str) -> Option<std::path::PathBuf> {
    let pathext = std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into());
    let exts: Vec<&str> = pathext.split(';').filter(|s| !s.is_empty()).collect();
    let search: Vec<std::path::PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    resolve_in(program, &search, &exts)
}

/// Find `program` in `search`, honoring `exts` (the `PATHEXT` list, each entry
/// carrying its leading dot) so a bare `pnpm` resolves to `pnpm.cmd`. A name that
/// already carries a path separator is used as-is (a relative name resolves against
/// the spawn's `current_dir`). A name without an extension matches ONLY via the
/// `PATHEXT` candidates — never a bare, extensionless file, which on Windows is the
/// non-executable bash shim that ships beside the `.cmd`.
#[cfg(windows)]
fn resolve_in(
    program: &str,
    search: &[std::path::PathBuf],
    exts: &[&str],
) -> Option<std::path::PathBuf> {
    use std::path::{Path, PathBuf};
    if program.contains('/') || program.contains('\\') {
        return Some(PathBuf::from(program));
    }
    let has_ext = Path::new(program).extension().is_some();
    for dir in search {
        if has_ext {
            let cand = dir.join(program);
            if cand.is_file() {
                return Some(cand);
            }
        } else {
            for ext in exts {
                let cand = dir.join(format!("{program}{ext}"));
                if cand.is_file() {
                    return Some(cand);
                }
            }
        }
    }
    None
}

/// Keep the last [`TAIL_BYTES`] of `s`, trimmed to a whole-line boundary so the
/// tail never starts mid-line. Empty stays empty.
fn tail(s: &str) -> String {
    let trimmed = s.trim_end();
    if trimmed.len() <= TAIL_BYTES {
        return trimmed.to_string();
    }
    let start = trimmed.len() - TAIL_BYTES;
    // Advance to the next newline so we drop the partial leading line.
    let slice = &trimmed[start..];
    match slice.find('\n') {
        Some(nl) => slice[nl + 1..].to_string(),
        None => slice.to_string(),
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
        let line = cmd.argv.join(" ");
        if cmd.passed() {
            out.push_str(&format!("\u{2713} {line}    exit 0\n"));
        } else if cmd.timed_out {
            out.push_str(&format!("\u{2717} {line}    timed out\n"));
        } else {
            let code = cmd
                .exit_code
                .map(|c| c.to_string())
                .unwrap_or_else(|| "killed".into());
            out.push_str(&format!("\u{2717} {line}    exit {code}\n"));
        }
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

/// Render the repair brief the runner drops in the workspace after a failed gate
/// (ADR-0011 amendment). The executor's charter reads it to fix the root cause
/// and re-signal done, after which the runner re-runs the SAME commands. It names
/// the failing command(s) and shows the output tail, and is blunt that the gate —
/// not weakening the commands — is the only way through.
pub fn repair_brief(stamp: &str, report: &VerifyReport) -> String {
    let mut out = format!("# Verify gate failed — repair required (Ralphy run {stamp})\n\n");
    out.push_str(
        "A previous session emitted `RALPHY_DONE_EXIT`, but the runner re-ran the \
         plan's `## Verify` commands over your committed work and the gate did NOT \
         pass. The repo is handed back to you to REPAIR.\n\n\
         Fix the ROOT CAUSE of the failure below, commit the fix, then emit \
         `RALPHY_DONE_EXIT` again so the runner re-checks the gate. Do NOT make the \
         gate pass by weakening, deleting, or skipping a verify command or by \
         editing the plan's `## Verify` section — the runner re-runs the SAME \
         commands and the gate is the authority.\n\n",
    );

    out.push_str("Gate commands (✗ marks where it failed):\n\n```\n");
    for cmd in &report.commands {
        let line = cmd.argv.join(" ");
        if cmd.passed() {
            out.push_str(&format!("\u{2713} {line}    exit 0\n"));
        } else if cmd.timed_out {
            out.push_str(&format!("\u{2717} {line}    timed out\n"));
        } else {
            let code = cmd
                .exit_code
                .map(|c| c.to_string())
                .unwrap_or_else(|| "killed".into());
            out.push_str(&format!("\u{2717} {line}    exit {code}\n"));
        }
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
mod tests {
    use super::*;

    #[test]
    fn parse_commands_one_per_line() {
        let md = "# Plan\n\n## Verify\n\ncargo fmt --check\ncargo test -p ralphy-core\n\n## Next\n";
        let spec = parse_verify(md);
        assert_eq!(
            spec,
            VerifySpec::Commands(vec![
                vec!["cargo".into(), "fmt".into(), "--check".into()],
                vec![
                    "cargo".into(),
                    "test".into(),
                    "-p".into(),
                    "ralphy-core".into()
                ],
            ])
        );
    }

    #[test]
    fn parse_none_is_opt_out() {
        assert_eq!(parse_verify("## Verify\n\nnone\n"), VerifySpec::None);
        // Case-insensitive.
        assert_eq!(parse_verify("## Verify\nNONE\n"), VerifySpec::None);
    }

    #[test]
    fn parse_absent_section_is_unspecified() {
        assert_eq!(
            parse_verify("# Plan\n\n## Steps\n- [ ] do\n"),
            VerifySpec::Unspecified
        );
    }

    #[test]
    fn parse_empty_section_is_unspecified() {
        let md = "## Verify\n\n## Notes\nstuff\n";
        assert_eq!(parse_verify(md), VerifySpec::Unspecified);
    }

    #[test]
    fn parse_is_fence_tolerant() {
        let md = "## Verify\n\n```\ncargo test\n```\n";
        assert_eq!(
            parse_verify(md),
            VerifySpec::Commands(vec![vec!["cargo".into(), "test".into()]])
        );
    }

    #[test]
    fn parse_quoted_args_stay_one_token() {
        let md = "## Verify\n\nsh -c \"cargo test --all\"\n";
        assert_eq!(
            parse_verify(md),
            VerifySpec::Commands(vec![vec![
                "sh".into(),
                "-c".into(),
                "cargo test --all".into()
            ]])
        );
    }

    #[test]
    fn parse_single_quotes_too() {
        assert_eq!(
            tokenize("echo 'hello world' bye"),
            vec!["echo", "hello world", "bye"]
        );
    }

    #[test]
    fn parse_stops_at_next_heading() {
        let md = "## Verify\ncargo test\n## Other\ncargo bogus\n";
        assert_eq!(
            parse_verify(md),
            VerifySpec::Commands(vec![vec!["cargo".into(), "test".into()]])
        );
    }

    #[test]
    fn tail_keeps_whole_last_lines() {
        let s = "line1\nline2\nline3";
        assert_eq!(tail(s), "line1\nline2\nline3");
    }

    /// A portable command that exits 0 on every platform: the OS shell builtin via
    /// the host interpreter. We avoid a shell and instead use a tiny program both
    /// platforms ship — but argv-only. On Windows `cmd /c exit 0`, elsewhere
    /// `sh -c "exit 0"`.
    fn ok_cmd() -> Vec<String> {
        if cfg!(windows) {
            vec!["cmd".into(), "/c".into(), "exit 0".into()]
        } else {
            vec!["sh".into(), "-c".into(), "exit 0".into()]
        }
    }

    fn fail_cmd() -> Vec<String> {
        if cfg!(windows) {
            vec!["cmd".into(), "/c".into(), "exit 3".into()]
        } else {
            vec!["sh".into(), "-c".into(), "exit 3".into()]
        }
    }

    #[test]
    fn run_all_pass() {
        let dir = std::env::temp_dir();
        let report = run(&[ok_cmd(), ok_cmd()], &dir, Duration::from_secs(30));
        assert!(report.passed, "both ok commands pass");
        assert_eq!(report.commands.len(), 2);
        assert!(report.commands.iter().all(|c| c.passed()));
    }

    #[test]
    fn run_stops_at_first_failure() {
        let dir = std::env::temp_dir();
        let report = run(
            &[ok_cmd(), fail_cmd(), ok_cmd()],
            &dir,
            Duration::from_secs(30),
        );
        assert!(!report.passed, "a non-zero exit fails the gate");
        // The third command never ran — the gate stops at the first failure.
        assert_eq!(report.commands.len(), 2, "stops after the failing command");
        assert_eq!(report.commands[1].exit_code, Some(3));
    }

    #[test]
    fn run_spawn_failure_is_a_failure() {
        let dir = std::env::temp_dir();
        let report = run(
            &[vec!["definitely-not-a-real-binary-xyz".into()]],
            &dir,
            Duration::from_secs(30),
        );
        assert!(!report.passed, "an unspawnable command fails the gate");
        assert!(report.commands[0].output_tail.contains("failed to spawn"));
    }

    #[test]
    fn comment_marks_pass_and_fail() {
        let report = VerifyReport {
            commands: vec![
                CommandOutcome {
                    argv: vec!["cargo".into(), "fmt".into()],
                    exit_code: Some(0),
                    timed_out: false,
                    output_tail: String::new(),
                },
                CommandOutcome {
                    argv: vec!["cargo".into(), "test".into()],
                    exit_code: Some(101),
                    timed_out: false,
                    output_tail: "panicked at assertion".into(),
                },
            ],
            passed: false,
        };
        let c = comment("stamp-1", &report);
        assert!(c.contains("## Verify (Ralphy run stamp-1)"));
        assert!(c.contains("\u{2713} cargo fmt"));
        assert!(c.contains("\u{2717} cargo test    exit 101"));
        assert!(c.contains("panicked at assertion"), "failing tail shown");
    }

    /// On Windows `pnpm` ships as a bare bash shim AND a `pnpm.cmd`; only the
    /// `.cmd` is executable, and PATHEXT resolution must return it — not the
    /// extensionless file a bare `CreateProcess` would choke on. This is the exact
    /// failure that left a Node monorepo's gate red with "program not found".
    #[cfg(windows)]
    #[test]
    fn resolve_in_finds_cmd_shim_not_the_bare_shell_file() {
        let dir =
            std::env::temp_dir().join(format!("ralphy-verify-resolve-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("pnpm"), "#!/bin/sh\n").unwrap(); // bash shim
        std::fs::write(dir.join("pnpm.cmd"), "@echo off\n").unwrap(); // cmd shim
        let exts = [".COM", ".EXE", ".BAT", ".CMD"];

        let got = resolve_in("pnpm", std::slice::from_ref(&dir), &exts)
            .expect("pnpm resolves via PATHEXT");
        // Filesystem is case-insensitive; compare on name (lowercased) + parent so
        // a `.CMD`-vs-`.cmd` PathBuf mismatch doesn't fail a correct resolution.
        assert_eq!(
            got.file_name().unwrap().to_string_lossy().to_lowercase(),
            "pnpm.cmd",
            "resolves to the .cmd shim, not the bare bash file"
        );
        assert_eq!(got.parent().unwrap(), dir);

        assert!(
            resolve_in("definitely-absent-xyz", std::slice::from_ref(&dir), &exts).is_none(),
            "a missing program stays unresolved"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn repair_brief_names_failure_and_forbids_weakening() {
        let report = VerifyReport {
            commands: vec![CommandOutcome {
                argv: vec!["pnpm".into(), "install".into()],
                exit_code: Some(1),
                timed_out: false,
                output_tail: "ERR_PNPM_LOCKFILE_MISMATCH".into(),
            }],
            passed: false,
        };
        let b = repair_brief("stamp-9", &report);
        assert!(b.contains("repair required"));
        assert!(b.contains("\u{2717} pnpm install    exit 1"));
        assert!(
            b.contains("ERR_PNPM_LOCKFILE_MISMATCH"),
            "failing tail shown"
        );
        // The gate is the authority — the brief must forbid gaming it.
        assert!(b.contains("RALPHY_DONE_EXIT"));
        assert!(b.to_lowercase().contains("root cause"));
        assert!(
            b.contains("SAME"),
            "must say the runner re-runs the same commands"
        );
    }
}
