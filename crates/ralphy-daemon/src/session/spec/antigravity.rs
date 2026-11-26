//! The Antigravity console's launch arguments and environment: the owned root
//! as `--gemini_dir`, and the opt-in that lets the agent run the guard's rules.

use std::ffi::OsString;
use std::path::Path;

use super::SessionSpec;

/// The dim first line of a console whose agent may run the workflow rules (ADR-0093 D5).
pub(super) const WORKFLOW_NOTICE: &str = concat!(
    "\x1b[2m",
    "The agent may switch branches, push and write pull requests in this console.",
    "\x1b[0m\r\n",
);

/// The Antigravity part of [`spec_with_status`](super::spec_with_status): the owned root as
/// `--gemini_dir`, no autonomy flag (ADR-0086 D8), and `RALPHY_STATUS_FILE`
/// for the status hook the root already holds (D9). The route refuses the
/// launch before this when the root cannot be resolved; if it still cannot,
/// the program is cleared so the spawn fails and `agy` never runs elsewhere.
pub(super) fn add_antigravity_launch(
    spec: &mut SessionSpec,
    repo_root: &Path,
    status: Option<crate::agent_state::StatusFiles>,
) {
    match ralphy_proc_util::antigravity::owned_root() {
        Ok(root) => {
            let mut flag = OsString::from("--gemini_dir=");
            flag.push(root);
            spec.args.push(flag);
        }
        Err(e) => {
            tracing::error!(error = %format!("{e:#}"), "no owned root for an Antigravity console");
            spec.program = OsString::new();
            return;
        }
    }
    if antigravity_console_agent_workflow(repo_root) {
        spec.env.push((
            OsString::from(ralphy_proc_util::GUARD_CONSOLE_ENV),
            OsString::from("1"),
        ));
        spec.start_notice = Some(WORKFLOW_NOTICE.to_owned());
    }
    let Some(files) = status else {
        return;
    };
    match files.create_status() {
        Ok(()) => super::tail_status_file(spec, files),
        Err(e) => tracing::warn!(error = %e, "could not write the console's agent-state file"),
    }
}

/// Whether the operator lets the agent of this repo's Antigravity consoles
/// run the guard's workflow rules, read from
/// `["antigravity"]["console_agent_workflow_i_understand_the_risk"]` with the
/// same reparse and the same invariant as [`claude_console_named`](super::claude_console_named).
// ADR-0093 D2, D3.
fn antigravity_console_agent_workflow(repo_root: &Path) -> bool {
    super::section_opt_in(
        repo_root,
        "antigravity",
        "console_agent_workflow_i_understand_the_risk",
    )
}
