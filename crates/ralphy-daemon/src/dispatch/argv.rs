//! The argv composers: one per verb family, each an exact vector over
//! validated client input — a malformed parameter yields [`ArgvError`] and
//! NO argv (docs/adr/0036 §2).

use super::{agent_flag, BranchMode, Verb};
use crate::session::Agent;

mod config;
mod paths;

pub use config::config_argv;
#[cfg(test)]
pub(crate) use config::LOCAL_ONLY_KEYS;
pub use paths::{blob_read_argv, changes_commit_argv, changes_paths_argv};
#[cfg(test)]
use paths::{validated_path, MAX_COMMIT_MESSAGE, MAX_PATH_CHARS, MAX_STAGE_PATHS};

/// A run's params failed closed-enum validation (ADR-0036 §1): the named field
/// was absent or carried an out-of-enum / free-text value. The caller sends one
/// refusal frame and spawns nothing — no partial argv reaches the CLI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ArgvError {
    #[error("invalid run param: {0}")]
    BadParam(&'static str),
}

/// Compose the blessed argv for `verb` from its closed-enum params (ADR-0036 §1).
/// The verb picks the static shape; `Run` reads `agent` (required), `planAgent`
/// (optional), and `branchMode` (required) from `payload`, each validated against
/// a closed enum. Any absent-required or out-of-enum value yields [`ArgvError`]
/// and NO argv — the client never contributes a raw argument, so remote input can
/// never widen the command line.
pub fn spawn_argv(verb: Verb, payload: &serde_json::Value) -> Result<Vec<String>, ArgvError> {
    let owned = |parts: &[&str]| parts.iter().map(ToString::to_string).collect::<Vec<_>>();
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
        // Non-Spawn verbs never reach the spawn path (`answer_one` answers the
        // Observe/Query/Mutate verbs and returns first); refuse an argv
        // defensively.
        Verb::TreeList
        | Verb::TreeFind
        | Verb::TreeGrep
        | Verb::FileRead
        | Verb::FileStat
        | Verb::TreeSize
        | Verb::ImageRead
        | Verb::NoteRead
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
        | Verb::FileUpload
        | Verb::NoteWrite
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
        | Verb::ProjectRemove
        | Verb::HostAliases
        | Verb::HostKey
        | Verb::HostTrust
        | Verb::HostCheck
        | Verb::HostAdd
        | Verb::HostInstall
        | Verb::HostRemove
        | Verb::DirList
        | Verb::ProjectAdd => Err(ArgvError::BadParam("verb")),
    }
}

/// The static argv for the board Query verb: `issues --format json --board` —
/// the whole-tracker Kanban fold (ADR-0036 slice 6). Takes no client input; the
/// verb alone fixes the command line.
pub fn board_argv() -> Vec<String> {
    ["issues", "--format", "json", "--board"]
        .iter()
        .map(ToString::to_string)
        .collect()
}

/// Compose the argv for the issue-detail Query verb: `issues show <n> --format
/// json`. `<n>` is a validated positive `u64` read from `payload.number` (the sole
/// client input) — anything absent, zero, or non-integer yields [`ArgvError`] and
/// NO argv, so remote input can never widen the command line.
pub fn issue_show_argv(payload: &serde_json::Value) -> Result<Vec<String>, ArgvError> {
    let n = payload
        .get("number")
        .and_then(serde_json::Value::as_u64)
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
        .map(ToString::to_string)
        .collect()
}

/// The static argv for the worktree-list Query verb: `worktree list --format json`
/// (ADR-0063 §2, issue #403). Takes no client input; the verb alone fixes the
/// command line, so listing the workbench worktrees for the picker can never be
/// widened by remote input.
pub fn worktree_list_argv() -> Vec<String> {
    ["worktree", "list", "--format", "json"]
        .iter()
        .map(ToString::to_string)
        .collect()
}

/// The static argv for the changes-list Query verb: `changes list --format json`
/// (issue #307). Takes no client input; the verb alone fixes the command line, so
/// reading the working-tree change set can never be widened by remote input.
pub fn changes_list_argv() -> Vec<String> {
    ["changes", "list", "--format", "json"]
        .iter()
        .map(ToString::to_string)
        .collect()
}

/// The static argv for the sync-status Query verb: `sync status --format json`
/// (issue #316). Takes no client input; the verb alone fixes the command line,
/// and the command itself makes no network call.
pub fn sync_status_argv() -> Vec<String> {
    ["sync", "status", "--format", "json"]
        .iter()
        .map(ToString::to_string)
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
/// #316) / `sync push` (issue #320). These verbs take NO client input, so the
/// verb argument is the only parameter there is to malform — anything else
/// yields [`ArgvError`] and NO argv, mirroring [`branch_argv`]'s guard.
pub fn sync_argv(verb: Verb) -> Result<Vec<String>, ArgvError> {
    #[expect(
        clippy::wildcard_enum_match_arm,
        reason = "a verb this builder does not serve is refused, which fails closed"
    )]
    let sub = match verb {
        Verb::SyncFetch => "fetch",
        Verb::SyncPull => "pull",
        Verb::SyncPush => "push",
        _ => return Err(ArgvError::BadParam("verb")),
    };
    Ok(vec!["sync".to_string(), sub.to_string()])
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
/// read from `payload.name`; an empty or ill-shaped name (`well_shaped_ref`)
/// yields [`ArgvError`] and NO argv. The `--` guard ends option parsing on the
/// ralphy command line; the shape gate is what keeps the name from being an
/// option to the `git checkout` behind it.
pub fn branch_argv(verb: Verb, payload: &serde_json::Value) -> Result<Vec<String>, ArgvError> {
    #[expect(
        clippy::wildcard_enum_match_arm,
        reason = "a verb this builder does not serve is refused, which fails closed"
    )]
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
        .and_then(serde_json::Value::as_u64)
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

#[cfg(test)]
mod tests;
