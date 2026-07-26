//! The remote command dispatcher (docs/adr/0032 §2): the closed vocabulary of
//! verbs a browser button can trigger, each mapped to EXACTLY one blessed
//! `ralphy` invocation and spawned as a detached child. This is the whole
//! attack surface of remote triggers, kept deliberately narrow:
//!
//! - A remote request names a [`Verb`] by string; [`Verb::from_query`] rejects
//!   everything outside `run`/`triage`/`push`, so no `kill`/`stop` verb — and no
//!   free-text — is reachable. The verb, not the client, chooses the argv.
//! - [`spawn_argv`] composes the argv from the verb plus CLOSED-ENUM params only
//!   (`agent`/`planAgent` via [`crate::session::Agent::from_query`], `branchMode`
//!   via [`BranchMode::from_query`]); any out-of-enum or free-text value yields
//!   [`ArgvError`] and the caller spawns nothing. The client never contributes a
//!   raw argument. The program is always the resolved `ralphy` exe run via
//!   `Command::new(exe).args(argv)` — never a shell string, so nothing the client
//!   sends is interpreted by `sh`/`cmd`.
//!
//! REGISTRY (ADR-0036 §1–2): each [`Verb`] carries an [`EffectClass`]; only
//! `Spawn` verbs reach the CLI, and [`spawn_argv`] is their argv table.
//!
//! TEARDOWN INVARIANT (the inverse of `session`'s): a dispatched run keeps its
//! OWN lifecycle — the daemon must NEVER kill it on shutdown or client
//! disconnect (PRD #157 story 18/20: "spawned runs keep their own lifecycle", "a
//! daemon crash never kills a run"). The [`Child`] handle here is `wait`-only;
//! it has no kill and dropping it does not kill (std semantics). The `/ws/command`
//! handler's teardown arms enforce the rest.
//!
//! OUTPUT STREAMING (issue #180): the child's stdout+stderr are merged into a
//! single OS pipe ([`Child::take_output`]) so the `/ws/command` handler can
//! stream the live output into the UI log pane. This does NOT weaken teardown:
//! a Rust child ignores `SIGPIPE`, so after a daemon crash the child's writes to
//! the now-broken pipe return a non-fatal `EPIPE`/Windows write error rather than
//! killing it. The obligation this adds is on the DAEMON, not the child: a live
//! daemon MUST drain the reader to EOF continuously, so the pipe never fills and
//! stalls the child. The handler's detached drain task discharges that.
//!
//! The [`Spawner`]/[`Child`] seam keeps this module unit-testable: a `FakeSpawner`
//! records the argv and returns a preset exit code without touching the OS.

use std::ffi::{OsStr, OsString};
use std::fmt;
use std::path::Path;

use anyhow::Result;

use crate::session::Agent;

/// The effect class of a verb (ADR-0036 §2). The registry's shape: `Native` runs
/// in-daemon, `Observe`/`Query` read state, `Spawn` launches a detached `ralphy`
/// child, `Mutate` writes daemon-owned state. Only `Spawn` is constructed today;
/// the other variants are declared so the registry is the full model and adding a
/// consumer later touches no enum. Public + reachable, so `dead_code` stays quiet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EffectClass {
    Native,
    Observe,
    Query,
    Spawn,
    Mutate,
    /// A byte-op on the confined workspace (ADR-0036 Write amendment): save,
    /// create, rename, delete. Runs in-daemon (`crate::fswrite`), never spawns,
    /// and does NOT consult the run lock.
    Write,
}

/// The branch mode a run modal offers (ADR-0036 §1): a closed daemon-local enum
/// so the modal's choice reaches `--branch-mode` without a free-text value ever
/// touching the argv.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BranchMode {
    New,
    Current,
}

impl BranchMode {
    /// Parse the `branchMode` payload value; anything outside `new`/`current`
    /// yields `None` so [`spawn_argv`] refuses the run.
    pub fn from_query(value: &str) -> Option<BranchMode> {
        match value {
            "new" => Some(BranchMode::New),
            "current" => Some(BranchMode::Current),
            _ => None,
        }
    }

    /// The `--branch-mode` flag value.
    fn as_flag(self) -> &'static str {
        match self {
            BranchMode::New => "new",
            BranchMode::Current => "current",
        }
    }
}

/// The `--agent`/`--plan-agent` CLI flag value for an [`Agent`]. Owned here rather
/// than widening `session::Agent`'s public API (`program_name` is private and
/// PATH-named); the CLI-flag mapping belongs to the dispatch registry.
pub(crate) fn agent_flag(a: Agent) -> &'static str {
    match a {
        Agent::Claude => "claude",
        Agent::Codex => "codex",
        Agent::Copilot => "copilot",
        Agent::Cursor => "cursor",
        Agent::Gemini => "gemini",
        Agent::Kimi => "kimi",
        Agent::OpenCode => "opencode",
    }
}

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

/// The closed set of remotely-triggerable verbs. Forge-neutral Ralphy vocabulary
/// (never gh/GitHub terms): `push` names the queue-snapshot verb. Anything else —
/// including `kill`/`stop` — is unrepresentable, so no destructive verb can reach
/// a spawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verb {
    /// Start a run, absorbing an overlap into a clean skip (`run --if-idle`).
    Run,
    /// Triage the queue non-interactively (`triage --if-idle --yes`).
    Triage,
    /// Push the queue snapshot (`issues --push`).
    PushQueue,
    /// List one directory level of a repo (Observe: reads state, never spawns).
    TreeList,
    /// Read a repo file's text (Observe: reads state, never spawns).
    FileRead,
    /// Read a repo file's bytes as an allowlisted image (Observe: reads state,
    /// never spawns — ADR-0049).
    ImageRead,
    /// List the repo's live runs from the snapshot directory (Observe: reads
    /// state, never spawns — ADR-0047 §9).
    RunsList,
    /// Read the repo's resolved config as JSON (Query: `config get --json`).
    ConfigGet,
    /// Read the whole-tracker Kanban board fold (Query: `issues --format json
    /// --board`).
    BoardList,
    /// Read one issue's detail — body, comments, blockers (Query: `issues show
    /// <n> --format json`).
    IssueShow,
    /// Persist a config key (Mutate: `config set`, run-lock-aware).
    ConfigSet,
    /// Clear a config key (Mutate: `config unset`, run-lock-aware).
    ConfigUnset,
    /// Write bytes to a repo file (Write: in-daemon, never spawns).
    FileWrite,
    /// Create a repo file or directory (Write: in-daemon, never spawns).
    FileCreate,
    /// Rename a repo path (Write: in-daemon, never spawns).
    FileRename,
    /// Delete a repo path (Write: in-daemon, never spawns).
    FileDelete,
    /// List the repo's local branches (Query: `branch list --format json`).
    BranchList,
    /// Check out a branch (Mutate: `branch switch -- <name>`, run-lock-aware).
    BranchSwitch,
    /// Create a branch from HEAD (Mutate: `branch create -- <name>`, run-lock-aware).
    BranchCreate,
    /// Add/remove a label on an issue (Mutate: `label set <n> --{op}=<label>`).
    LabelSet,
    /// List the repo's working-tree changes (Query: `changes list --format json`).
    ChangesList,
    /// Read a file's content at a revision (Query: `blob read --revision head
    /// --path <p> --format json`) — the diff tab's original side.
    BlobRead,
    /// Read the branch's upstream state (Query: `sync status --format json`) —
    /// never a network call.
    SyncStatus,
    /// Fetch the branch's remote (Mutate: `sync fetch`, run-lock-aware) — the
    /// operator's own act, never a timer's.
    SyncFetch,
    /// Fast-forward from the upstream (Mutate: `sync pull`, run-lock-aware).
    SyncPull,
    /// Publish the current branch (Mutate: `sync push`, run-lock-aware) — the
    /// OPERATOR's own click, never a run's (ADR-0046 amendment, issue #320).
    SyncPush,
    /// Add paths to the index (Mutate: `changes stage --path=<p>…`,
    /// run-lock-aware).
    ChangesStage,
    /// Remove paths from the index (Mutate: `changes unstage --path=<p>…`,
    /// run-lock-aware).
    ChangesUnstage,
    /// Record the staged index (Mutate: `changes commit --message=<msg>`,
    /// run-lock-aware).
    ChangesCommit,
    /// Discard paths' working-tree changes (Mutate: `changes discard
    /// --path=<p>…`, run-lock-aware).
    ChangesDiscard,
}

impl Verb {
    /// Parse a remote verb string. `run`/`triage`/`push` map to Spawn verbs and
    /// `tree.list`/`file.read` to Observe verbs; every other string — `kill`,
    /// `stop`, `issues`, `""` — yields `None`, so the handler can reject it.
    pub fn from_query(value: &str) -> Option<Verb> {
        match value {
            "run" => Some(Verb::Run),
            "triage" => Some(Verb::Triage),
            "push" => Some(Verb::PushQueue),
            "tree.list" => Some(Verb::TreeList),
            "file.read" => Some(Verb::FileRead),
            "file.image" => Some(Verb::ImageRead),
            "runs.list" => Some(Verb::RunsList),
            "config.get" => Some(Verb::ConfigGet),
            "board.list" => Some(Verb::BoardList),
            "issue.show" => Some(Verb::IssueShow),
            "config.set" => Some(Verb::ConfigSet),
            "config.unset" => Some(Verb::ConfigUnset),
            "file.write" => Some(Verb::FileWrite),
            "file.create" => Some(Verb::FileCreate),
            "file.rename" => Some(Verb::FileRename),
            "file.delete" => Some(Verb::FileDelete),
            "branch.list" => Some(Verb::BranchList),
            "branch.switch" => Some(Verb::BranchSwitch),
            "branch.create" => Some(Verb::BranchCreate),
            "label.set" => Some(Verb::LabelSet),
            "changes.list" => Some(Verb::ChangesList),
            "blob.read" => Some(Verb::BlobRead),
            "sync.status" => Some(Verb::SyncStatus),
            "sync.fetch" => Some(Verb::SyncFetch),
            "sync.pull" => Some(Verb::SyncPull),
            "sync.push" => Some(Verb::SyncPush),
            "changes.stage" => Some(Verb::ChangesStage),
            "changes.unstage" => Some(Verb::ChangesUnstage),
            "changes.commit" => Some(Verb::ChangesCommit),
            "changes.discard" => Some(Verb::ChangesDiscard),
            _ => None,
        }
    }

    /// Every verb in the registry, for exhaustive round-trips.
    pub const ALL: &'static [Verb] = &[
        Verb::Run,
        Verb::Triage,
        Verb::PushQueue,
        Verb::TreeList,
        Verb::FileRead,
        Verb::ImageRead,
        Verb::RunsList,
        Verb::ConfigGet,
        Verb::BoardList,
        Verb::IssueShow,
        Verb::ConfigSet,
        Verb::ConfigUnset,
        Verb::FileWrite,
        Verb::FileCreate,
        Verb::FileRename,
        Verb::FileDelete,
        Verb::BranchList,
        Verb::BranchSwitch,
        Verb::BranchCreate,
        Verb::LabelSet,
        Verb::ChangesList,
        Verb::BlobRead,
        Verb::SyncStatus,
        Verb::SyncFetch,
        Verb::SyncPull,
        Verb::SyncPush,
        Verb::ChangesStage,
        Verb::ChangesUnstage,
        Verb::ChangesCommit,
        Verb::ChangesDiscard,
    ];

    /// The effect class of this verb (ADR-0036 §2): the Observe read verbs read
    /// state in-daemon and never spawn; `config.get` is a Query (spawn-and-collect
    /// `config get --json`); `config.set`/`config.unset` are Mutate; the three run
    /// verbs reach the CLI through [`spawn_argv`].
    pub fn effect_class(self) -> EffectClass {
        match self {
            Verb::TreeList | Verb::FileRead | Verb::ImageRead | Verb::RunsList => {
                EffectClass::Observe
            }
            Verb::ConfigGet
            | Verb::BoardList
            | Verb::IssueShow
            | Verb::BranchList
            | Verb::ChangesList
            | Verb::BlobRead
            | Verb::SyncStatus => EffectClass::Query,
            Verb::ConfigSet
            | Verb::ConfigUnset
            | Verb::BranchSwitch
            | Verb::BranchCreate
            | Verb::LabelSet
            | Verb::SyncFetch
            | Verb::SyncPull
            | Verb::SyncPush
            | Verb::ChangesStage
            | Verb::ChangesUnstage
            | Verb::ChangesCommit
            | Verb::ChangesDiscard => EffectClass::Mutate,
            Verb::FileWrite | Verb::FileCreate | Verb::FileRename | Verb::FileDelete => {
                EffectClass::Write
            }
            Verb::Run | Verb::Triage | Verb::PushQueue => EffectClass::Spawn,
        }
    }
}

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
        | Verb::FileDelete
        | Verb::BranchList
        | Verb::BranchSwitch
        | Verb::BranchCreate
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
        | Verb::ChangesDiscard => Err(ArgvError::BadParam("verb")),
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

/// Compose the argv for a branch Mutate verb: `branch switch -- <name>` /
/// `branch create -- <name>` (issue #199). `<name>` is the sole client input,
/// read from `payload.name`; empty/whitespace-only names yield [`ArgvError`] and
/// NO argv. The `--` guard ends option parsing so a name is never mis-parsed as a
/// flag (mirrors [`config_argv`]'s guard).
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
        .filter(|n| !n.is_empty())
        .ok_or(ArgvError::BadParam("name"))?;
    Ok(vec![
        "branch".to_string(),
        sub.to_string(),
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
const EXEC_ADJACENT_KEYS: [&str; 1] = ["verify.command"];

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

/// Spawn a Query/Mutate child and COLLECT its output to EOF, returning its exit
/// code and stdout+stderr bytes verbatim (distinct from the streaming Spawn path:
/// a Query/Mutate answer is a single collected reply, not a live stream). Blocking
/// (`wait` + a full read); the `command_ws` caller runs it in `spawn_blocking`.
pub fn collect(
    spawner: &dyn Spawner,
    program: &OsStr,
    argv: &[&str],
    cwd: &Path,
    daemon_id: Option<&str>,
) -> Result<(Option<i32>, Vec<u8>)> {
    use std::io::Read;
    let mut child = spawner.spawn(program, argv, cwd, daemon_id)?;
    let mut bytes = Vec::new();
    if let Some(mut reader) = child.take_output() {
        reader.read_to_end(&mut bytes)?;
    }
    let code = child.wait()?;
    Ok((code, bytes))
}

/// A spawned child the dispatcher can await but NEVER kill (see the module
/// teardown invariant). `wait` blocks until the child exits and yields its exit
/// code (`None` when terminated by a signal with no code).
pub trait Child: Send {
    /// The OS process id, when known.
    fn pid(&self) -> Option<u32>;
    /// Block until the child exits; yield its exit code.
    fn wait(&mut self) -> Result<Option<i32>>;
    /// Take the child's merged stdout+stderr reader, ONCE. Yields `Some(reader)`
    /// on the first call and `None` thereafter — the caller owns the reader and
    /// must drain it to EOF (see the module OUTPUT STREAMING note). `wait` no
    /// longer owns the reader, so the drain and the wait can proceed concurrently.
    fn take_output(&mut self) -> Option<Box<dyn std::io::Read + Send>>;
}

/// Spawns a blessed child. The seam that keeps [`dispatch`] testable: production
/// uses [`ProcessSpawner`]; tests use a fake that records the argv.
pub trait Spawner: Send + Sync + 'static {
    /// Spawn `program` with `args` in `cwd`, detached from the daemon's lifecycle.
    /// When `daemon_id` is `Some`, inject it as `RALPHY_DAEMON_ID` on the child so
    /// its emitter carries the daemon identity (dispatch path only).
    fn spawn(
        &self,
        program: &OsStr,
        args: &[&str],
        cwd: &Path,
        daemon_id: Option<&str>,
    ) -> Result<Box<dyn Child>>;
}

/// Spawn a blessed child: `program` (the resolved `ralphy` exe) with `argv`, in
/// `cwd`. `argv` is composed by [`spawn_argv`] from the verb + closed-enum params
/// — never client free-text — and `program` is a real exe run without a shell.
pub fn dispatch(
    spawner: &dyn Spawner,
    program: &OsStr,
    argv: &[&str],
    cwd: &Path,
    daemon_id: Option<&str>,
) -> Result<Box<dyn Child>> {
    spawner.spawn(program, argv, cwd, daemon_id)
}

/// Test seam pointing the dispatcher at a stand-in exe: `RALPHY_EXE_OVERRIDE`
/// when set (an integration test's `command_test_child`), else the daemon's own
/// `current_exe` — which IS `ralphy` in production. Mirrors `session`'s
/// `RALPHY_DAEMON_AGENT_OVERRIDE`.
pub fn ralphy_exe() -> OsString {
    if let Some(over) = std::env::var_os("RALPHY_EXE_OVERRIDE") {
        return over;
    }
    std::env::current_exe()
        .map(Into::into)
        .unwrap_or_else(|_| OsString::from("ralphy"))
}

/// The production spawner: a real detached OS process with null stdio.
pub struct ProcessSpawner;

impl Spawner for ProcessSpawner {
    fn spawn(
        &self,
        program: &OsStr,
        args: &[&str],
        cwd: &Path,
        daemon_id: Option<&str>,
    ) -> Result<Box<dyn Child>> {
        use std::process::{Command, Stdio};
        let mut cmd = Command::new(program);
        // Merge stdout+stderr into ONE pipe so the handler streams a single
        // ordered log (issue #180). The child ignores SIGPIPE, so a daemon crash
        // that drops the reader gives it a non-fatal broken-pipe write, never a
        // kill — but a LIVE daemon must drain the reader to EOF or the pipe fills
        // and stalls the child (the detached drain task in `command_ws` does).
        let (reader, writer) = std::io::pipe()?;
        let writer2 = writer.try_clone()?;
        cmd.args(args)
            .current_dir(cwd)
            // Null stdin (no console); piped stdout+stderr for live streaming.
            .stdin(Stdio::null())
            .stdout(Stdio::from(writer))
            .stderr(Stdio::from(writer2));
        // Cross-process wire contract read by ralphy-cli `emitter::DAEMON_ID_ENV`:
        // identity, NOT a credential. Set per-child here (the dispatch path) rather
        // than process-globally, so console/session children — which never get this
        // injection — truthfully lack it (a `ralphy run` typed in a free console).
        if let Some(id) = daemon_id {
            cmd.env("RALPHY_DAEMON_ID", id);
        }
        // Detach so daemon shutdown never reaches the run. On Unix its own process
        // group isolates it from the daemon's group.
        ralphy_proc_util::own_process_group(&mut cmd);
        // On Windows `own_process_group` is a no-op, so a same-console child
        // would receive the daemon's console control events. Two flags detach it:
        // CREATE_NEW_PROCESS_GROUP (0x200) stops CTRL_C/CTRL_BREAK reaching it,
        // and DETACHED_PROCESS (0x8) gives it no console at all — otherwise a
        // CTRL_CLOSE/LOGOFF/SHUTDOWN event, delivered to every process on the
        // daemon's console regardless of group, would still kill the run. The
        // child is null-stdio, so it needs no console. Together they honor the
        // "spawned runs keep their own lifecycle" invariant on Windows.
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0000_0208);
        }
        let child = cmd.spawn()?;
        Ok(Box::new(ProcessChild {
            child,
            output: Some(reader),
        }))
    }
}

/// A real OS child. `wait`-only: no kill method exists, and dropping it does not
/// kill (std semantics) — the dispatched run outlives the daemon. `output` holds
/// the merged stdout+stderr reader until [`Child::take_output`] hands it off.
struct ProcessChild {
    child: std::process::Child,
    output: Option<std::io::PipeReader>,
}

impl Child for ProcessChild {
    fn pid(&self) -> Option<u32> {
        Some(self.child.id())
    }

    fn wait(&mut self) -> Result<Option<i32>> {
        Ok(self.child.wait()?.code())
    }

    fn take_output(&mut self) -> Option<Box<dyn std::io::Read + Send>> {
        self.output.take().map(|r| Box::new(r) as _)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::Mutex;

    /// Records what `dispatch` asked to spawn and hands back a child with a preset
    /// exit code — no OS process touched, so the argv mapping is asserted purely.
    /// One recorded spawn: (program, argv, cwd, daemon_id).
    type SpawnCall = (OsString, Vec<String>, std::path::PathBuf, Option<String>);

    #[derive(Default)]
    struct FakeSpawner {
        calls: Mutex<Vec<SpawnCall>>,
    }

    #[derive(Default)]
    struct FakeChild {
        code: i32,
        output: Option<Vec<u8>>,
    }

    impl Child for FakeChild {
        fn pid(&self) -> Option<u32> {
            Some(4242)
        }
        fn wait(&mut self) -> Result<Option<i32>> {
            Ok(Some(self.code))
        }
        fn take_output(&mut self) -> Option<Box<dyn std::io::Read + Send>> {
            self.output
                .take()
                .map(|b| Box::new(std::io::Cursor::new(b)) as _)
        }
    }

    impl Spawner for FakeSpawner {
        fn spawn(
            &self,
            program: &OsStr,
            args: &[&str],
            cwd: &Path,
            daemon_id: Option<&str>,
        ) -> Result<Box<dyn Child>> {
            self.calls.lock().unwrap().push((
                program.to_os_string(),
                args.iter().map(|a| a.to_string()).collect(),
                cwd.to_path_buf(),
                daemon_id.map(str::to_owned),
            ));
            Ok(Box::new(FakeChild {
                code: 7,
                output: None,
            }))
        }
    }

    #[test]
    fn take_output_yields_child_bytes() {
        use std::io::Read;
        let mut child = FakeChild {
            code: 0,
            output: Some(b"hello-output".to_vec()),
        };
        let mut reader = child.take_output().expect("first take yields the reader");
        let mut buf = Vec::new();
        reader.read_to_end(&mut buf).unwrap();
        assert_eq!(buf, b"hello-output", "the reader yields the child's bytes");
        assert!(
            child.take_output().is_none(),
            "a second take_output yields None"
        );
    }

    #[test]
    fn verb_effect_classes() {
        assert_eq!(Verb::TreeList.effect_class(), EffectClass::Observe);
        assert_eq!(Verb::FileRead.effect_class(), EffectClass::Observe);
        assert_eq!(Verb::ImageRead.effect_class(), EffectClass::Observe);
        assert_eq!(Verb::from_query("file.image"), Some(Verb::ImageRead));
        assert_eq!(Verb::RunsList.effect_class(), EffectClass::Observe);
        assert_eq!(Verb::from_query("runs.list"), Some(Verb::RunsList));
        assert_eq!(Verb::Run.effect_class(), EffectClass::Spawn);
        assert_eq!(Verb::Triage.effect_class(), EffectClass::Spawn);
        assert_eq!(Verb::PushQueue.effect_class(), EffectClass::Spawn);
        assert_eq!(Verb::ConfigGet.effect_class(), EffectClass::Query);
        assert_eq!(Verb::BoardList.effect_class(), EffectClass::Query);
        assert_eq!(Verb::IssueShow.effect_class(), EffectClass::Query);
        assert_eq!(Verb::ConfigSet.effect_class(), EffectClass::Mutate);
        assert_eq!(Verb::ConfigUnset.effect_class(), EffectClass::Mutate);
        assert_eq!(Verb::FileWrite.effect_class(), EffectClass::Write);
        assert_eq!(Verb::FileCreate.effect_class(), EffectClass::Write);
        assert_eq!(Verb::FileRename.effect_class(), EffectClass::Write);
        assert_eq!(Verb::FileDelete.effect_class(), EffectClass::Write);
        assert_eq!(Verb::BranchList.effect_class(), EffectClass::Query);
        assert_eq!(Verb::BranchSwitch.effect_class(), EffectClass::Mutate);
        assert_eq!(Verb::BranchCreate.effect_class(), EffectClass::Mutate);
        assert_eq!(Verb::LabelSet.effect_class(), EffectClass::Mutate);
        assert_eq!(Verb::SyncStatus.effect_class(), EffectClass::Query);
        assert_eq!(Verb::SyncFetch.effect_class(), EffectClass::Mutate);
        assert_eq!(Verb::SyncPull.effect_class(), EffectClass::Mutate);
        assert_eq!(Verb::SyncPush.effect_class(), EffectClass::Mutate);
        assert_eq!(Verb::ChangesStage.effect_class(), EffectClass::Mutate);
        assert_eq!(Verb::ChangesUnstage.effect_class(), EffectClass::Mutate);
        assert_eq!(Verb::ChangesCommit.effect_class(), EffectClass::Mutate);
        assert_eq!(Verb::ChangesDiscard.effect_class(), EffectClass::Mutate);
        assert_eq!(
            Verb::ALL.len(),
            30,
            "the registry holds exactly thirty verbs"
        );
    }

    /// The shared shape gate as its OWN unit, exercised directly rather than
    /// through a builder — and with NO `#[cfg]` anywhere in it, which is what
    /// makes "a Windows daemon refuses a POSIX-absolute path exactly as a Linux
    /// one refuses a drive prefix" a test rather than a claim.
    #[test]
    fn validated_path_refuses_every_bad_shape_on_every_platform() {
        for bad in [
            "/etc/passwd",
            "C:\\Windows\\x",
            "D:/x",
            "../../secret",
            "src/../../secret",
            "-rf",
            "",
            // Pathspec MAGIC, which is a leading-`:` shape, not a glob char.
            ":(glob)x",
            ":(top)etc/passwd",
            ":!x",
        ] {
            assert_eq!(
                validated_path(bad),
                None,
                "{bad:?} must be refused on every platform"
            );
        }

        // Glob metacharacters are LEGAL FILENAMES and must survive: refusing
        // them made `app/blog/[slug]/page.tsx` unreadable in the diff tab and
        // poisoned every group action over such a repo. `--literal-pathspecs`
        // plus change-set membership are what stop a pattern from expanding.
        for legal in [
            "app/blog/[slug]/page.tsx",
            "[Content_Types].xml",
            "src/*.rs",
            "a?.txt",
            "*",
            "**/x",
        ] {
            assert_eq!(
                validated_path(legal).as_deref(),
                Some(legal),
                "{legal:?} is a legal filename and must pass through unchanged"
            );
        }

        // The positive control: a real path survives, and a Windows-shaped one
        // normalises to the SAME string rather than being a second spelling.
        assert_eq!(
            validated_path("src/main.rs").as_deref(),
            Some("src/main.rs")
        );
        assert_eq!(
            validated_path("src\\main.rs").as_deref(),
            Some("src/main.rs")
        );
        assert_eq!(
            validated_path("a file.txt").as_deref(),
            Some("a file.txt"),
            "a space is not a shape problem"
        );
        // `..` is refused as a SEGMENT, not as a substring — a real filename
        // containing dots stays readable.
        assert_eq!(validated_path("v..1/x").as_deref(), Some("v..1/x"));
    }

    /// The discard verb rides the SHARED builder — no second definition of what
    /// a path is — so this pins its exact vector and that a malformed path
    /// yields no argv at all.
    #[test]
    fn changes_discard_argv_composes_an_exact_vector() {
        assert_eq!(
            Verb::from_query("changes.discard"),
            Some(Verb::ChangesDiscard)
        );
        assert_eq!(
            changes_paths_argv(
                Verb::ChangesDiscard,
                &json!({ "paths": ["a.txt", "b/c.txt"] })
            )
            .unwrap(),
            vec!["changes", "discard", "--path=a.txt", "--path=b/c.txt"]
        );
        // A Windows-shaped path composes the same token as its POSIX spelling.
        assert_eq!(
            changes_paths_argv(Verb::ChangesDiscard, &json!({ "paths": ["b\\c.txt"] })).unwrap(),
            vec!["changes", "discard", "--path=b/c.txt"]
        );
        // The untracked-directory entry shape git itself reports.
        assert_eq!(
            changes_paths_argv(Verb::ChangesDiscard, &json!({ "paths": ["newdir/"] })).unwrap(),
            vec!["changes", "discard", "--path=newdir/"]
        );
    }

    #[test]
    fn changes_discard_refuses_a_malformed_path_with_no_argv() {
        for bad in [
            "/etc/passwd",
            "C:\\x",
            "../secret",
            "-rf",
            ":(glob)x",
            ":!x",
            "",
        ] {
            assert_eq!(
                changes_paths_argv(Verb::ChangesDiscard, &json!({ "paths": [bad] })),
                Err(ArgvError::BadParam("paths")),
                "{bad:?} must yield NO argv"
            );
        }
        for empty in [
            json!({}),
            json!({ "paths": [] }),
            json!({ "paths": "a.txt" }),
        ] {
            assert_eq!(
                changes_paths_argv(Verb::ChangesDiscard, &empty),
                Err(ArgvError::BadParam("paths")),
                "{empty} must yield NO argv"
            );
        }
        // One bad element poisons the whole list — a partial argv would discard
        // the good half of a request the daemon refused, unrecoverably.
        assert_eq!(
            changes_paths_argv(
                Verb::ChangesDiscard,
                &json!({ "paths": ["ok.txt", "/etc/passwd"] })
            ),
            Err(ArgvError::BadParam("paths"))
        );
        let over: Vec<String> = (0..MAX_STAGE_PATHS + 1)
            .map(|i| format!("f{i}.txt"))
            .collect();
        assert_eq!(
            changes_paths_argv(Verb::ChangesDiscard, &json!({ "paths": over })),
            Err(ArgvError::BadParam("paths")),
            "257 paths is over MAX_STAGE_PATHS"
        );
    }

    #[test]
    fn changes_mutate_argv_composes_exact_vectors_and_refuses() {
        assert_eq!(Verb::from_query("changes.stage"), Some(Verb::ChangesStage));
        assert_eq!(
            Verb::from_query("changes.unstage"),
            Some(Verb::ChangesUnstage)
        );
        assert_eq!(
            Verb::from_query("changes.commit"),
            Some(Verb::ChangesCommit)
        );

        assert_eq!(
            changes_paths_argv(
                Verb::ChangesStage,
                &json!({ "paths": ["a.txt", "b/c.txt"] })
            )
            .unwrap(),
            vec!["changes", "stage", "--path=a.txt", "--path=b/c.txt"]
        );
        assert_eq!(
            changes_paths_argv(
                Verb::ChangesUnstage,
                &json!({ "paths": ["a.txt", "b/c.txt"] })
            )
            .unwrap(),
            vec!["changes", "unstage", "--path=a.txt", "--path=b/c.txt"]
        );
        // A Windows-shaped path composes the SAME token as its POSIX spelling.
        assert_eq!(
            changes_paths_argv(Verb::ChangesStage, &json!({ "paths": ["b\\c.txt"] })).unwrap(),
            vec!["changes", "stage", "--path=b/c.txt"]
        );

        let commit = changes_commit_argv(&json!({ "message": "-oops" })).unwrap();
        assert_eq!(commit, vec!["changes", "commit", "--message=-oops"]);
        assert_eq!(
            commit.len(),
            3,
            "the message is ONE token — a split one dies in clap"
        );

        // Refusals, each with NO argv.
        for bad in ["/etc/passwd", "C:\\x", "../x", "-rf", ":(glob)x", ":!x", ""] {
            assert_eq!(
                changes_paths_argv(Verb::ChangesStage, &json!({ "paths": [bad] })),
                Err(ArgvError::BadParam("paths")),
                "{bad:?} must yield NO argv"
            );
        }
        // …while a bracketed route file — the Next.js convention — composes its
        // token unchanged. `--literal-pathspecs` is what keeps git from
        // expanding it, not a refusal here.
        assert_eq!(
            changes_paths_argv(
                Verb::ChangesStage,
                &json!({ "paths": ["app/blog/[slug]/page.tsx"] })
            )
            .unwrap(),
            vec!["changes", "stage", "--path=app/blog/[slug]/page.tsx"]
        );
        assert!(
            blob_read_argv(&json!({ "revision": "head", "path": "app/blog/[slug]/page.tsx" }))
                .is_ok(),
            "the diff tab must still read a bracketed route file"
        );
        // One bad element poisons the whole list — a partial argv would stage
        // the good half of a request the daemon refused.
        assert_eq!(
            changes_paths_argv(
                Verb::ChangesStage,
                &json!({ "paths": ["ok.txt", "/etc/passwd"] })
            ),
            Err(ArgvError::BadParam("paths"))
        );
        for empty in [
            json!({}),
            json!({ "paths": [] }),
            json!({ "paths": "a.txt" }),
        ] {
            assert_eq!(
                changes_paths_argv(Verb::ChangesStage, &empty),
                Err(ArgvError::BadParam("paths")),
                "{empty} must yield NO argv"
            );
        }
        // A single over-long path is refused too — the count bound alone does
        // not bound the command line.
        assert_eq!(
            changes_paths_argv(
                Verb::ChangesStage,
                &json!({ "paths": ["x".repeat(MAX_PATH_CHARS + 1)] })
            ),
            Err(ArgvError::BadParam("paths"))
        );
        assert!(
            changes_paths_argv(
                Verb::ChangesStage,
                &json!({ "paths": ["x".repeat(MAX_PATH_CHARS)] })
            )
            .is_ok(),
            "the bound is the bound: a path of exactly MAX_PATH_CHARS is accepted"
        );
        let over: Vec<String> = (0..257).map(|i| format!("f{i}.txt")).collect();
        assert_eq!(
            changes_paths_argv(Verb::ChangesStage, &json!({ "paths": over })),
            Err(ArgvError::BadParam("paths")),
            "257 paths is over MAX_STAGE_PATHS"
        );
        let at_bound: Vec<String> = (0..MAX_STAGE_PATHS).map(|i| format!("f{i}.txt")).collect();
        assert_eq!(
            changes_paths_argv(Verb::ChangesStage, &json!({ "paths": at_bound }))
                .unwrap()
                .len(),
            MAX_STAGE_PATHS + 2,
            "the bound is the bound: 256 paths are accepted"
        );

        for bad in [
            json!({}),
            json!({ "message": "" }),
            json!({ "message": "   " }),
        ] {
            assert_eq!(
                changes_commit_argv(&bad),
                Err(ArgvError::BadParam("message")),
                "{bad} must yield NO argv"
            );
        }
        assert_eq!(
            changes_commit_argv(&json!({ "message": "x".repeat(MAX_COMMIT_MESSAGE + 1) })),
            Err(ArgvError::BadParam("message")),
            "4097 chars is over MAX_COMMIT_MESSAGE"
        );
        assert!(
            changes_commit_argv(&json!({ "message": "x".repeat(MAX_COMMIT_MESSAGE) })).is_ok(),
            "the bound is the bound: 4096 chars are accepted"
        );

        // The verb is a parameter too, and a wrong one yields no argv.
        assert_eq!(
            changes_paths_argv(Verb::ChangesCommit, &json!({ "paths": ["a.txt"] })),
            Err(ArgvError::BadParam("verb"))
        );
        assert_eq!(
            changes_paths_argv(Verb::Run, &json!({ "paths": ["a.txt"] })),
            Err(ArgvError::BadParam("verb"))
        );
        for verb in [
            Verb::ChangesStage,
            Verb::ChangesUnstage,
            Verb::ChangesCommit,
            Verb::ChangesDiscard,
        ] {
            assert_eq!(
                spawn_argv(verb, &json!({})),
                Err(ArgvError::BadParam("verb")),
                "a Mutate verb never reaches the spawn path"
            );
        }

        // The SAME bad vector is refused by the read-side builder too — the one
        // shared gate, exercised through both callers.
        assert_eq!(
            blob_read_argv(&json!({ "revision": "head", "path": ":(glob)x" })),
            Err(ArgvError::BadParam("path"))
        );
    }

    #[test]
    fn sync_argv_composes_exact_vectors_and_refuses() {
        assert_eq!(Verb::from_query("sync.status"), Some(Verb::SyncStatus));
        assert_eq!(Verb::from_query("sync.fetch"), Some(Verb::SyncFetch));
        assert_eq!(Verb::from_query("sync.pull"), Some(Verb::SyncPull));
        assert_eq!(Verb::from_query("sync.push"), Some(Verb::SyncPush));

        assert_eq!(
            sync_status_argv(),
            vec!["sync", "status", "--format", "json"]
        );
        assert_eq!(sync_argv(Verb::SyncFetch).unwrap(), vec!["sync", "fetch"]);
        assert_eq!(sync_argv(Verb::SyncPull).unwrap(), vec!["sync", "pull"]);
        assert_eq!(sync_argv(Verb::SyncPush).unwrap(), vec!["sync", "push"]);

        // The verbs carry no client input, so the verb itself is the only
        // parameter there is to malform — and a bad one yields NO argv.
        assert_eq!(sync_argv(Verb::Run), Err(ArgvError::BadParam("verb")));
        assert_eq!(
            sync_argv(Verb::ChangesList),
            Err(ArgvError::BadParam("verb"))
        );
        assert_eq!(
            spawn_argv(Verb::SyncFetch, &serde_json::json!({})),
            Err(ArgvError::BadParam("verb")),
            "a Mutate verb never reaches the spawn path"
        );
        assert_eq!(
            spawn_argv(Verb::SyncStatus, &serde_json::json!({})),
            Err(ArgvError::BadParam("verb"))
        );
    }

    #[test]
    fn blob_read_argv_composes_exact_vector_and_refuses() {
        assert_eq!(Verb::from_query("blob.read"), Some(Verb::BlobRead));
        assert_eq!(Verb::BlobRead.effect_class(), EffectClass::Query);

        let expected: Vec<String> = [
            "blob",
            "read",
            "--revision",
            "head",
            "--path",
            "src/main.rs",
            "--format",
            "json",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();

        assert_eq!(
            blob_read_argv(&json!({ "revision": "head", "path": "src/main.rs" })).unwrap(),
            expected
        );
        // A Windows-shaped path from the browser composes the SAME vector: the
        // separator is normalised, not a second accepted spelling.
        assert_eq!(
            blob_read_argv(&json!({ "revision": "head", "path": "src\\main.rs" })).unwrap(),
            expected
        );

        for bad in [
            "/etc/passwd",
            "C:\\Windows\\x",
            "../../secret",
            "-rf",
            "",
            "src/../../secret",
        ] {
            assert_eq!(
                blob_read_argv(&json!({ "revision": "head", "path": bad })),
                Err(ArgvError::BadParam("path")),
                "{bad:?} must yield NO argv"
            );
        }
        assert_eq!(
            blob_read_argv(&json!({ "revision": "head" })),
            Err(ArgvError::BadParam("path")),
            "a missing path yields no argv"
        );

        // The revision is a closed enum: HEAD is the only side this slice diffs against.
        assert_eq!(
            blob_read_argv(&json!({ "revision": "work", "path": "a" })),
            Err(ArgvError::BadParam("revision"))
        );
        assert_eq!(
            blob_read_argv(&json!({ "path": "a" })),
            Err(ArgvError::BadParam("revision"))
        );
    }

    #[test]
    fn from_query_maps_config_verbs() {
        assert_eq!(Verb::from_query("config.get"), Some(Verb::ConfigGet));
        assert_eq!(Verb::from_query("board.list"), Some(Verb::BoardList));
        assert_eq!(Verb::from_query("issue.show"), Some(Verb::IssueShow));
        assert_eq!(Verb::from_query("config.set"), Some(Verb::ConfigSet));
        assert_eq!(Verb::from_query("config.unset"), Some(Verb::ConfigUnset));
        assert_eq!(Verb::from_query("file.write"), Some(Verb::FileWrite));
        assert_eq!(Verb::from_query("file.create"), Some(Verb::FileCreate));
        assert_eq!(Verb::from_query("file.rename"), Some(Verb::FileRename));
        assert_eq!(Verb::from_query("file.delete"), Some(Verb::FileDelete));
        assert_eq!(Verb::from_query("branch.list"), Some(Verb::BranchList));
        assert_eq!(Verb::from_query("branch.switch"), Some(Verb::BranchSwitch));
        assert_eq!(Verb::from_query("branch.create"), Some(Verb::BranchCreate));
        assert_eq!(Verb::from_query("label.set"), Some(Verb::LabelSet));
        assert_eq!(Verb::from_query("blob.read"), Some(Verb::BlobRead));
    }

    #[test]
    fn branch_list_argv_is_static() {
        assert_eq!(
            branch_list_argv(),
            vec!["branch", "list", "--format", "json"],
            "the branch-list verb takes no client input"
        );
    }

    #[test]
    fn changes_list_argv_is_static() {
        assert_eq!(
            changes_list_argv(),
            vec!["changes", "list", "--format", "json"],
            "the changes-list verb takes no client input"
        );
        assert_eq!(Verb::from_query("changes.list"), Some(Verb::ChangesList));
        assert_eq!(Verb::ChangesList.effect_class(), EffectClass::Query);
    }

    #[test]
    fn branch_argv_composes_guarded_vectors() {
        assert_eq!(
            branch_argv(Verb::BranchSwitch, &serde_json::json!({ "name": "feat/x" })).unwrap(),
            vec!["branch", "switch", "--", "feat/x"]
        );
        assert_eq!(
            branch_argv(Verb::BranchCreate, &serde_json::json!({ "name": "feat/x" })).unwrap(),
            vec!["branch", "create", "--", "feat/x"]
        );
        // Empty / whitespace-only / absent name never reaches argv.
        assert_eq!(
            branch_argv(Verb::BranchSwitch, &serde_json::json!({ "name": "" })),
            Err(ArgvError::BadParam("name"))
        );
        assert_eq!(
            branch_argv(Verb::BranchSwitch, &serde_json::json!({ "name": "   " })),
            Err(ArgvError::BadParam("name"))
        );
        assert_eq!(
            branch_argv(Verb::BranchCreate, &serde_json::json!({})),
            Err(ArgvError::BadParam("name"))
        );
    }

    #[test]
    fn label_argv_composes_single_token_op() {
        assert_eq!(
            label_argv(&serde_json::json!({ "number": 7, "label": "AFK", "op": "add" })).unwrap(),
            vec!["label", "set", "7", "--add=AFK"]
        );
        assert_eq!(
            label_argv(&serde_json::json!({ "number": 7, "label": "AFK", "op": "remove" }))
                .unwrap(),
            vec!["label", "set", "7", "--remove=AFK"]
        );
        // Zero/absent number, empty label, and out-of-enum op are all refused.
        assert_eq!(
            label_argv(&serde_json::json!({ "number": 0, "label": "AFK", "op": "add" })),
            Err(ArgvError::BadParam("number"))
        );
        assert_eq!(
            label_argv(&serde_json::json!({ "number": 7, "label": "", "op": "add" })),
            Err(ArgvError::BadParam("label"))
        );
        assert_eq!(
            label_argv(&serde_json::json!({ "number": 7, "label": "AFK", "op": "toggle" })),
            Err(ArgvError::BadParam("op"))
        );
    }

    #[test]
    fn config_argv_composes_exact_vectors() {
        assert_eq!(
            config_argv(Verb::ConfigGet, &serde_json::json!({})).unwrap(),
            vec!["config", "get", "--json"]
        );
        assert_eq!(
            config_argv(
                Verb::ConfigSet,
                &serde_json::json!({ "key": "branch_mode", "value": "new" })
            )
            .unwrap(),
            vec!["config", "set", "--", "branch_mode", "new"]
        );
        assert_eq!(
            config_argv(
                Verb::ConfigUnset,
                &serde_json::json!({ "key": "branch_mode" })
            )
            .unwrap(),
            vec!["config", "unset", "--", "branch_mode"]
        );
        // A dash-leading value is stored, not parsed as a flag (the `--` guard).
        assert_eq!(
            config_argv(
                Verb::ConfigSet,
                &serde_json::json!({ "key": "opencode.model", "value": "--weird" })
            )
            .unwrap(),
            vec!["config", "set", "--", "opencode.model", "--weird"]
        );
    }

    #[test]
    fn config_argv_refuses_exec_adjacent_keys() {
        // `verify.command` is well-shaped and a genuinely supported CLI key, so
        // neither the character class nor the CLI's `require_known_key` refuses
        // it. Its value becomes argv[0] of the verify gate's child, so the daemon
        // refuses the key itself — remotely, only.
        assert_eq!(
            config_argv(
                Verb::ConfigSet,
                &serde_json::json!({ "key": "verify.command", "value": "C:/evil.exe" })
            ),
            Err(ArgvError::BadParam("key"))
        );
        assert_eq!(
            config_argv(
                Verb::ConfigUnset,
                &serde_json::json!({ "key": "verify.command" })
            ),
            Err(ArgvError::BadParam("key"))
        );
        // A neighbouring verify key stays settable — the denylist is one key, not
        // a namespace.
        assert!(config_argv(
            Verb::ConfigSet,
            &serde_json::json!({ "key": "verify.require_verify_gate", "value": "true" })
        )
        .is_ok());
    }

    #[test]
    fn config_argv_refuses_ill_shaped_key_and_empty_value() {
        // An out-of-class or empty key never reaches argv.
        assert_eq!(
            config_argv(
                Verb::ConfigSet,
                &serde_json::json!({ "key": "bad key!", "value": "x" })
            ),
            Err(ArgvError::BadParam("key"))
        );
        assert_eq!(
            config_argv(
                Verb::ConfigSet,
                &serde_json::json!({ "key": "", "value": "x" })
            ),
            Err(ArgvError::BadParam("key"))
        );
        assert_eq!(
            config_argv(Verb::ConfigUnset, &serde_json::json!({ "key": "Bad.Key" })),
            Err(ArgvError::BadParam("key"))
        );
        // An empty/absent value refuses `set`.
        assert_eq!(
            config_argv(
                Verb::ConfigSet,
                &serde_json::json!({ "key": "branch_mode", "value": "" })
            ),
            Err(ArgvError::BadParam("value"))
        );
        assert_eq!(
            config_argv(
                Verb::ConfigSet,
                &serde_json::json!({ "key": "branch_mode" })
            ),
            Err(ArgvError::BadParam("value"))
        );
    }

    #[test]
    fn board_argv_is_static() {
        assert_eq!(
            board_argv(),
            vec!["issues", "--format", "json", "--board"],
            "the board verb takes no client input"
        );
    }

    #[test]
    fn issue_show_argv_validates_number() {
        // A positive integer composes the detail argv.
        assert_eq!(
            issue_show_argv(&serde_json::json!({ "number": 42 })).unwrap(),
            vec!["issues", "show", "42", "--format", "json"]
        );
        // Zero, missing, and non-integer are all refused — no argv reaches the CLI.
        assert_eq!(
            issue_show_argv(&serde_json::json!({ "number": 0 })),
            Err(ArgvError::BadParam("number"))
        );
        assert_eq!(
            issue_show_argv(&serde_json::json!({})),
            Err(ArgvError::BadParam("number"))
        );
        assert_eq!(
            issue_show_argv(&serde_json::json!({ "number": "12" })),
            Err(ArgvError::BadParam("number"))
        );
    }

    #[test]
    fn collect_returns_child_stdout_and_code() {
        // A one-off spawner returns a FakeChild with known bytes + code so
        // `collect` is asserted purely (no OS process touched).
        struct OutSpawner;
        impl Spawner for OutSpawner {
            fn spawn(
                &self,
                _program: &OsStr,
                _args: &[&str],
                _cwd: &Path,
                _daemon_id: Option<&str>,
            ) -> Result<Box<dyn Child>> {
                Ok(Box::new(FakeChild {
                    code: 3,
                    output: Some(b"{\"branch_mode\":\"new\"}".to_vec()),
                }))
            }
        }
        let (code, bytes) = collect(
            &OutSpawner,
            OsStr::new("ralphy"),
            &["config", "get", "--json"],
            Path::new("/work/repo"),
            None,
        )
        .unwrap();
        assert_eq!(code, Some(3), "the fake's exit code comes back");
        assert_eq!(
            bytes, b"{\"branch_mode\":\"new\"}",
            "the child's stdout bytes come back verbatim"
        );
    }

    #[test]
    fn spawn_argv_static_verbs() {
        assert_eq!(
            spawn_argv(Verb::Triage, &serde_json::json!({})).unwrap(),
            vec!["triage", "--if-idle", "--yes"]
        );
        assert_eq!(
            spawn_argv(Verb::PushQueue, &serde_json::json!({})).unwrap(),
            vec!["issues", "--push"]
        );
    }

    #[test]
    fn spawn_argv_run_composes_validated_flags() {
        // Executor-only, new branch.
        assert_eq!(
            spawn_argv(
                Verb::Run,
                &serde_json::json!({ "agent": "claude", "branchMode": "new" })
            )
            .unwrap(),
            vec![
                "run",
                "--if-idle",
                "--agent",
                "claude",
                "--branch-mode",
                "new"
            ]
        );
        // Split planner + current branch.
        assert_eq!(
            spawn_argv(
                Verb::Run,
                &serde_json::json!({
                    "agent": "opencode",
                    "planAgent": "claude",
                    "branchMode": "current"
                })
            )
            .unwrap(),
            vec![
                "run",
                "--if-idle",
                "--agent",
                "opencode",
                "--plan-agent",
                "claude",
                "--branch-mode",
                "current"
            ]
        );
        // A JSON-null planAgent is omitted, not refused (the modal sends null when
        // not split).
        assert_eq!(
            spawn_argv(
                Verb::Run,
                &serde_json::json!({ "agent": "claude", "planAgent": null, "branchMode": "new" })
            )
            .unwrap(),
            vec![
                "run",
                "--if-idle",
                "--agent",
                "claude",
                "--branch-mode",
                "new"
            ]
        );
    }

    #[test]
    fn spawn_argv_carries_copilot_through_to_the_agent_flag() {
        // Copilot's adapter and CLI variant landed in #229; the daemon's own enum is
        // hand-kept in step with them (ADR-0040 Tier 4, issue #238). The flag value
        // must be the CLI's own `--agent copilot`.
        assert_eq!(
            spawn_argv(
                Verb::Run,
                &serde_json::json!({ "agent": "copilot", "branchMode": "new" })
            )
            .unwrap(),
            vec![
                "run",
                "--if-idle",
                "--agent",
                "copilot",
                "--branch-mode",
                "new"
            ]
        );
    }

    #[test]
    fn spawn_argv_carries_kimi_through_to_the_agent_flag() {
        // Kimi was absent from the daemon's enum while its adapter shipped, so a
        // workbench run refused with BadParam("agent") (issue #228). The flag value
        // must be the CLI's own `--agent kimi`.
        assert_eq!(
            spawn_argv(
                Verb::Run,
                &serde_json::json!({ "agent": "kimi", "branchMode": "new" })
            )
            .unwrap(),
            vec![
                "run",
                "--if-idle",
                "--agent",
                "kimi",
                "--branch-mode",
                "new"
            ]
        );
    }

    #[test]
    fn spawn_argv_carries_cursor_through_to_the_agent_flag() {
        // ADR-0042 D1 deferred the daemon on purpose; #248 lifts it. The flag value
        // is the CLI's `--agent cursor`, NOT the binary name `cursor-agent`.
        assert_eq!(
            spawn_argv(
                Verb::Run,
                &serde_json::json!({ "agent": "cursor", "branchMode": "new" })
            )
            .unwrap(),
            vec![
                "run",
                "--if-idle",
                "--agent",
                "cursor",
                "--branch-mode",
                "new"
            ]
        );
    }

    #[test]
    fn spawn_argv_carries_gemini_through_to_the_agent_flag() {
        assert_eq!(
            spawn_argv(
                Verb::Run,
                &serde_json::json!({ "agent": "gemini", "branchMode": "new" })
            )
            .unwrap(),
            vec![
                "run",
                "--if-idle",
                "--agent",
                "gemini",
                "--branch-mode",
                "new"
            ]
        );
    }

    /// The ADR-0040 canary: `from_query` (what the workbench sends IN) and
    /// `agent_flag` (what the CLI receives OUT) are hand-maintained in two places,
    /// so a vendor added to one and not the other silently refuses a launch.
    #[test]
    fn agent_flag_round_trips_through_from_query() {
        for a in Agent::ALL {
            assert_eq!(
                Agent::from_query(agent_flag(a)),
                Some(a),
                "{a:?}'s CLI flag does not parse back through from_query"
            );
        }
    }

    /// The workbench's vendor list used to be hand-maintained in THREE places in
    /// `app.js` and nothing compiled it — Kimi shipped missing from all three
    /// (issue #228). Issue #304 removed the list: the menu renders from
    /// `GET /api/agents`, whose digits an exhaustive match owns (`roster.rs`).
    /// So the pin INVERTED — `app.js` must hold NO vendor enumeration, and the
    /// guarantee that every adapter reaches the menu lives in `roster.rs`.
    #[test]
    fn app_js_holds_no_vendor_list() {
        let js = include_str!("../assets/ui/app.js");

        // Non-vacuous first: the menu really does fetch the daemon's roster.
        assert!(
            js.contains("\"/api/agents\""),
            "app.js must render the menu from the daemon's roster endpoint"
        );

        // The `agents` binding survives (the run dialog's pickers bind it) but it
        // is now filled from the roster, so its literal must be EMPTY. Anchored on
        // the leading space so `planAgents: [` cannot satisfy it, and required to
        // be UNIQUE — a second, populated `agents: [ … ]` further down would
        // otherwise leave this first (empty) one answering for it.
        assert_eq!(
            js.matches(" agents: [").count(),
            1,
            "app.js must declare exactly one `agents: [` literal"
        );
        let start = js
            .find(" agents: [")
            .expect("app.js no longer declares `agents: [`");
        let rest = &js[start + " agents: [".len()..];
        let end = rest
            .find(']')
            .expect("app.js's `agents: [` is never closed");
        assert!(
            rest[..end].trim().is_empty(),
            "app.js's `agents:` must stay an empty literal; found: {:?}",
            &rest[..end]
        );

        // The accelerator map keyed by vendor is gone; digits come from the rows.
        assert!(
            !js.contains("const map = { Digit1"),
            "app.js still maps accelerator digits to vendors"
        );

        // No launchable vendor is named in `app.js` — except `claude`, which
        // survives ONLY as the run dialog's default value (a default naming one
        // vendor is not an enumeration; it is the CLI's own default). Checked
        // QUOTE-AGNOSTICALLY: `app.js` is full of template literals and single
        // quotes, so pinning only `"codex"` would wave `'codex'` and `` `codex` ``
        // straight through — the reintroduced list would look exactly like that.
        for a in Agent::ALL {
            let flag = agent_flag(a);
            if flag == "claude" {
                continue;
            }
            for quote in ['"', '\'', '`'] {
                let quoted = format!("{quote}{flag}{quote}");
                assert!(
                    !js.contains(&quoted),
                    "app.js names {flag} ({quoted}) — the frontend must hold no vendor list"
                );
            }
        }
        let defaults =
            js.matches(r#"agent: "claude""#).count() + js.matches(r#"planAgent: "claude""#).count();
        assert_eq!(
            js.matches("\"claude\"").count(),
            defaults,
            "`\"claude\"` may appear in app.js ONLY as the run dialog's agent/planAgent default"
        );
        assert!(
            !js.contains(r#"kind: "claude""#) && !js.contains(r#"Digit1: "claude""#),
            "app.js still routes a console launch through a hardcoded vendor"
        );
    }

    #[test]
    fn spawn_argv_refuses_out_of_enum_params() {
        // Out-of-enum or free-text (a shell injection attempt) never reaches argv.
        assert_eq!(
            spawn_argv(
                Verb::Run,
                &serde_json::json!({ "agent": "bogus", "branchMode": "new" })
            ),
            Err(ArgvError::BadParam("agent"))
        );
        assert_eq!(
            spawn_argv(
                Verb::Run,
                &serde_json::json!({ "agent": "claude", "branchMode": "sideways" })
            ),
            Err(ArgvError::BadParam("branchMode"))
        );
        assert_eq!(
            spawn_argv(
                Verb::Run,
                &serde_json::json!({ "agent": "claude", "planAgent": "x;rm", "branchMode": "new" })
            ),
            Err(ArgvError::BadParam("planAgent"))
        );
        // Absent required params are refused too.
        assert_eq!(
            spawn_argv(Verb::Run, &serde_json::json!({ "branchMode": "new" })),
            Err(ArgvError::BadParam("agent"))
        );
        assert_eq!(
            spawn_argv(Verb::Run, &serde_json::json!({ "agent": "claude" })),
            Err(ArgvError::BadParam("branchMode"))
        );
    }

    #[test]
    fn daemon_id_is_forwarded_to_the_spawner() {
        let spawner = FakeSpawner::default();
        let exe = OsString::from("/opt/ralphy/bin/ralphy");
        let cwd = Path::new("/work/repo");
        let argv = ["run", "--if-idle"];
        dispatch(
            &spawner,
            &exe,
            &argv,
            cwd,
            Some("01FWD00000000000000000000"),
        )
        .unwrap();
        let calls = spawner.calls.lock().unwrap();
        assert_eq!(calls[0].1, argv, "the composed argv must reach the spawner");
        assert_eq!(
            calls[0].3,
            Some("01FWD00000000000000000000".to_string()),
            "the daemon_id must reach the spawner"
        );
        // The program is a resolved exe, never a shell.
        assert_ne!(calls[0].0, OsStr::new("sh"));
        assert_ne!(calls[0].0, OsStr::new("cmd.exe"));
    }

    #[test]
    fn from_query_accepts_only_the_blessed_verbs() {
        assert_eq!(Verb::from_query("run"), Some(Verb::Run));
        assert_eq!(Verb::from_query("triage"), Some(Verb::Triage));
        assert_eq!(Verb::from_query("push"), Some(Verb::PushQueue));
        assert_eq!(Verb::from_query("tree.list"), Some(Verb::TreeList));
        assert_eq!(Verb::from_query("file.read"), Some(Verb::FileRead));
        // No destructive verb, and no arbitrary composition, is reachable.
        for rejected in ["kill", "stop", "issues", "run --if-idle", "", "Run", "PUSH"] {
            assert_eq!(
                Verb::from_query(rejected),
                None,
                "{rejected:?} must not parse to a verb"
            );
        }
    }
}
