//! The argv composers of the verbs that carry a repo path or a commit message:
//! the shape gate every path passes, and the blob and changes verbs.

use super::{ArgvError, Verb};

/// Validate a client-supplied repo path BY SHAPE, returning it normalised.
/// `None` — never a partially-cleaned path — for anything refused.
///
/// The shape rules, and what each is for:
/// - backslashes normalise to `/`, so a Windows-shaped path from the browser is
///   the SAME path, not a second accepted spelling;
/// - empty, or a leading `-`: would read as a flag at the next hop;
/// - a leading `/` or a `<letter>:` drive prefix: escapes the repo root;
/// - any `..` segment: climbs out of it;
/// - a leading `:`: git's PATHSPEC MAGIC prefix (`:(glob)`, `:(top)`), which is
///   a shape rule of the same family as the leading `-`.
///
/// Glob METACHARACTERS (`*`, `?`, `[`) are deliberately NOT refused: they are
/// legal filenames, and common ones — `app/blog/[slug]/page.tsx` is the Next.js
/// dynamic-route convention and `[Content_Types].xml` sits in every unpacked
/// OOXML file. Refusing them would make those rows unreadable in the diff tab
/// and would poison a whole group action, while buying nothing: git is invoked
/// with `--literal-pathspecs` (so no name is ever expanded at the last hop) and
/// `ralphy_core::worktree` refuses any path absent from the change set.
///
/// PURE by contract: no `std::fs`, no `canonicalize`, no `Path::is_absolute` —
/// the host's own filesystem must not decide what a remote path means, and a
/// Windows daemon must refuse a POSIX-absolute path just as a Linux one refuses
/// `C:\`. Real containment is the CLI's job, standing in the repo.
///
/// ONE definition, shared by EVERY path-carrying verb ([`blob_read_argv`],
/// [`changes_paths_argv`]), so the read surface and the write surface can never
/// drift apart on what a path is.
pub(crate) fn validated_path(raw: &str) -> Option<String> {
    let path = raw.replace('\\', "/");
    let drive_prefixed = {
        let b = path.as_bytes();
        b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':'
    };
    if path.is_empty()
        || path.starts_with('-')
        || path.starts_with('/')
        || path.starts_with(':')
        || drive_prefixed
        || path.split('/').any(|seg| seg == "..")
    {
        return None;
    }
    Some(path)
}

/// Compose the argv for the blob-read Query verb: `blob read --revision head
/// --path <p> --format json` (issue #311). Two client inputs, both closed down:
/// `revision` is a one-value enum (`head`), and `path` goes through the shared
/// `validated_path` shape gate.
pub fn blob_read_argv(payload: &serde_json::Value) -> Result<Vec<String>, ArgvError> {
    let revision = match payload.get("revision").and_then(|v| v.as_str()) {
        Some("head") => "head",
        _ => return Err(ArgvError::BadParam("revision")),
    };

    let path = payload
        .get("path")
        .and_then(|v| v.as_str())
        .and_then(validated_path)
        .ok_or(ArgvError::BadParam("path"))?;

    Ok(vec![
        "blob".to_owned(),
        "read".to_owned(),
        "--revision".to_owned(),
        revision.to_owned(),
        "--path".to_owned(),
        path,
        "--format".to_owned(),
        "json".to_owned(),
    ])
}

/// The wire bounds for the working-tree Mutate verbs. They live HERE, at the
/// wire, and not in `ralphy-core`: the daemon must not depend on core
/// (ADR-0032), and these bound what a REMOTE may send, not what the operation
/// can do.
///
/// The count bound alone does NOT bound the command line — a path is otherwise
/// unbounded — so `MAX_PATH_CHARS` is enforced too, and together they cap the
/// composed argv at roughly 256 KiB: over Windows' 32767-char limit in the
/// worst case, but a spawn failure there is graceful (`"mutation write
/// failed"`), while an UNSTATED bound is the kind of comment that rots. The
/// realistic shape — 256 paths averaging 60 chars — stays far under it.
pub(super) const MAX_STAGE_PATHS: usize = 256;

pub(super) const MAX_PATH_CHARS: usize = 1024;

pub(super) const MAX_COMMIT_MESSAGE: usize = 4096;

/// Compose the argv for a path-carrying Mutate verb: `changes stage
/// --path=<p>…` / `changes unstage --path=<p>…` (issue #318) / `changes discard
/// --path=<p>…` (issue #319). Every element of
/// `payload.paths` goes through the shared `validated_path` gate; an absent,
/// empty, over-long, or malformed list yields [`ArgvError`] and NO argv, so a
/// glob or a pathspec can never cross the wire.
///
/// Each path is fused into its own `--path=<p>` token, the dash-safe form
/// [`super::label_argv`] already uses: a path passed as a separate token would be
/// re-read by clap as a flag.
pub fn changes_paths_argv(
    verb: Verb,
    payload: &serde_json::Value,
) -> Result<Vec<String>, ArgvError> {
    #[expect(
        clippy::wildcard_enum_match_arm,
        reason = "a verb this builder does not serve is refused, which fails closed"
    )]
    let sub = match verb {
        Verb::ChangesStage => "stage",
        Verb::ChangesUnstage => "unstage",
        Verb::ChangesDiscard => "discard",
        _ => return Err(ArgvError::BadParam("verb")),
    };
    let paths = payload
        .get("paths")
        .and_then(|v| v.as_array())
        .filter(|list| !list.is_empty() && list.len() <= MAX_STAGE_PATHS)
        .ok_or(ArgvError::BadParam("paths"))?;

    let mut argv = vec!["changes".to_owned(), sub.to_owned()];
    for value in paths {
        let path = value
            .as_str()
            .filter(|p| p.chars().count() <= MAX_PATH_CHARS)
            .and_then(validated_path)
            .ok_or(ArgvError::BadParam("paths"))?;
        argv.push(format!("--path={path}"));
    }
    Ok(argv)
}

/// Compose the argv for the commit Mutate verb: `changes commit
/// --message=<msg>` (issue #318). The message is the sole client input, read
/// from `payload.message`; absent, blank, or longer than `MAX_COMMIT_MESSAGE`
/// chars yields [`ArgvError`] and NO argv.
///
/// The message is ONE fused token and the argv is exactly three long — a
/// message split across two tokens would be refused by clap the moment it began
/// with `-`, so the fusion is the whole point.
pub fn changes_commit_argv(payload: &serde_json::Value) -> Result<Vec<String>, ArgvError> {
    let message = payload
        .get("message")
        .and_then(|v| v.as_str())
        .filter(|m| !m.trim().is_empty() && m.chars().count() <= MAX_COMMIT_MESSAGE)
        .ok_or(ArgvError::BadParam("message"))?;
    Ok(vec![
        "changes".to_owned(),
        "commit".to_owned(),
        format!("--message={message}"),
    ])
}
