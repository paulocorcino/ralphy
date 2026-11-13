//! The remote command dispatcher (docs/adr/0032 §2): the closed vocabulary of
//! verbs a browser button can trigger. This is the whole attack surface of
//! remote triggers, kept deliberately narrow:
//!
//! - A remote request names a [`Verb`] by string; [`Verb::from_query`] rejects
//!   everything outside the registry, so no `kill`/`stop` verb and no free text
//!   is reachable. The verb, not the client, chooses the argv.
//! - [`spawn_argv`] composes the argv from the verb plus CLOSED-ENUM params only
//!   (`agent`/`planAgent` via [`crate::session::Agent::from_query`], `branchMode`
//!   via [`BranchMode::from_query`]); any other value yields [`ArgvError`] and
//!   the caller spawns nothing. The program is always the resolved `ralphy` exe
//!   run via `Command::new(exe).args(argv)`, never a shell string.
//!
//! REGISTRY (ADR-0036 §1–2): each [`Verb`] carries an [`EffectClass`]; only
//! `Spawn` verbs reach the CLI, and [`spawn_argv`] is their argv table.
//!
//! TEARDOWN INVARIANT (the inverse of `session`'s): a dispatched run keeps its OWN lifecycle. The
//! daemon must NEVER kill it on shutdown or client disconnect (PRD #157 story 18/20). The
//! [`Child`] handle here is `wait`-only: it has no kill and dropping it does not kill. The
//! teardown arms of the run's relay (`routes::ws_command::stream`) enforce the rest.
//!
//! OUTPUT STREAMING (issue #180): the child's stdout and stderr are merged into one OS pipe
//! ([`Child::take_output`]) that the `spawn` channel streams to the UI log pane. A Rust child
//! ignores `SIGPIPE`, so a daemon crash does not kill it. The duty is on the DAEMON: a live daemon
//! MUST drain the pipe to EOF, so it never fills and stalls the child. The handler's detached
//! drain task does this; its channel to the browser is bounded, and a browser that does not take a
//! frame within the stream's send deadline is given up, so the drain discards.
//!
//! The [`Spawner`]/[`Child`] seam keeps this module unit-testable: a `FakeSpawner` records the argv
//! and returns a preset exit code without touching the OS.

use crate::session::Agent;

mod antigravity;
mod argv;
mod host;
mod registry;
mod spawn;

pub use antigravity::antigravity_root_argv;
#[cfg(test)]
pub(crate) use argv::LOCAL_ONLY_KEYS;
pub use argv::{
    blob_read_argv, board_argv, branch_argv, branch_list_argv, changes_commit_argv,
    changes_list_argv, changes_paths_argv, config_argv, issue_show_argv, label_argv,
    project_remove_argv, run_stop_argv, spawn_argv, sync_argv, sync_status_argv, worktree_add_argv,
    worktree_list_argv, worktree_remove_argv, ArgvError,
};
pub use host::{host_argv, host_password};
pub use registry::project_add_argv;
#[cfg(test)]
pub(crate) use spawn::MAX_COLLECT_CHILDREN;
pub use spawn::{collect, dispatch, ralphy_exe, Child, ProcessSpawner, Spawner, REPLY_DEADLINE};
pub(crate) use spawn::{collect_within, Collected, COLLECT_SLOTS};

/// The effect class of a verb (ADR-0036 §2): `Native` runs in-daemon,
/// `Observe`/`Query` read state, `Spawn` launches a detached `ralphy` child,
/// `Mutate` writes daemon-owned state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EffectClass {
    Native,
    Observe,
    Query,
    Spawn,
    Mutate,
    /// A byte-op on the confined workspace (ADR-0036 Write amendment). Runs
    /// in-daemon (`crate::fswrite`), never spawns, and does NOT consult the run lock.
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
    pub(super) fn as_flag(self) -> &'static str {
        match self {
            BranchMode::New => "new",
            BranchMode::Current => "current",
        }
    }
}

/// The `--agent`/`--plan-agent` CLI flag value for an [`Agent`]. It is owned here
/// so `session::Agent`'s public API does not widen.
pub(crate) fn agent_flag(a: Agent) -> &'static str {
    match a {
        Agent::Antigravity => "antigravity",
        Agent::Claude => "claude",
        Agent::Codex => "codex",
        Agent::Copilot => "copilot",
        Agent::Cursor => "cursor",
        Agent::Kimi => "kimi",
        Agent::OpenCode => "opencode",
    }
}

/// The closed set of remotely-triggerable verbs, in forge-neutral Ralphy words.
/// Anything else, including `kill`/`stop`, is unrepresentable, so no destructive
/// verb can reach a spawn.
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
    /// Find a repo's entries by name (Observe: walks the tree, never spawns —
    /// ADR-0036 amendment 2026-09-15).
    TreeFind,
    /// Find a repo's text files by content (Observe: reads bytes, never
    /// spawns — ADR-0036 amendment 2026-09-15).
    TreeGrep,
    /// Read a repo file's text (Observe: reads state, never spawns).
    FileRead,
    /// The facts of one path: size, time, type, ignored mark (Observe: reads
    /// state, never spawns — ADR-0036 amendment 2026-10-15).
    FileStat,
    /// The size of one folder, counted by a walk with a budget (Observe: reads
    /// state, never spawns — ADR-0036 amendment 2026-10-15).
    TreeSize,
    /// Read a repo file's bytes as an allowlisted image (Observe: reads state,
    /// never spawns — ADR-0049).
    ImageRead,
    /// Read a `.note` file's markdown out of its container (Observe: reads
    /// state, never spawns — ADR-0064 §5).
    NoteRead,
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
    /// Copy a repo file to a new path (Write: in-daemon, never spawns).
    FileCopy,
    /// Delete a repo path (Write: in-daemon, never spawns).
    FileDelete,
    /// Write a pasted raster image under `.ralphy/clipboard/` with a name the
    /// daemon chooses (Write: in-daemon, never spawns, no client path —
    /// ADR-0055).
    ImageWrite,
    /// Append one part of a file the operator uploads; the last part moves it
    /// to its target (Write: in-daemon, never spawns — ADR-0036 amendment
    /// 2026-10-19).
    FileUpload,
    /// Write a note's markdown into a `.note` container (Write: in-daemon,
    /// never spawns; the target class is the verb's, not the client's —
    /// ADR-0064 §5). The ONE Write verb a `checkout` is honoured on: it
    /// confines against that worktree's own root.
    NoteWrite,
    /// List the repo's local branches (Query: `branch list --format json`).
    BranchList,
    /// Check out a branch (Mutate: `branch switch -- <name>`, run-lock-aware).
    BranchSwitch,
    /// Create a branch from HEAD (Mutate: `branch create -- <name>`, run-lock-aware).
    BranchCreate,
    /// List the workbench worktrees (Query: `worktree list --format json`,
    /// ADR-0063 §2).
    WorktreeList,
    /// Create a workbench worktree (Mutate: `worktree add [--base=<ref>] --
    /// <name>`, run-lock-aware, ADR-0063 §2).
    WorktreeAdd,
    /// Remove a workbench worktree (Mutate: `worktree remove -- <name>`,
    /// run-lock-aware; refused in-daemon while a live session's checkout names
    /// it — ADR-0063 §2).
    WorktreeRemove,
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
    /// Ask a live run to stop (Mutate: `stop --runid=<id>`, docs/adr/0054).
    ///
    /// The ONE Mutate that is NOT run-lock-aware: it acts while a run holds the
    /// lock. It is not a kill: the short-lived `ralphy stop` writes a request the
    /// RUN acts on, so the teardown invariant above holds.
    RunStop,
    /// Throw away the repo's finalized plan (Write: in-daemon, never spawns) —
    /// `.ralphy/plan.md`, and nothing else.
    ///
    /// It takes NO client path: the verb alone fixes the target (ADR-0036 §1), so
    /// the `.ralphy` denylist in [`crate::fswrite`] stays whole. The NEXT run
    /// picks up a finalized plan, so this is the way to change one's mind.
    PlanDiscard,
    /// Drop a project from the daemon's registry (Mutate: `daemon remove <slug>`).
    ///
    /// A Mutate and not a Write: the CLI subcommand is the one owner of the
    /// registry file. It unregisters only; the directory on disk is untouched.
    ProjectRemove,
    /// List the hosts of the SSH config of the computer the daemon runs on
    /// (Query: `host aliases`). The host verbs name no repo and always run
    /// on the daemon the browser is connected to (ADR-0036 amendment
    /// 2026-09-29, ADR-0067).
    HostAliases,
    /// Read the state of a destination's host key (Query: `host key`).
    HostKey,
    /// Add a host key whose fingerprint the operator saw (Mutate: `host trust`).
    HostTrust,
    /// Run the host checks, changing nothing (Spawn: `host check --json`).
    HostCheck,
    /// Make a host a peer (Spawn: `host add --json`).
    HostAdd,
    /// Send this computer's Ralphy to a host (Spawn: `host install --json`).
    HostInstall,
    /// Remove a host (Spawn: `host remove --json`).
    HostRemove,
    /// List one level of folder names on the daemon's disk (Observe, no repo).
    DirList,
    /// Register a folder (Mutate: `daemon add [--init] -- <path>`, no repo).
    ProjectAdd,
}

impl Verb {
    /// Parse a remote verb string. Every string outside the registry — `kill`,
    /// `stop`, `issues`, `""` — yields `None`, so the handler can reject it.
    ///
    /// The bare `stop` and `kill` stay unrepresentable. `run.stop` composes a
    /// fixed `ralphy stop --runid=<id>` argv that writes a request the run acts
    /// on; nothing here can signal a process (docs/adr/0054).
    pub fn from_query(value: &str) -> Option<Verb> {
        match value {
            "run" => Some(Verb::Run),
            "triage" => Some(Verb::Triage),
            "push" => Some(Verb::PushQueue),
            "tree.list" => Some(Verb::TreeList),
            "tree.find" => Some(Verb::TreeFind),
            "tree.grep" => Some(Verb::TreeGrep),
            "file.read" => Some(Verb::FileRead),
            "file.stat" => Some(Verb::FileStat),
            "tree.size" => Some(Verb::TreeSize),
            "file.image" => Some(Verb::ImageRead),
            "note.read" => Some(Verb::NoteRead),
            "runs.list" => Some(Verb::RunsList),
            "config.get" => Some(Verb::ConfigGet),
            "board.list" => Some(Verb::BoardList),
            "issue.show" => Some(Verb::IssueShow),
            "config.set" => Some(Verb::ConfigSet),
            "config.unset" => Some(Verb::ConfigUnset),
            "file.write" => Some(Verb::FileWrite),
            "file.create" => Some(Verb::FileCreate),
            "file.rename" => Some(Verb::FileRename),
            "file.copy" => Some(Verb::FileCopy),
            "file.delete" => Some(Verb::FileDelete),
            "image.write" => Some(Verb::ImageWrite),
            "file.upload" => Some(Verb::FileUpload),
            "note.write" => Some(Verb::NoteWrite),
            "branch.list" => Some(Verb::BranchList),
            "branch.switch" => Some(Verb::BranchSwitch),
            "branch.create" => Some(Verb::BranchCreate),
            "worktree.list" => Some(Verb::WorktreeList),
            "worktree.add" => Some(Verb::WorktreeAdd),
            "worktree.remove" => Some(Verb::WorktreeRemove),
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
            "run.stop" => Some(Verb::RunStop),
            "plan.discard" => Some(Verb::PlanDiscard),
            "project.remove" => Some(Verb::ProjectRemove),
            "host.aliases" => Some(Verb::HostAliases),
            "host.key" => Some(Verb::HostKey),
            "host.trust" => Some(Verb::HostTrust),
            "host.check" => Some(Verb::HostCheck),
            "host.add" => Some(Verb::HostAdd),
            "host.install" => Some(Verb::HostInstall),
            "host.remove" => Some(Verb::HostRemove),
            "dir.list" => Some(Verb::DirList),
            "project.add" => Some(Verb::ProjectAdd),
            _ => None,
        }
    }

    /// Every verb in the registry, for exhaustive round-trips.
    pub const ALL: &'static [Verb] = &[
        Verb::Run,
        Verb::Triage,
        Verb::PushQueue,
        Verb::TreeList,
        Verb::TreeFind,
        Verb::TreeGrep,
        Verb::FileRead,
        Verb::FileStat,
        Verb::TreeSize,
        Verb::ImageRead,
        Verb::NoteRead,
        Verb::RunsList,
        Verb::ConfigGet,
        Verb::BoardList,
        Verb::IssueShow,
        Verb::ConfigSet,
        Verb::ConfigUnset,
        Verb::FileWrite,
        Verb::FileCreate,
        Verb::FileRename,
        Verb::FileCopy,
        Verb::FileDelete,
        Verb::ImageWrite,
        Verb::FileUpload,
        Verb::NoteWrite,
        Verb::BranchList,
        Verb::BranchSwitch,
        Verb::BranchCreate,
        Verb::WorktreeList,
        Verb::WorktreeAdd,
        Verb::WorktreeRemove,
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
        Verb::RunStop,
        Verb::PlanDiscard,
        Verb::ProjectRemove,
        Verb::HostAliases,
        Verb::HostKey,
        Verb::HostTrust,
        Verb::HostCheck,
        Verb::HostAdd,
        Verb::HostInstall,
        Verb::HostRemove,
        Verb::DirList,
        Verb::ProjectAdd,
    ];

    /// The effect class of this verb (ADR-0036 §2). The Spawn verbs reach the
    /// CLI through [`spawn_argv`].
    pub fn effect_class(self) -> EffectClass {
        match self {
            Verb::TreeList
            | Verb::TreeFind
            | Verb::TreeGrep
            | Verb::FileRead
            | Verb::FileStat
            | Verb::TreeSize
            | Verb::ImageRead
            | Verb::NoteRead
            | Verb::RunsList
            | Verb::DirList => EffectClass::Observe,
            Verb::ConfigGet
            | Verb::BoardList
            | Verb::IssueShow
            | Verb::BranchList
            | Verb::WorktreeList
            | Verb::ChangesList
            | Verb::BlobRead
            | Verb::SyncStatus
            | Verb::HostAliases
            | Verb::HostKey => EffectClass::Query,
            Verb::ConfigSet
            | Verb::ConfigUnset
            | Verb::BranchSwitch
            | Verb::BranchCreate
            | Verb::WorktreeAdd
            | Verb::WorktreeRemove
            | Verb::LabelSet
            | Verb::SyncFetch
            | Verb::SyncPull
            | Verb::SyncPush
            | Verb::ChangesStage
            | Verb::ChangesUnstage
            | Verb::ChangesCommit
            | Verb::ChangesDiscard
            | Verb::RunStop
            | Verb::ProjectRemove
            | Verb::ProjectAdd
            | Verb::HostTrust => EffectClass::Mutate,
            Verb::FileWrite
            | Verb::FileCreate
            | Verb::FileRename
            | Verb::FileCopy
            | Verb::FileDelete
            | Verb::ImageWrite
            | Verb::FileUpload
            | Verb::NoteWrite
            | Verb::PlanDiscard => EffectClass::Write,
            Verb::Run
            | Verb::Triage
            | Verb::PushQueue
            | Verb::HostCheck
            | Verb::HostAdd
            | Verb::HostInstall
            | Verb::HostRemove => EffectClass::Spawn,
        }
    }

    /// The host family: no repo, local only, composed by [`host_argv`].
    pub fn is_host(self) -> bool {
        matches!(
            self,
            Verb::HostAliases
                | Verb::HostKey
                | Verb::HostTrust
                | Verb::HostCheck
                | Verb::HostAdd
                | Verb::HostInstall
                | Verb::HostRemove
        )
    }

    /// The git-backed family (ADR-0063 §2, ADR-0036 `checkout`): a selected
    /// `checkout` is the composed command's `current_dir`, so the verb acts on
    /// that worktree's index, working tree and HEAD; the argv is unchanged.
    /// Every other verb ignores the key — `.ralphy/` (config, queue, run lock)
    /// is the primary's and `worktree.*` normalises to the primary itself.
    pub fn takes_checkout_cwd(self) -> bool {
        matches!(
            self,
            Verb::BranchList
                | Verb::BranchSwitch
                | Verb::BranchCreate
                | Verb::ChangesList
                | Verb::ChangesStage
                | Verb::ChangesUnstage
                | Verb::ChangesCommit
                | Verb::ChangesDiscard
                | Verb::BlobRead
                | Verb::SyncStatus
                | Verb::SyncFetch
                | Verb::SyncPull
                | Verb::SyncPush
        )
    }
}

#[cfg(test)]
mod tests;
