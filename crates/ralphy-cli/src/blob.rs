//! `ralphy blob read` — a file's content at a git revision, read-only. The
//! clap surface and the wire shapes over [`ralphy_core::blob`]; the daemon's
//! argv builder validates the path by SHAPE, and this module — the process that
//! actually stands in the repo — resolves the real toplevel and enforces
//! containment against it before any read.

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
    guard_contained(&repo_root, &args.path)?;

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

/// Refuse a path that does not land inside `repo_root`.
///
/// Two layers, deliberately: the shape gate refuses an absolute, rooted,
/// drive-prefixed or `..`-bearing path (and normalises `\` so a POSIX host sees
/// the same shape a Windows one does), then the RESOLVED join is required to stay
/// under the root — this is the real-containment check, made against the actual
/// filesystem by the process standing in the repo. Resolution is lexical, not
/// `canonicalize`: a path that does not exist yet must still be judged, because
/// absence at the revision is a legitimate answer, not an escape.
///
/// The shape gate is PLATFORM-INDEPENDENT by construction, matching the daemon's
/// `dispatch::validated_path`: `Component::Prefix` exists only on Windows, so on
/// Linux `C:\Windows\win.ini` normalises to the ordinary relative path
/// `C:/Windows/win.ini` and every `std::path` arm above waves it through. A host
/// must not decide what a remote path means — the drive prefix is refused on
/// every platform, spelled out rather than delegated to `std::path`.
fn guard_contained(repo_root: &Path, path: &str) -> Result<()> {
    let normalised = path.replace('\\', "/");
    let p = Path::new(&normalised);
    let drive_prefixed = {
        let b = normalised.as_bytes();
        b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':'
    };
    let bad_shape = normalised.is_empty()
        || p.is_absolute()
        || drive_prefixed
        || p.components().any(|c| {
            matches!(
                c,
                Component::RootDir | Component::Prefix(_) | Component::ParentDir
            )
        });
    if bad_shape || !lands_inside(p) {
        bail!("path escapes the repo: {path}");
    }
    // The root must really be a repo root on THIS filesystem — the containment
    // above is only meaningful relative to a root that exists.
    if !repo_root.is_dir() {
        bail!("not a repo directory: {}", repo_root.display());
    }
    Ok(())
}

/// Whether `rel` still sits under its root once `.`/`..` are folded lexically.
///
/// Lexical on purpose, NOT `canonicalize`: a path committed at the revision but
/// deleted from the working tree does not exist on disk, and refusing it would
/// break the one case — reviewing a deletion — that most needs the HEAD side.
fn lands_inside(rel: &Path) -> bool {
    let mut depth = 0usize;
    for c in rel.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                if depth == 0 {
                    return false;
                }
                depth -= 1;
            }
            Component::Normal(_) => depth += 1,
            Component::RootDir | Component::Prefix(_) => return false,
        }
    }
    depth > 0
}
