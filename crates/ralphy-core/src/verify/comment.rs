//! The honesty artifacts a gate run leaves on the issue (ADR-0011): the
//! comment, the invalid-section comment, the spawn-failure comment and the
//! repair brief the executor is handed.

use super::{CommandOutcome, VerifyReport};

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
