//! The argv composers: one per verb family, each an exact vector over
//! validated client input — a malformed parameter yields [`ArgvError`] and
//! NO argv (docs/adr/0036 §2).

use std::fmt;

use super::{agent_flag, BranchMode, Verb};
use crate::session::Agent;

/// A run's params failed closed-enum validation (ADR-0036 §1): the named field
/// was absent or carried an out-of-enum / free-text value. The caller sends one
/// refusal frame and spawns nothing — no partial argv reaches the CLI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArgvError {
    BadParam(&'static str),
}

impl fmt::Display for ArgvError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ArgvError::BadParam(field) => write!(f, "invalid run param: {field}"),
        }
    }
}

impl std::error::Error for ArgvError {}

/// Compose the blessed argv for `verb` from its closed-enum params (ADR-0036 §1).
/// The verb picks the static shape; `Run` reads `agent` (required), `planAgent`
/// (optional), and `branchMode` (required) from `payload`, each validated against
/// a closed enum. Any absent-required or out-of-enum value yields [`ArgvError`]
/// and NO argv — the client never contributes a raw argument, so remote input can
/// never widen the command line.
pub fn spawn_argv(verb: Verb, payload: &serde_json::Value) -> Result<Vec<String>, ArgvError> {
    let owned = |parts: &[&str]| parts.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    match verb {
        Verb::Triage => Ok(owned(&["triage", "--if-idle", "--yes"])),
        Verb::PushQueue => Ok(owned(&["issues", "--push"])),
        Verb::Run => {
            let agent = payload
                .get("agent")
                .and_then(|v| v.as_str())
                .and_then(Agent::from_query)
                .ok_or(ArgvError::BadParam("agent"))?;
            // `--if-idle` is the daemon-spawn overlap-skip semantics (kept, not
            // weakened); the modal's live preview omits it — that string is human-
            // facing, this argv is the wire contract.
            let mut argv = owned(&["run", "--if-idle", "--agent", agent_flag(agent)]);
            // Optional planner: absent or JSON `null` ⇒ omit; a present non-null
            // value MUST be a known agent, else refuse (no free-text planner).
            match payload.get("planAgent") {
                None => {}
                Some(v) if v.is_null() => {}
                Some(v) => {
                    let plan = v
                        .as_str()
                        .and_then(Agent::from_query)
                        .ok_or(ArgvError::BadParam("planAgent"))?;
                    argv.push("--plan-agent".to_string());
                    argv.push(agent_flag(plan).to_string());
                }
            }
            let mode = payload
                .get("branchMode")
                .and_then(|v| v.as_str())
                .and_then(BranchMode::from_query)
                .ok_or(ArgvError::BadParam("branchMode"))?;
            argv.push("--branch-mode".to_string());
            argv.push(mode.as_flag().to_string());
            Ok(argv)
        }
        // Non-Spawn verbs never reach the spawn path (the `command_ws`
        // Observe/Query/Mutate branches answer and return first); refuse an argv
        // defensively.
        Verb::TreeList
        | Verb::TreeFind
        | Verb::TreeGrep
        | Verb::FileRead
        | Verb::ImageRead
        | Verb::RunsList
        | Verb::ConfigGet
        | Verb::BoardList
        | Verb::IssueShow
        | Verb::ConfigSet
        | Verb::ConfigUnset
        | Verb::FileWrite
        | Verb::FileCreate
        | Verb::FileRename
        | Verb::FileCopy
        | Verb::FileDelete
        | Verb::ImageWrite
        | Verb::BranchList
        | Verb::BranchSwitch
        | Verb::BranchCreate
        | Verb::WorktreeList
        | Verb::WorktreeAdd
        | Verb::WorktreeRemove
        | Verb::LabelSet
        | Verb::ChangesList
        | Verb::BlobRead
        | Verb::SyncStatus
        | Verb::SyncFetch
        | Verb::SyncPull
        | Verb::SyncPush
        | Verb::ChangesStage
        | Verb::ChangesUnstage
        | Verb::ChangesCommit
        | Verb::ChangesDiscard
        | Verb::RunStop
        | Verb::PlanDiscard
        | Verb::ProjectRemove => Err(ArgvError::BadParam("verb")),
    }
}

/// The static argv for the board Query verb: `issues --format json --board` —
/// the whole-tracker Kanban fold (ADR-0036 slice 6). Takes no client input; the
/// verb alone fixes the command line.
pub fn board_argv() -> Vec<String> {
    ["issues", "--format", "json", "--board"]
        .iter()
        .map(|s| s.to_string())
        .collect()
}

/// Compose the argv for the issue-detail Query verb: `issues show <n> --format
/// json`. `<n>` is a validated positive `u64` read from `payload.number` (the sole
/// client input) — anything absent, zero, or non-integer yields [`ArgvError`] and
/// NO argv, so remote input can never widen the command line.
pub fn issue_show_argv(payload: &serde_json::Value) -> Result<Vec<String>, ArgvError> {
    let n = payload
        .get("number")
        .and_then(|v| v.as_u64())
        .filter(|&n| n > 0)
        .ok_or(ArgvError::BadParam("number"))?;
    Ok(vec![
        "issues".to_string(),
        "show".to_string(),
        n.to_string(),
        "--format".to_string(),
        "json".to_string(),
    ])
}

/// The static argv for the branch-list Query verb: `branch list --format json`
/// (issue #199). Takes no client input; the verb alone fixes the command line, so
/// listing branches for the switcher can never be widened by remote input.
pub fn branch_list_argv() -> Vec<String> {
    ["branch", "list", "--format", "json"]
        .iter()
        .map(|s| s.to_string())
        .collect()
}

/// The static argv for the worktree-list Query verb: `worktree list --format json`
/// (ADR-0063 §2, issue #403). Takes no client input; the verb alone fixes the
/// command line, so listing the workbench worktrees for the picker can never be
/// widened by remote input.
pub fn worktree_list_argv() -> Vec<String> {
    ["worktree", "list", "--format", "json"]
        .iter()
        .map(|s| s.to_string())
        .collect()
}

/// The static argv for the changes-list Query verb: `changes list --format json`
/// (issue #307). Takes no client input; the verb alone fixes the command line, so
/// reading the working-tree change set can never be widened by remote input.
pub fn changes_list_argv() -> Vec<String> {
    ["changes", "list", "--format", "json"]
        .iter()
        .map(|s| s.to_string())
        .collect()
}

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
/// [`validated_path`] shape gate.
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
        "blob".to_string(),
        "read".to_string(),
        "--revision".to_string(),
        revision.to_string(),
        "--path".to_string(),
        path,
        "--format".to_string(),
        "json".to_string(),
    ])
}

/// The static argv for the sync-status Query verb: `sync status --format json`
/// (issue #316). Takes no client input; the verb alone fixes the command line,
/// and the command itself makes no network call.
pub fn sync_status_argv() -> Vec<String> {
    ["sync", "status", "--format", "json"]
        .iter()
        .map(|s| s.to_string())
        .collect()
}

/// The longest `runid` the wire will carry. A runid is a ULID (26 chars); the
/// bound is generous enough for a future format and tight enough that the
/// composed argv can never be the attack.
const MAX_RUNID_CHARS: usize = 64;

/// Compose the argv for the run-stop Mutate verb: `stop --runid=<id>`
/// (docs/adr/0054).
///
/// `runid` is the sole client input and is validated as a bare identifier —
/// ASCII alphanumerics only. That is stricter than the ULID alphabet on purpose:
/// it admits no `-`, no `=`, no path separator and no whitespace, so the value
/// can never be read as a flag, a path, or a second argument, and the
/// `--runid=<v>` single-token form leaves nothing for an option parser to split.
/// An out-of-shape value yields [`ArgvError`] and the caller spawns nothing.
///
/// The runid is REQUIRED here even though the CLI can infer it when a repo has
/// exactly one live run: the browser is always addressing a run it can see in
/// the panel, and inferring on its behalf would let a click land on a run that
/// started between the render and the request.
pub fn run_stop_argv(payload: &serde_json::Value) -> Result<Vec<String>, ArgvError> {
    let runid = payload
        .get("runid")
        .and_then(|v| v.as_str())
        .filter(|r| {
            !r.is_empty()
                && r.chars().count() <= MAX_RUNID_CHARS
                && r.chars().all(|c| c.is_ascii_alphanumeric())
        })
        .ok_or(ArgvError::BadParam("runid"))?;
    Ok(vec!["stop".to_string(), format!("--runid={runid}")])
}

/// The longest registry slug the wire accepts. GitHub's own owner+name ceiling is
/// far under this; the bound exists so the composed argv cannot be grown by a
/// remote.
const MAX_SLUG_CHARS: usize = 256;

/// Compose `daemon remove <slug>` for [`Verb::ProjectRemove`] (issue #363).
///
/// The slug shape is pinned to what `ralphy_core::git::project_slug` can produce:
/// either `<owner>/<name>` (a forge remote — and `slug_from_url` takes the
/// remote's last two segments VERBATIM, so `~user` is a real owner on a
/// self-hosted host) or a SINGLE `path-<hash>` segment (a remoteless repo, the
/// fallback that same function falls back to). Both forms
/// allow only `[A-Za-z0-9._~-]` per segment, every segment non-empty, and no
/// leading `-` — so the value admits no second path separator, no whitespace and
/// no shell metacharacter, and can never be read as a flag or widen the command
/// line. An out-of-shape value yields [`ArgvError`] and the caller spawns
/// nothing.
pub fn project_remove_argv(payload: &serde_json::Value) -> Result<Vec<String>, ArgvError> {
    let slug = payload
        .get("slug")
        .and_then(|v| v.as_str())
        .filter(|s| {
            let mut segments = s.split('/');
            s.len() <= MAX_SLUG_CHARS
                && !s.starts_with('-')
                && s.bytes().all(|b| {
                    b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-' | b'~' | b'/')
                })
                && segments.all(|seg| !seg.is_empty())
                && s.matches('/').count() <= 1
        })
        .ok_or(ArgvError::BadParam("slug"))?;
    Ok(vec![
        "daemon".to_string(),
        "remove".to_string(),
        slug.to_string(),
    ])
}

/// Compose the argv for a sync Mutate verb: `sync fetch` / `sync pull` (issue
/// #316) / `sync push` (issue #320). These verbs take NO client input, so the verb argument is the only
/// parameter there is to malform — anything else yields [`ArgvError`] and NO
/// argv, mirroring [`branch_argv`]'s guard.
pub fn sync_argv(verb: Verb) -> Result<Vec<String>, ArgvError> {
    let sub = match verb {
        Verb::SyncFetch => "fetch",
        Verb::SyncPull => "pull",
        Verb::SyncPush => "push",
        _ => return Err(ArgvError::BadParam("verb")),
    };
    Ok(vec!["sync".to_string(), sub.to_string()])
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
const MAX_STAGE_PATHS: usize = 256;

const MAX_PATH_CHARS: usize = 1024;

const MAX_COMMIT_MESSAGE: usize = 4096;

/// Compose the argv for a path-carrying Mutate verb: `changes stage
/// --path=<p>…` / `changes unstage --path=<p>…` (issue #318) / `changes discard
/// --path=<p>…` (issue #319). Every element of
/// `payload.paths` goes through the shared [`validated_path`] gate; an absent,
/// empty, over-long, or malformed list yields [`ArgvError`] and NO argv, so a
/// glob or a pathspec can never cross the wire.
///
/// Each path is fused into its own `--path=<p>` token, the dash-safe form
/// [`label_argv`] already uses: a path passed as a separate token would be
/// re-read by clap as a flag.
pub fn changes_paths_argv(
    verb: Verb,
    payload: &serde_json::Value,
) -> Result<Vec<String>, ArgvError> {
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

    let mut argv = vec!["changes".to_string(), sub.to_string()];
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
/// from `payload.message`; absent, blank, or longer than [`MAX_COMMIT_MESSAGE`]
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
        "changes".to_string(),
        "commit".to_string(),
        format!("--message={message}"),
    ])
}

/// The lexical shape of a git ref the workbench may name — a branch to switch
/// to or create, a base to cut from (security audit 2026-09-21, F10/F11). PURE
/// like [`validated_path`]: no git spawn (ADR-0036 — the daemon never runs
/// git), so this is git's `check-ref-format` rules restated, and the CLI's
/// own `check-ref-format`/`rev-parse` gate is what stands in the repo. What
/// this refuses NEVER reaches the CLI: a leading `-` (the `--` guard on the
/// ralphy command line does not survive the hop into `git checkout <name>`),
/// `..`, `@{`, ASCII control and space, `~ ^ : ? * [ \`, a trailing `/` or
/// `.lock`, a `.`-leading component. `origin/main`, `feat/x` and a SHA pass.
fn well_shaped_ref(raw: &str) -> bool {
    !raw.is_empty()
        && !raw.starts_with('-')
        && !raw.ends_with('/')
        && !raw.ends_with(".lock")
        && !raw.contains("..")
        && !raw.contains("@{")
        && !raw.contains("//")
        && raw != "@"
        && !raw
            .chars()
            .any(|c| c.is_ascii_control() || c == ' ' || "~^:?*[\\".contains(c))
        && !raw
            .split('/')
            .any(|seg| seg.starts_with('.') || seg.ends_with('.'))
}

/// Compose the argv for a branch Mutate verb: `branch switch -- <name>` /
/// `branch create -- <name>` (issue #199). `<name>` is the sole client input,
/// read from `payload.name`; an empty or ill-shaped name ([`well_shaped_ref`])
/// yields [`ArgvError`] and NO argv. The `--` guard ends option parsing on the
/// ralphy command line; the shape gate is what keeps the name from being an
/// option to the `git checkout` behind it.
pub fn branch_argv(verb: Verb, payload: &serde_json::Value) -> Result<Vec<String>, ArgvError> {
    let sub = match verb {
        Verb::BranchSwitch => "switch",
        Verb::BranchCreate => "create",
        _ => return Err(ArgvError::BadParam("verb")),
    };
    let name = payload
        .get("name")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|n| well_shaped_ref(n))
        .ok_or(ArgvError::BadParam("name"))?;
    Ok(vec![
        "branch".to_string(),
        sub.to_string(),
        "--".to_string(),
        name.to_string(),
    ])
}

/// Compose the argv for the worktree-add Mutate verb: `worktree add
/// [--base=<ref>] -- <name>` (ADR-0063 §2). `name` is read exactly as
/// [`branch_argv`] reads it; `base` is omitted when absent or `null`, becomes
/// the single-token `--base=<ref>` when a non-empty string (dash-safe, like
/// [`label_argv`]'s `--add=`), and an empty or non-string `base` is a
/// malformed request — [`ArgvError`] and NO argv.
pub fn worktree_add_argv(payload: &serde_json::Value) -> Result<Vec<String>, ArgvError> {
    let name = payload
        .get("name")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|n| well_shaped_ref(n))
        .ok_or(ArgvError::BadParam("name"))?;
    // Both shape-gated ([`well_shaped_ref`]): `base` rides as `--base=<ref>` and
    // then becomes the last positional of `git worktree add` (audit F11).
    let base = match payload.get("base") {
        None | Some(serde_json::Value::Null) => None,
        Some(v) => Some(
            v.as_str()
                .map(str::trim)
                .filter(|b| well_shaped_ref(b))
                .ok_or(ArgvError::BadParam("base"))?,
        ),
    };
    let mut argv = vec!["worktree".to_string(), "add".to_string()];
    if let Some(base) = base {
        argv.push(format!("--base={base}"));
    }
    argv.push("--".to_string());
    argv.push(name.to_string());
    Ok(argv)
}

/// Compose the argv for the worktree-remove Mutate verb: `worktree remove --
/// <name>` (ADR-0063 §1's gates run in the CLI; the live-console gate runs in
/// the daemon before this is ever called). `name` is read exactly as
/// [`branch_argv`] reads it — empty/whitespace-only yields [`ArgvError`] and NO
/// argv.
pub fn worktree_remove_argv(payload: &serde_json::Value) -> Result<Vec<String>, ArgvError> {
    let name = payload
        .get("name")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .ok_or(ArgvError::BadParam("name"))?;
    Ok(vec![
        "worktree".to_string(),
        "remove".to_string(),
        "--".to_string(),
        name.to_string(),
    ])
}

/// Compose the argv for the label Mutate verb: `label set <n> --{op}=<label>`
/// (issue #199). Validates `number` (positive `u64`), `label` (non-empty), and
/// `op` (∈ {`add`,`remove`}); any bad field yields [`ArgvError`] and NO argv. The
/// single-token `--add=<label>`/`--remove=<label>` form is dash-safe: a label
/// starting with `-` passed as a separate token would be parsed by clap as a flag.
pub fn label_argv(payload: &serde_json::Value) -> Result<Vec<String>, ArgvError> {
    let number = payload
        .get("number")
        .and_then(|v| v.as_u64())
        .filter(|&n| n > 0)
        .ok_or(ArgvError::BadParam("number"))?;
    let label = payload
        .get("label")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .ok_or(ArgvError::BadParam("label"))?;
    let op = match payload.get("op").and_then(|v| v.as_str()) {
        Some("add") => "add",
        Some("remove") => "remove",
        _ => return Err(ArgvError::BadParam("op")),
    };
    Ok(vec![
        "label".to_string(),
        "set".to_string(),
        number.to_string(),
        format!("--{op}={label}"),
    ])
}

/// Whether `key` is a well-shaped config key (`^[a-z0-9_.]+$`): a closed
/// character class, NOT a value allowlist (that lives in the CLI's
/// `require_known_key`, ADR-0036 Decision). Argv-safety comes from no-shell
/// `Command::args()`, not a closed key set; an unknown-but-well-shaped key is
/// accepted here and rejected by `ralphy config set`, relayed as a Mutate error.
fn well_shaped_key(key: &str) -> bool {
    !key.is_empty()
        && key
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'.')
}

/// Config keys whose VALUE names a program a later run spawns. `verify.command`
/// becomes `argv[0]` of the verification gate's child (`ralphy_core::verify`), so
/// setting it is arbitrary deferred code execution — persistent on disk, fired
/// under a run the operator started, and invisible in `git status` because
/// `.ralphy/` self-ignores.
///
/// Shape-checking the value cannot fix this: the gate spawns with
/// `current_dir(repo_root)` and Windows `CreateProcess` searches the current
/// directory ahead of `PATH`, so even a bare program name resolves to a file the
/// Write verbs can plant. Deny the KEY at the remote boundary instead — the
/// operator's own `ralphy config set` is unaffected.
/// `pub(crate)` so the settings-panel gate in `lib.rs` can assert the schema
/// declares each of these `readonly` — a key denied here but offered as an
/// editable field is a control that takes an edit and answers "refused".
pub(crate) const EXEC_ADJACENT_KEYS: [&str; 1] = ["verify.command"];

/// Whether `key` may be set through the daemon's `config.set`/`config.unset`.
/// Well-shaped AND not exec-adjacent.
fn remotely_settable_key(key: &str) -> bool {
    well_shaped_key(key) && !EXEC_ADJACENT_KEYS.contains(&key)
}

/// Compose the blessed argv for a config Query/Mutate verb (ADR-0036 §2). The
/// verb picks the static shape; `ConfigSet`/`ConfigUnset` read `key` (must match
/// `^[a-z0-9_.]+$`) and — for `set` — a non-empty `value` from `payload`, each
/// passed as a single argv token (never a shell string). Any absent/ill-shaped
/// value yields [`ArgvError`] and NO argv. Runs in `cwd = <repo path>` with no
/// `--repo` flag (defaults to `.`).
pub fn config_argv(verb: Verb, payload: &serde_json::Value) -> Result<Vec<String>, ArgvError> {
    let owned = |parts: &[&str]| parts.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    let key = || -> Result<String, ArgvError> {
        let key = payload
            .get("key")
            .and_then(|v| v.as_str())
            .ok_or(ArgvError::BadParam("key"))?;
        if remotely_settable_key(key) {
            Ok(key.to_string())
        } else {
            Err(ArgvError::BadParam("key"))
        }
    };
    match verb {
        Verb::ConfigGet => Ok(owned(&["config", "get", "--json"])),
        // `--` ends option parsing: without it a value like `--help`/`-V` is
        // consumed by clap as a flag (help exits 0 → silent false-success), and a
        // legit dash-leading value can't be stored. The key is `^[a-z0-9_.]+$`, so
        // it is never a flag; `--` protects the free-text value.
        Verb::ConfigSet => {
            let key = key()?;
            let value = payload
                .get("value")
                .and_then(|v| v.as_str())
                .filter(|v| !v.is_empty())
                .ok_or(ArgvError::BadParam("value"))?;
            Ok(vec![
                "config".to_string(),
                "set".to_string(),
                "--".to_string(),
                key,
                value.to_string(),
            ])
        }
        Verb::ConfigUnset => Ok(vec![
            "config".to_string(),
            "unset".to_string(),
            "--".to_string(),
            key()?,
        ]),
        // Non-config verbs never route here (`command_ws` picks the branch by
        // effect class); refuse an argv defensively.
        _ => Err(ArgvError::BadParam("verb")),
    }
}

#[cfg(test)]
mod tests;
