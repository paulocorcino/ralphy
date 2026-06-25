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

    let mut child = match Command::new(program)
        .args(rest)
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
}
