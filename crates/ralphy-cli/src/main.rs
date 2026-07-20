//! Ralphy's command-line entry point and composition root: parse flags, resolve
//! the repo, build the queue, build the Claude adapter, and hand off to the core
//! queue lifecycle.

use std::path::PathBuf;

use anyhow::Result;
use clap::Parser;
use ralphy_core::{git, Workspace};
use tracing::warn;

mod cli;
mod config;
mod daemon;
mod delivery;
mod events;
mod guard;
mod hook;
mod init;
mod install;
mod issues;
mod models;
mod mutate;
mod pricing;
mod run;
mod runlock;
mod runstate;
mod schedule;
mod split_agent;
mod telegram;
mod triage;
mod ui;
mod usage;

use cli::{Cli, Command, ConsolidateArgs, HookCommand};
// Re-exported at the crate root so `crate::CliAgent` stays a stable path for the
// sibling modules that select on it (e.g. `models`) after the CLI defs moved to
// `cli.rs`.
pub(crate) use cli::CliAgent;

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Run(args) => run::run_cmd(*args),
        Command::Consolidate(args) => consolidate_cmd(args),
        Command::Models(args) => models::run(args),
        Command::Config(args) => config::run(args),
        Command::Usage(args) => usage::usage_cmd(args),
        Command::Hook(HookCommand::Stop) => hook::run_stop_hook(),
        Command::Hook(HookCommand::Guard) => guard::run_guard_hook(),
        Command::Hook(HookCommand::Post) => hook::run_post_hook(),
        Command::Telegram(cmd) => telegram::run(cmd),
        Command::Install(args) => install::run(&args),
        Command::Init(args) => init::run(&args),
        Command::Triage(args) => triage::run(&args),
        Command::Issues(args) => issues::issues_cmd(args),
        Command::Schedule(cmd) => schedule::run(cmd),
        Command::Daemon(args) => daemon::run(&args),
        Command::Branch(cmd) => mutate::branch(cmd),
        Command::Label(cmd) => mutate::label(cmd),
    }
}

/// The default consolidation model/effort per vendor when the operator names none.
/// Claude keeps the deliberate opus/medium pairing (curation is judgment-heavy);
/// every other adapter passes `None`, letting it resolve its own default model and
/// (for Kimi/OpenCode, which have no reasoning-effort knob) ignore `effort`.
pub(crate) fn consolidate_defaults(
    agent: CliAgent,
) -> (Option<&'static str>, Option<&'static str>) {
    match agent {
        CliAgent::Claude => (Some("opus"), Some("medium")),
        CliAgent::Codex | CliAgent::Copilot | CliAgent::Kimi | CliAgent::OpenCode => (None, None),
    }
}

/// Dispatch the one-shot knowledge-consolidation session to the selected agent's
/// adapter (docs/adr/0031). Each adapter drives the SAME vendor-neutral charter
/// (`ralphy_core::PROMPT_CONSOLIDATE`); only the CLI invocation differs. Mirrors
/// the `diagnose_with_agent`/triage dispatch so `--agent` selects the vendor here
/// exactly as it does for the plan/execute loop and the other one-shots.
fn consolidate_with_agent(
    agent: CliAgent,
    ws: &Workspace,
    run_dir: &std::path::Path,
    model: Option<&str>,
    effort: Option<&str>,
    timeout: std::time::Duration,
) -> Result<()> {
    match agent {
        CliAgent::Claude => {
            ralphy_agent_claude::consolidate_knowledge(ws, run_dir, model, effort, timeout)
        }
        CliAgent::Codex => {
            ralphy_agent_codex::consolidate_knowledge(ws, run_dir, model, effort, timeout)
        }
        CliAgent::Copilot => {
            ralphy_agent_copilot::consolidate_knowledge(ws, run_dir, model, effort, timeout)
        }
        CliAgent::Kimi => {
            ralphy_agent_kimi::consolidate_knowledge(ws, run_dir, model, effort, timeout)
        }
        CliAgent::OpenCode => {
            ralphy_agent_opencode::consolidate_knowledge(ws, run_dir, model, effort, timeout)
        }
    }
}

/// The shared consolidation step behind both `ralphy consolidate` and the
/// automatic end-of-run trigger: run the curation session, verify it actually
/// rewrote `KNOWLEDGE.md` AND that the result passes the structural gate
/// (`knowledge::validate_knowledge`), then archive ONLY the notes the session
/// declared folded (its `<!-- folded: ... -->` marker) into `knowledge/raw/` —
/// unfolded notes stay loose, named in a warning, for the next pass. Returns
/// how many notes were archived. Errors — leaving every note loose for a retry
/// and restoring the pre-session `KNOWLEDGE.md` — when the session left the
/// file missing, unchanged, or structurally malformed (the rejected output is
/// kept as `KNOWLEDGE.rejected.md` in the run dir for inspection). `notes`
/// must be non-empty; callers gate on `loose_notes` first.
///
/// Callers are responsible for clearing `ANTHROPIC_API_KEY` (the subscription-quota
/// sentinel) before this runs — `run` already does so up front, `consolidate` does
/// it just before calling.
fn run_consolidation(
    agent: CliAgent,
    ws: &Workspace,
    run_dir: &std::path::Path,
    model: Option<&str>,
    effort: Option<&str>,
    max_minutes: u64,
    notes: &[PathBuf],
) -> Result<usize> {
    use anyhow::{bail, Context};
    use ralphy_core::knowledge;

    std::fs::create_dir_all(run_dir).ok();

    // The curated file before the session, to verify the session produced one.
    let before = std::fs::read_to_string(ws.knowledge_file()).ok();

    consolidate_with_agent(
        agent,
        ws,
        run_dir,
        model,
        effort,
        std::time::Duration::from_secs(max_minutes * 60),
    )?;

    let after = std::fs::read_to_string(ws.knowledge_file()).ok();
    let after = match after {
        Some(a) if before.as_deref() != Some(a.as_str()) => a,
        _ => bail!(
            "the session left KNOWLEDGE.md missing or unchanged — notes kept loose (see {})",
            run_dir.join("consolidate.log").display()
        ),
    };

    // Structural gate: a truncated/mangled file must not count as success. On
    // rejection restore the pre-session curated file (a mangled one would
    // poison every reader until the next consolidation) and keep the rejected
    // output beside the log for inspection.
    let folded = match knowledge::validate_knowledge(&after) {
        Ok(folded) => folded,
        Err(e) => {
            let _ = std::fs::write(run_dir.join("KNOWLEDGE.rejected.md"), &after);
            let restore = match &before {
                Some(b) => std::fs::write(ws.knowledge_file(), b),
                None => std::fs::remove_file(ws.knowledge_file()),
            };
            restore.context("restoring the pre-session KNOWLEDGE.md")?;
            bail!(
                "the session produced a malformed KNOWLEDGE.md ({e:#}) — change rejected, \
                 notes kept loose (rejected file kept at {})",
                run_dir.join("KNOWLEDGE.rejected.md").display()
            );
        }
    };

    let (to_archive, leftover) = knowledge::partition_folded(notes, &folded);
    if !leftover.is_empty() {
        let names: Vec<String> = leftover
            .iter()
            .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
            .collect();
        warn!(
            notes = %names.join(", "),
            "notes not folded by the session — kept loose for the next pass"
        );
    }
    knowledge::archive_notes(ws, &to_archive)
}

/// `ralphy consolidate`: run a one-shot agent session that curates the loose
/// knowledge notes into `KNOWLEDGE.md`, then archive the consumed notes under
/// `knowledge/raw/`. The session's only deliverable is the curated file — the
/// command verifies it actually changed before archiving anything, so a failed
/// or no-op session leaves the notes loose for a retry.
fn consolidate_cmd(args: ConsolidateArgs) -> Result<()> {
    use ralphy_core::knowledge;

    let repo_root = git::resolve_toplevel(&args.repo)?;
    let ws = Workspace::new(&repo_root);

    let notes = knowledge::loose_notes(&ws);
    if notes.is_empty() {
        println!("No loose knowledge notes under .ralphy/knowledge/ — nothing to consolidate.");
        return Ok(());
    }
    let names: Vec<String> = notes
        .iter()
        .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        .collect();
    println!(
        "Consolidating {} note(s) into KNOWLEDGE.md: {}",
        notes.len(),
        names.join(", ")
    );

    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
    let run_dir = ws.run_dir(&stamp);

    // Same subscription-quota sentinel as `run` (see the comment there).
    std::env::set_var("ANTHROPIC_API_KEY", "");

    // An explicit `--model`/`--effort` wins; otherwise fall back to the vendor's
    // default pairing (opus/medium for Claude, the adapter's own for the rest).
    let (def_model, def_effort) = consolidate_defaults(args.agent);
    let model = args.model.and_then(non_empty);
    let effort = args.effort.and_then(non_empty);

    let archived = run_consolidation(
        args.agent,
        &ws,
        &run_dir,
        model.as_deref().or(def_model),
        effort.as_deref().or(def_effort),
        args.max_minutes,
        &notes,
    )?;
    println!(
        "Done: KNOWLEDGE.md updated, {archived} note(s) archived into .ralphy/knowledge/raw/."
    );
    Ok(())
}

/// Collapse an empty string to `None`, leaving a non-empty one as `Some`. The
/// shared helper the run orchestrator's adapter builders lean on so an empty
/// override never reaches an adapter as a real value.
pub(crate) fn non_empty(s: String) -> Option<String> {
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// #237: the four one-shot dispatch sites must route Copilot to real work, not
    /// bail. Fragments so the needle cannot match this very assertion.
    #[test]
    fn copilot_one_shots_are_wired() {
        let needle = concat!("does not support ", "one-shot");
        for src in [
            include_str!("init/run.rs"),
            include_str!("init/issues.rs"),
            include_str!("triage.rs"),
            include_str!("main.rs"),
        ] {
            assert!(!src.contains(needle), "stale one-shot bail found");
        }
        assert_eq!(consolidate_defaults(CliAgent::Copilot), (None, None));
    }
}
