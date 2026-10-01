use super::*;
use std::future::Future;
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

/// A reader that serves `b"out"` once, then blocks until its sender drops: a
/// grandchild that inherited the pipe and holds it open.
struct HeldPipe {
    first: bool,
    hold: std::sync::mpsc::Receiver<()>,
}

impl std::io::Read for HeldPipe {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.first {
            self.first = false;
            buf[..3].copy_from_slice(b"out");
            return Ok(3);
        }
        // `recv` returns only when the test drops the sender: then EOF.
        let _ = self.hold.recv();
        Ok(0)
    }
}

/// A child whose `wait` returns once `exit` is sent (or dropped), and whose
/// output is `output`.
struct GatedChild {
    exit: std::sync::mpsc::Receiver<()>,
    output: Option<Box<dyn std::io::Read + Send>>,
}

impl Child for GatedChild {
    fn pid(&self) -> Option<u32> {
        Some(4243)
    }
    fn wait(&mut self) -> Result<Option<i32>> {
        let _ = self.exit.recv();
        Ok(Some(0))
    }
    fn take_output(&mut self) -> Option<Box<dyn std::io::Read + Send>> {
        self.output.take()
    }
}

/// Hands out ONE prepared child.
struct OneChild(Mutex<Option<Box<dyn Child>>>);

impl Spawner for OneChild {
    fn spawn(
        &self,
        _program: &OsStr,
        _args: &[&str],
        _cwd: &Path,
        _daemon_id: Option<&str>,
    ) -> Result<Box<dyn Child>> {
        self.0
            .lock()
            .unwrap()
            .take()
            .ok_or_else(|| anyhow::anyhow!("spawned twice"))
    }
}

fn one_child(child: impl Child + 'static) -> Arc<dyn Spawner> {
    Arc::new(OneChild(Mutex::new(Some(Box::new(child)))))
}

#[test]
fn collect_returns_the_output_when_a_grandchild_holds_the_pipe() {
    let (_hold, held) = std::sync::mpsc::channel::<()>();
    let (exit_tx, exit) = std::sync::mpsc::channel::<()>();
    drop(exit_tx);
    let spawner = one_child(GatedChild {
        exit,
        output: Some(Box::new(HeldPipe {
            first: true,
            hold: held,
        })),
    });
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let got = collect(
            spawner.as_ref(),
            OsStr::new("ralphy"),
            &[],
            Path::new("."),
            None,
        );
        let _ = tx.send(got.map_err(|e| e.to_string()));
    });
    let got = rx
        .recv_timeout(Duration::from_secs(2))
        .expect("collect must answer while a grandchild holds the pipe");
    assert_eq!(got.unwrap(), (Some(0), b"out".to_vec()));
}

fn call_within(
    spawner: Arc<dyn Spawner>,
    slots: Arc<Semaphore>,
) -> impl Future<Output = Collected> {
    collect_within(
        spawner,
        OsString::from("ralphy"),
        vec!["config".into()],
        std::path::PathBuf::from("."),
        None,
        slots,
        Duration::from_millis(200),
    )
}

#[tokio::test]
async fn collect_within_answers_still_running_for_a_child_that_never_exits() {
    let (_exit_tx, exit) = std::sync::mpsc::channel::<()>();
    let spawner = one_child(GatedChild { exit, output: None });
    let got = tokio::time::timeout(
        Duration::from_secs(2),
        call_within(spawner, Arc::new(Semaphore::new(1))),
    )
    .await
    .expect("the deadline must answer before 2 s");
    assert!(matches!(got, Collected::StillRunning), "got {got:?}");
}

#[tokio::test]
async fn a_command_over_the_limit_waits_for_a_slot_within_the_deadline() {
    let slots = Arc::new(Semaphore::new(1));
    let (exit_a, exit) = std::sync::mpsc::channel::<()>();
    let a = one_child(GatedChild { exit, output: None });
    let got = call_within(a, slots.clone()).await;
    assert!(matches!(got, Collected::StillRunning), "A: got {got:?}");

    let b = || {
        one_child(FakeChild {
            code: 0,
            output: Some(b"b".to_vec()),
        })
    };
    let got = call_within(b(), slots.clone()).await;
    assert!(
        matches!(got, Collected::StillRunning),
        "B must wait while A still runs and holds the one slot: got {got:?}"
    );

    exit_a.send(()).unwrap();
    let freed = tokio::time::timeout(Duration::from_secs(2), async {
        while slots.available_permits() != 1 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    assert!(freed.is_ok(), "A's slot must come back when A exits");
    let got = call_within(b(), slots.clone()).await;
    assert!(
        matches!(got, Collected::Done(Some(0), ref out) if out == b"b"),
        "got {got:?}"
    );
}
