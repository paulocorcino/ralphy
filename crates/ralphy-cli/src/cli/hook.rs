//! The internal `ralphy hook` subcommands the agents call during a run, and
//! the agent selectors of the guard and the status hook.

use clap::{Subcommand, ValueEnum};

#[derive(Subcommand)]
pub(crate) enum HookCommand {
    /// Record in `$RALPHY_FLAG_FILE` that the agent session ended.
    Stop,
    /// Block commands and file writes that would destroy work.
    Guard {
        /// Also refuse a check command already measured as slow while the plan
        /// still has open steps. Execute sessions only.
        #[arg(long)]
        cost_gate: bool,
        /// The agent whose hook runs this; `claude` when not given.
        #[arg(long, value_enum, default_value_t = GuardVendor::Claude)]
        vendor: GuardVendor,
    },
    /// Record how long each check command took.
    Post,
    /// Record what the agent is doing in `$RALPHY_STATUS_FILE`, so the workbench
    /// can show it. Prints `{}` for Claude, an allow decision for Antigravity
    /// and nothing for the other agents, and always exits 0.
    // ADR-0059, ADR-0085 D6.
    Status {
        /// The agent whose hook runs this. Without it, `claude`, so a settings
        /// file written by an older version keeps working.
        #[arg(value_enum, default_value_t = HookVendor::Claude)]
        vendor: HookVendor,
    },
}

/// The agent a `hook status` call comes from: it decides what the hook
/// prints and where the question text is in the payload.
#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
pub(crate) enum HookVendor {
    Claude,
    Codex,
    // One word, like `--agent opencode`; clap would derive `open-code`.
    #[value(name = "opencode")]
    OpenCode,
    Antigravity,
}

/// The agent a `hook guard` call comes from: it decides how the payload is
/// read and how the decision is answered.
#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
pub(crate) enum GuardVendor {
    Claude,
    Antigravity,
}
