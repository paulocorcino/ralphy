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

use crate::session::Agent;

mod argv;
mod spawn;

#[cfg(test)]
pub(crate) use argv::LOCAL_ONLY_KEYS;
pub use argv::{
    blob_read_argv, board_argv, branch_argv, branch_list_argv, changes_commit_argv,
    changes_list_argv, changes_paths_argv, config_argv, issue_show_argv, label_argv,
    project_remove_argv, run_stop_argv, spawn_argv, sync_argv, sync_status_argv, worktree_add_argv,
    worktree_list_argv, worktree_remove_argv, ArgvError,
};
pub use spawn::{collect, dispatch, ralphy_exe, Child, ProcessSpawner, Spawner};

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
    pub(super) fn as_flag(self) -> &'static str {
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
    /// Find a repo's entries by name (Observe: walks the tree, never spawns —
    /// ADR-0036 amendment 2026-09-15).
    TreeFind,
    /// Find a repo's text files by content (Observe: reads bytes, never
    /// spawns — ADR-0036 amendment 2026-09-15).
    TreeGrep,
    /// Read a repo file's text (Observe: reads state, never spawns).
    FileRead,
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
    /// List the workbench worktrees (Query: `worktree list --format json`, ADR-0063 §2).
    WorktreeList,
    /// Create a workbench worktree (Mutate: `worktree add [--base=<ref>] -- <name>`, run-lock-aware, ADR-0063 §2).
    WorktreeAdd,
    /// Remove a workbench worktree (Mutate: `worktree remove -- <name>`, run-lock-aware; refused
    /// in-daemon while a live session's checkout names it — ADR-0063 §2).
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
    /// The ONE Mutate that is deliberately NOT run-lock-aware — it exists to act
    /// while a run holds the lock. It is also not a Spawn and not a kill: it
    /// spawns a short-lived `ralphy stop` that writes a request the RUN acts on,
    /// so the teardown invariant above holds unchanged (the daemon still never
    /// signals or kills a dispatched child).
    RunStop,
    /// Throw away the repo's finalized plan (Write: in-daemon, never spawns) —
    /// `.ralphy/plan.md`, and nothing else.
    ///
    /// It takes NO client path: the verb alone fixes the target, exactly as
    /// `runs.list` fixes what it reads (ADR-0036 §1). That is what lets the
    /// `.ralphy` denylist in [`crate::fswrite`] stay whole — a plan discard is a
    /// named capability, not a hole in the protected directory. It exists because
    /// a finalized plan is picked up by the NEXT run (the plan trailer is the
    /// resume signal), so changing one's mind otherwise meant a hand-deleted file.
    PlanDiscard,
    /// Drop a project from the daemon's registry (Mutate: `daemon remove <slug>`).
    ///
    /// A Mutate and not a Write: the registry file has exactly one owner, the CLI
    /// subcommand, and the daemon never edits `repos.toml` itself. It unregisters
    /// only — the directory on disk is untouched.
    ProjectRemove,
}

impl Verb {
    /// Parse a remote verb string. `run`/`triage`/`push` map to Spawn verbs and
    /// `tree.list`/`file.read` to Observe verbs; every other string — `kill`,
    /// `stop`, `issues`, `""` — yields `None`, so the handler can reject it.
    ///
    /// Note the bare `stop` and `kill` are STILL unrepresentable. `run.stop` is
    /// a different string and a different thing: it composes a fixed
    /// `ralphy stop --runid=<id>` argv, and `ralphy stop` writes a request the
    /// run acts on. Nothing here has gained the power to signal a process
    /// (docs/adr/0054).
    pub fn from_query(value: &str) -> Option<Verb> {
        match value {
            "run" => Some(Verb::Run),
            "triage" => Some(Verb::Triage),
            "push" => Some(Verb::PushQueue),
            "tree.list" => Some(Verb::TreeList),
            "tree.find" => Some(Verb::TreeFind),
            "tree.grep" => Some(Verb::TreeGrep),
            "file.read" => Some(Verb::FileRead),
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
    ];

    /// The effect class of this verb (ADR-0036 §2): the Observe read verbs read
    /// state in-daemon and never spawn; `config.get` is a Query (spawn-and-collect
    /// `config get --json`); `config.set`/`config.unset` are Mutate; the three run
    /// verbs reach the CLI through [`spawn_argv`].
    pub fn effect_class(self) -> EffectClass {
        match self {
            Verb::TreeList
            | Verb::TreeFind
            | Verb::TreeGrep
            | Verb::FileRead
            | Verb::ImageRead
            | Verb::NoteRead
            | Verb::RunsList => EffectClass::Observe,
            Verb::ConfigGet
            | Verb::BoardList
            | Verb::IssueShow
            | Verb::BranchList
            | Verb::WorktreeList
            | Verb::ChangesList
            | Verb::BlobRead
            | Verb::SyncStatus => EffectClass::Query,
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
            | Verb::ProjectRemove => EffectClass::Mutate,
            Verb::FileWrite
            | Verb::FileCreate
            | Verb::FileRename
            | Verb::FileCopy
            | Verb::FileDelete
            | Verb::ImageWrite
            | Verb::NoteWrite
            | Verb::PlanDiscard => EffectClass::Write,
            Verb::Run | Verb::Triage | Verb::PushQueue => EffectClass::Spawn,
        }
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
mod tests {
    use super::*;

    #[test]
    fn verb_effect_classes() {
        assert_eq!(Verb::TreeList.effect_class(), EffectClass::Observe);
        assert_eq!(Verb::TreeFind.effect_class(), EffectClass::Observe);
        assert_eq!(Verb::TreeGrep.effect_class(), EffectClass::Observe);
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
        assert_eq!(Verb::FileCopy.effect_class(), EffectClass::Write);
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
        // A stop is a Mutate, never a Spawn: it runs one short `ralphy stop` and
        // collects it, exactly like `sync push` (docs/adr/0054).
        assert_eq!(Verb::RunStop.effect_class(), EffectClass::Mutate);
        // A plan discard is a Write: one in-daemon unlink of a target the VERB
        // fixes, never a spawn and never a client-named path.
        assert_eq!(Verb::from_query("plan.discard"), Some(Verb::PlanDiscard));
        assert_eq!(Verb::PlanDiscard.effect_class(), EffectClass::Write);
        // A clipboard drop is a Write: one in-daemon confined write of bytes the
        // daemon verified, to a name the daemon chose — never a spawn and never
        // a client-named path (ADR-0055).
        assert_eq!(Verb::from_query("image.write"), Some(Verb::ImageWrite));
        assert_eq!(Verb::ImageWrite.effect_class(), EffectClass::Write);
        // A project removal is a Mutate: it spawns the existing `daemon remove`
        // subcommand, the ONE owner of the registry file (issue #363).
        assert_eq!(
            Verb::from_query("project.remove"),
            Some(Verb::ProjectRemove)
        );
        assert_eq!(Verb::ProjectRemove.effect_class(), EffectClass::Mutate);
        // The two note verbs (ADR-0064 §5). `note.read` is an Observe read
        // like `file.image`; `note.write` is a Write whose target class the
        // VERB fixes — only a `.note` path, and the container is the daemon's
        // to build, so the client never hands over bytes.
        assert_eq!(Verb::from_query("note.read"), Some(Verb::NoteRead));
        assert_eq!(Verb::NoteRead.effect_class(), EffectClass::Observe);
        assert_eq!(Verb::from_query("note.write"), Some(Verb::NoteWrite));
        assert_eq!(Verb::NoteWrite.effect_class(), EffectClass::Write);
        assert_eq!(
            Verb::ALL.len(),
            42,
            "the registry holds exactly forty-two verbs"
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
        assert_eq!(Verb::from_query("file.copy"), Some(Verb::FileCopy));
        assert_eq!(Verb::from_query("file.delete"), Some(Verb::FileDelete));
        assert_eq!(Verb::from_query("branch.list"), Some(Verb::BranchList));
        assert_eq!(Verb::from_query("branch.switch"), Some(Verb::BranchSwitch));
        assert_eq!(Verb::from_query("branch.create"), Some(Verb::BranchCreate));
        assert_eq!(Verb::from_query("label.set"), Some(Verb::LabelSet));
        assert_eq!(Verb::from_query("blob.read"), Some(Verb::BlobRead));
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
        let agents_js = include_str!("../assets/ui/wb-agents.js");

        // Non-vacuous first: app.js delegates its request URL to the roster
        // module, whose repo-specific and local forms both name the endpoint.
        assert!(
            js.contains("window.WBAgents.rosterUrl(repo)")
                && agents_js.contains("/api/agents?repo=")
                && agents_js.contains("\"/api/agents\""),
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
        let named_non_default_vendors = |source: &str| {
            Agent::ALL
                .into_iter()
                .map(agent_flag)
                .filter(|flag| *flag != "claude")
                .filter(|flag| {
                    ['"', '\'', '`']
                        .into_iter()
                        .any(|quote| source.contains(&format!("{quote}{flag}{quote}")))
                })
                .collect::<Vec<_>>()
        };
        assert!(
            named_non_default_vendors(js).is_empty(),
            "app.js must hold no non-default vendor names"
        );
        assert_eq!(
            named_non_default_vendors(r#"agents: [{ id: "codex" }]"#),
            ["codex"],
            "the vendor-list guard must reject a populated production roster"
        );
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
    fn from_query_accepts_only_the_blessed_verbs() {
        assert_eq!(Verb::from_query("run"), Some(Verb::Run));
        assert_eq!(Verb::from_query("triage"), Some(Verb::Triage));
        assert_eq!(Verb::from_query("push"), Some(Verb::PushQueue));
        assert_eq!(Verb::from_query("tree.list"), Some(Verb::TreeList));
        assert_eq!(Verb::from_query("tree.find"), Some(Verb::TreeFind));
        assert_eq!(Verb::from_query("tree.grep"), Some(Verb::TreeGrep));
        assert_eq!(Verb::from_query("file.read"), Some(Verb::FileRead));
        // No destructive verb, and no arbitrary composition, is reachable.
        // `stop` and `kill` stay unrepresentable even though `run.stop` now
        // exists (docs/adr/0054): that verb spawns a `ralphy stop` which WRITES A
        // REQUEST, and nothing on this surface ever signals a process. Do not
        // relax this list to accommodate it.
        // `plan.discard` is the whole plan capability: no `plan.write`, no
        // `plan.read` (the panel reads the plan through `file.read` like any other
        // file), and no path parameter anywhere near it.
        // `image.write` is likewise the whole clipboard-drop capability: the
        // read side is `file.image`, and there is no generic `image.*` family.
        // The note family is two verbs and stays two: a note is renamed and
        // deleted with `file.rename`/`file.delete`, which reach it through the
        // denylist's one carve-out (ADR-0064 §5), not through a `note.*` verb
        // that would have to re-derive the same confinement.
        for rejected in [
            "note",
            "note.delete",
            "note.rename",
            "note.list",
            "image.read",
            "image",
            "image.delete",
            "kill",
            "stop",
            "issues",
            "run --if-idle",
            "",
            "Run",
            "PUSH",
            "run.kill",
            "plan.write",
            "plan.delete",
            "plan",
        ] {
            assert_eq!(
                Verb::from_query(rejected),
                None,
                "{rejected:?} must not parse to a verb"
            );
        }
    }

    /// The cwd-taking set is exactly the 13 git-backed verbs (ADR-0063 §2 plus
    /// `sync.*`): every Observe/Write/Spawn verb and the `.ralphy`-owning
    /// Query/Mutate verbs keep the registry path whatever `checkout` says.
    #[test]
    fn takes_checkout_cwd_is_exactly_the_git_backed_family() {
        let expected = [
            Verb::BranchList,
            Verb::BranchSwitch,
            Verb::BranchCreate,
            Verb::ChangesList,
            Verb::ChangesStage,
            Verb::ChangesUnstage,
            Verb::ChangesCommit,
            Verb::ChangesDiscard,
            Verb::BlobRead,
            Verb::SyncStatus,
            Verb::SyncFetch,
            Verb::SyncPull,
            Verb::SyncPush,
        ];
        let mut count = 0;
        for &v in Verb::ALL {
            assert_eq!(
                v.takes_checkout_cwd(),
                expected.contains(&v),
                "{v:?} membership in the git-backed family"
            );
            if v.takes_checkout_cwd() {
                count += 1;
            }
        }
        assert_eq!(count, 13, "the family is the 13 git-backed verbs");
        assert_eq!(Verb::ALL.len(), 42, "Verb::ALL grew — revisit the family");

        for &v in Verb::ALL {
            if matches!(
                v.effect_class(),
                EffectClass::Observe | EffectClass::Write | EffectClass::Spawn
            ) {
                assert!(
                    !v.takes_checkout_cwd(),
                    "{v:?} is not Query/Mutate, so never takes the cwd"
                );
            }
        }
        for v in [
            Verb::ConfigGet,
            Verb::ConfigSet,
            Verb::ConfigUnset,
            Verb::BoardList,
            Verb::IssueShow,
            Verb::WorktreeList,
            Verb::WorktreeAdd,
            Verb::WorktreeRemove,
            Verb::LabelSet,
            Verb::RunStop,
            Verb::ProjectRemove,
        ] {
            assert!(
                !v.takes_checkout_cwd(),
                "{v:?} owns the primary's .ralphy/ and keeps the registry path"
            );
        }
    }
}
