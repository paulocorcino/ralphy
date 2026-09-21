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
