//! `ralphy antigravity prepare-root`: the folder Ralphy owns for Antigravity,
//! ready for a workbench console. The daemon may not import the adapter, so it
//! runs this command (ADR-0040 Amendment 3, ADR-0032 §10); the work itself is
//! the adapter's. Also the options of an Antigravity one-shot.

use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};
use clap::Subcommand;
use ralphy_agent_antigravity::{AntigravitySettings, OneShotOptions};
use ralphy_core::{Settings, Workspace};

#[derive(Subcommand)]
pub(crate) enum AntigravityCommand {
    /// Write the hooks, the configuration and the skills into the folder
    /// Ralphy owns for Antigravity. Starts no agent.
    PrepareRoot,
}

/// `ralphy antigravity prepare-root`. Prints nothing on success.
pub(crate) fn antigravity(cmd: &AntigravityCommand) -> Result<()> {
    let AntigravityCommand::PrepareRoot = cmd;
    ralphy_agent_antigravity::prepare_console_root()
        .context("preparing the folder Ralphy owns for Antigravity")?;
    Ok(())
}

/// The options of an Antigravity one-shot in `repo`. The guard-proof waiver
/// is read from the repo's settings only, never from a flag.
pub(crate) fn one_shot_options<'a>(
    repo: &Path,
    model: Option<&'a str>,
    effort: Option<&'a str>,
    timeout: Duration,
) -> OneShotOptions<'a> {
    OneShotOptions {
        model,
        effort,
        timeout,
        run_without_guard_proof: waiver_in(repo),
    }
}

/// `antigravity.run_without_guard_proof_i_understand_the_risk` of `repo`;
/// `false`, with a warning, when the settings cannot be read.
fn waiver_in(repo: &Path) -> bool {
    let section = Settings::load(&Workspace::new(repo))
        .and_then(|s| s.agent_settings::<AntigravitySettings>(AntigravitySettings::SECTION));
    section.map_or_else(
        |e| {
            tracing::warn!(error = %e, "unreadable antigravity settings: the hook proof stays on");
            false
        },
        |a| a.run_without_guard_proof_i_understand_the_risk,
    )
}
