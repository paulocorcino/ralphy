//! The remote command dispatcher (docs/adr/0032 §2): the closed vocabulary of
//! verbs a browser button can trigger, each mapped to EXACTLY one blessed
//! `ralphy` invocation and spawned as a detached child. This is the whole
//! attack surface of remote triggers, kept deliberately narrow:
//!
//! - A remote request names a [`Verb`] by string; [`Verb::from_query`] rejects
//!   everything outside `run`/`triage`/`push`, so no `kill`/`stop` verb — and no
//!   free-text — is reachable. The verb, not the client, chooses the argv.
//! - `blessed_args` returns a `&'static` argv per verb; the client never
//!   composes arguments. The program is always the resolved `ralphy` exe run via
//!   `Command::new(exe).args(argv)` — never a shell string, so nothing the client
//!   sends is interpreted by `sh`/`cmd`.
//!
//! TEARDOWN INVARIANT (the inverse of `session`'s): a dispatched run keeps its
//! OWN lifecycle — the daemon must NEVER kill it on shutdown or client
//! disconnect (PRD #157 story 18/20: "spawned runs keep their own lifecycle", "a
//! daemon crash never kills a run"). The [`Child`] handle here is `wait`-only;
//! it has no kill and dropping it does not kill (std semantics). The `/ws/command`
//! handler's teardown arms enforce the rest.
//!
//! The [`Spawner`]/[`Child`] seam keeps this module unit-testable: a `FakeSpawner`
//! records the argv and returns a preset exit code without touching the OS.

use std::ffi::{OsStr, OsString};
use std::path::Path;

use anyhow::Result;

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

    /// The one blessed argv for this verb. `&'static` on purpose: the client
    /// never contributes an argument, so remote input can never widen the
    /// command line.
    fn blessed_args(self) -> &'static [&'static str] {
        match self {
            Verb::Run => &["run", "--if-idle"],
            Verb::Triage => &["triage", "--if-idle", "--yes"],
            Verb::PushQueue => &["issues", "--push"],
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

/// Spawn the blessed child for `verb`: `program` (the resolved `ralphy` exe) with
/// `verb`'s `&'static` argv, in `cwd`. The verb — not any client input — chooses
/// the argv, and `program` is a real exe run without a shell.
pub fn dispatch(
    spawner: &dyn Spawner,
    program: &OsStr,
    verb: Verb,
    cwd: &Path,
    daemon_id: Option<&str>,
) -> Result<Box<dyn Child>> {
    spawner.spawn(program, verb.blessed_args(), cwd, daemon_id)
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
        cmd.args(args)
            .current_dir(cwd)
            // Null stdio: a dispatched run keeps its own console-less lifecycle
            // and CloudEvents sink; null prevents pipe-fill blocking and keeps the
            // daemon's own logs clean.
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
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
        Ok(Box::new(ProcessChild(child)))
    }
}

/// A real OS child. `wait`-only: no kill method exists, and dropping it does not
/// kill (std semantics) — the dispatched run outlives the daemon.
struct ProcessChild(std::process::Child);

impl Child for ProcessChild {
    fn pid(&self) -> Option<u32> {
        Some(self.0.id())
    }

    fn wait(&mut self) -> Result<Option<i32>> {
        Ok(self.0.wait()?.code())
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

    struct FakeChild {
        code: i32,
    }

    impl Child for FakeChild {
        fn pid(&self) -> Option<u32> {
            Some(4242)
        }
        fn wait(&mut self) -> Result<Option<i32>> {
            Ok(Some(self.code))
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
            Ok(Box::new(FakeChild { code: 7 }))
        }
    }

    #[test]
    fn each_verb_maps_to_its_blessed_argv() {
        let exe = OsString::from("/opt/ralphy/bin/ralphy");
        let cwd = Path::new("/work/repo");
        let cases = [
            (Verb::Run, vec!["run", "--if-idle"]),
            (Verb::Triage, vec!["triage", "--if-idle", "--yes"]),
            (Verb::PushQueue, vec!["issues", "--push"]),
        ];
        for (verb, expected) in cases {
            let spawner = FakeSpawner::default();
            let mut child = dispatch(&spawner, &exe, verb, cwd, None).unwrap();
            let calls = spawner.calls.lock().unwrap();
            assert_eq!(calls.len(), 1, "{verb:?} must spawn exactly once");
            let (program, args, seen_cwd, _daemon_id) = &calls[0];
            assert_eq!(args, &expected, "{verb:?} argv");
            assert_eq!(program, &exe, "program must be the passed exe");
            // The program is a resolved exe, never a shell — remote input can
            // never be interpreted by a shell.
            assert_ne!(program, OsStr::new("sh"));
            assert_ne!(program, OsStr::new("cmd"));
            assert_ne!(program, OsStr::new("cmd.exe"));
            assert_eq!(seen_cwd, cwd);
            assert_eq!(child.wait().unwrap(), Some(7));
        }
    }

    #[test]
    fn daemon_id_is_forwarded_to_the_spawner() {
        let spawner = FakeSpawner::default();
        let exe = OsString::from("/opt/ralphy/bin/ralphy");
        let cwd = Path::new("/work/repo");
        dispatch(
            &spawner,
            &exe,
            Verb::Run,
            cwd,
            Some("01FWD00000000000000000000"),
        )
        .unwrap();
        let calls = spawner.calls.lock().unwrap();
        assert_eq!(
            calls[0].3,
            Some("01FWD00000000000000000000".to_string()),
            "the daemon_id must reach the spawner"
        );
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
