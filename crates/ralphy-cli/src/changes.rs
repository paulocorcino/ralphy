//! `ralphy changes list` — the working-tree change set of a repo, read-only.
//! Delegates to [`ralphy_core::changes`]; this module is only the clap surface
//! and the two output shapes. Read-only, so no run-lock guard (see
//! `mutate::branch`'s `List` arm).

use std::path::PathBuf;

use clap::{Args, Subcommand};

#[derive(Subcommand)]
pub(crate) enum ChangesCommand {
    /// List the repo's working-tree changes (read-only; never consults the run.lock).
    List(ChangesListArgs),
}

#[derive(Args)]
pub(crate) struct ChangesListArgs {
    /// Any path inside the target repo; resolved to its git toplevel.
    #[arg(long, default_value = ".")]
    pub(crate) repo: PathBuf,

    /// Output format: `json` emits `{changes}`; omitted prints one entry per line.
    #[arg(long)]
    pub(crate) format: Option<String>,
}

/// `ralphy changes list [--format json]`.
pub(crate) fn changes(cmd: ChangesCommand) -> anyhow::Result<()> {
    let ChangesCommand::List(args) = cmd;
    let repo_root = ralphy_core::git::resolve_toplevel(&args.repo)?;
    let list = ralphy_core::changes::changes(&repo_root)?;

    if args.format.as_deref() == Some("json") {
        let out = serde_json::json!({ "changes": list });
        println!("{out}");
    } else {
        for c in &list {
            match &c.original_path {
                Some(from) => println!("renamed {from} -> {}", c.path),
                None => println!("{} {}", status_word(c.status), c.path),
            }
        }
    }
    Ok(())
}

fn status_word(status: ralphy_core::ChangeStatus) -> &'static str {
    use ralphy_core::ChangeStatus::*;
    match status {
        Modified => "modified",
        Added => "added",
        Deleted => "deleted",
        Renamed => "renamed",
        Untracked => "untracked",
        Conflicted => "conflicted",
    }
}
