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
fn agent_flag(a: Agent) -> &'static str {
    match a {
        Agent::Claude => "claude",
        Agent::Codex => "codex",
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
}

impl Verb {
    /// Parse a remote verb string. Only `run`/`triage`/`push` map to a verb;
    /// every other string — `kill`, `stop`, `issues`, `""` — yields `None`, so
    /// the handler can reject it and spawn nothing.
    pub fn from_query(value: &str) -> Option<Verb> {
        match value {
            "run" => Some(Verb::Run),
            "triage" => Some(Verb::Triage),
            "push" => Some(Verb::PushQueue),
            _ => None,
        }
    }

    /// Every verb in the registry, for exhaustive round-trips.
    pub const ALL: &'static [Verb] = &[Verb::Run, Verb::Triage, Verb::PushQueue];

    /// The effect class of this verb (ADR-0036 §2). Every verb is `Spawn` today —
    /// each reaches the CLI through [`spawn_argv`].
    pub fn effect_class(self) -> EffectClass {
        EffectClass::Spawn
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
    }
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
    fn every_verb_in_all_is_spawn() {
        for &verb in Verb::ALL {
            assert_eq!(
                verb.effect_class(),
                EffectClass::Spawn,
                "{verb:?} must be a Spawn verb"
            );
        }
        assert_eq!(Verb::ALL.len(), 3, "the registry holds exactly three verbs");
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
            vec!["run", "--if-idle", "--agent", "claude", "--branch-mode", "new"]
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
            vec!["run", "--if-idle", "--agent", "claude", "--branch-mode", "new"]
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
    fn from_query_accepts_only_the_three_blessed_verbs() {
        assert_eq!(Verb::from_query("run"), Some(Verb::Run));
        assert_eq!(Verb::from_query("triage"), Some(Verb::Triage));
        assert_eq!(Verb::from_query("push"), Some(Verb::PushQueue));
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
