//! `ralphy blob read` — a file's content at a git revision, read-only. The
//! clap surface and the wire shapes over [`ralphy_core::blob`]; the daemon's
//! argv builder validates the path by SHAPE, and this module — the process that
//! actually stands in the repo — enforces real containment before any read.

use std::path::{Component, Path, PathBuf};

use anyhow::{bail, Result};
use clap::{Args, Subcommand, ValueEnum};
use serde::Serialize;

/// The `--format json` wire shape. A struct, not `json!`: serde emits fields in
/// DECLARATION order, while `json!` builds a `Map` that sorts keys without the
/// `preserve_order` feature — and `status` leads this reply.
#[derive(Serialize)]
struct BlobReply<'a> {
    status: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<&'a str>,
}

#[derive(Subcommand)]
pub(crate) enum BlobCommand {
    /// Print a file's content at a revision (read-only; never consults the run.lock).
    Read(BlobReadArgs),
}

/// Which revision to read at. A closed set: this slice diffs against HEAD only.
#[derive(Clone, Copy, Debug, ValueEnum)]
pub(crate) enum RevisionArg {
    Head,
}

#[derive(Args)]
pub(crate) struct BlobReadArgs {
    /// Any path inside the target repo; resolved to its git toplevel.
    #[arg(long, default_value = ".")]
    pub(crate) repo: PathBuf,

    /// Revision to read at.
    #[arg(long, value_enum)]
    pub(crate) revision: RevisionArg,

    /// Repo-relative path of the file to read.
    #[arg(long)]
    pub(crate) path: String,

    /// Output format: `json` emits `{status, …}`; omitted prints the raw text.
    #[arg(long)]
    pub(crate) format: Option<String>,
}

/// `ralphy blob read --revision head --path <p> [--format json]`.
pub(crate) fn blob(cmd: BlobCommand) -> Result<()> {
    let BlobCommand::Read(args) = cmd;
    let repo_root = ralphy_core::git::resolve_toplevel(&args.repo)?;
    // Containment BEFORE the read: `git rev-parse HEAD:../x` would otherwise
    // reach a sibling tree entry, and a refusal here is the last line of defence
    // for any caller that is not the daemon's pure argv builder.
    guard_contained(&args.path)?;

    let rev = match args.revision {
        RevisionArg::Head => ralphy_core::Revision::Head,
    };
    let blob = ralphy_core::blob::read(&repo_root, rev, &args.path)?;

    if args.format.as_deref() == Some("json") {
        let reply = match &blob {
            ralphy_core::Blob::Text(s) => BlobReply {
                status: "present",
                content: Some(s.as_str()),
                reason: None,
            },
            ralphy_core::Blob::Absent => BlobReply {
                status: "absent",
                content: None,
                reason: None,
            },
            ralphy_core::Blob::Binary => BlobReply {
                status: "refused",
                content: None,
                reason: Some("binary"),
            },
            ralphy_core::Blob::TooLarge => BlobReply {
                status: "refused",
                content: None,
                reason: Some("too large"),
            },
        };
        println!("{}", serde_json::to_string(&reply)?);
    } else {
        match &blob {
            ralphy_core::Blob::Text(s) => print!("{s}"),
            ralphy_core::Blob::Absent => {}
            ralphy_core::Blob::Binary => bail!("refused: binary"),
            ralphy_core::Blob::TooLarge => bail!("refused: too large"),
        }
    }
    Ok(())
}

/// Refuse a path that is absolute, rooted, drive-prefixed, or walks up out of
/// the repo. Shape-only — no filesystem access, so a missing file still reads
/// as absent rather than as an escape.
fn guard_contained(path: &str) -> Result<()> {
    let p = Path::new(path);
    let escapes = p.is_absolute()
        || p.components().any(|c| {
            matches!(
                c,
                Component::RootDir | Component::Prefix(_) | Component::ParentDir
            )
        });
    if escapes {
        bail!("path escapes the repo: {path}");
    }
    Ok(())
}
