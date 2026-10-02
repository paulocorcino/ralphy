//! `ralphy gemini prepare-root` — Gemini's own configuration for a workbench
//! console. The daemon runs it before it opens a Gemini console in a repo that
//! has none (ADR-0040 Amendment 3); the work itself is the adapter's.

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Args, Subcommand};

#[derive(Subcommand)]
pub(crate) enum GeminiCommand {
    /// Write Gemini's own configuration and policy into the repo's `.ralphy`
    /// folder. Starts no agent.
    PrepareRoot(PrepareRootArgs),
}

#[derive(Args)]
pub(crate) struct PrepareRootArgs {
    /// Any folder inside the repo.
    #[arg(long, default_value = ".")]
    pub(crate) repo: PathBuf,
}

/// `ralphy gemini prepare-root [--repo <dir>]`. Prints nothing on success.
pub(crate) fn gemini(cmd: GeminiCommand) -> Result<()> {
    let GeminiCommand::PrepareRoot(args) = cmd;
    let repo_root = ralphy_core::git::resolve_toplevel(&args.repo)?;
    ralphy_agent_gemini::prepare_console_root(&repo_root).with_context(|| {
        format!(
            "preparing Gemini's configuration in {}",
            repo_root.display()
        )
    })?;
    Ok(())
}
