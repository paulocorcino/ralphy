//! `OpenCode` version reads that run the `version_test_child` helper.
#![expect(
    clippy::tests_outside_test_module,
    reason = "an integration test crate is the test module"
)]

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use ralphy_proc_util::opencode::{
    read_opencode_version, read_opencode_version_within, OpenCodeMajor, OpenCodeVersionError,
};

const CHILD: &str = env!("CARGO_BIN_EXE_version_test_child");

/// A copy of the child in `dir` that runs `mode` (one argument per line).
fn copy_child(dir: &Path, mode: &[&str]) -> PathBuf {
    let name = if cfg!(windows) { "child.exe" } else { "child" };
    let target = dir.join(name);
    std::fs::copy(CHILD, &target).expect("copy the child");
    let mut sidecar = target.clone().into_os_string();
    sidecar.push(".args");
    std::fs::write(sidecar, mode.join("\n")).expect("write the mode");
    target
}

fn run_count(counts: &Path) -> usize {
    std::fs::read_to_string(counts).map_or(0, |t| t.lines().count())
}

#[test]
fn a_version_read_past_its_limit_is_unreadable() {
    let root = tempfile::tempdir().expect("temp dir");
    let child = copy_child(root.path(), &["hang"]);
    let started = Instant::now();
    let got = read_opencode_version_within(&child, Duration::from_secs(1));
    assert!(
        matches!(got, Err(OpenCodeVersionError::Unreadable { .. })),
        "{got:?}"
    );
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "{:?}",
        started.elapsed()
    );
}

#[test]
fn the_opencode_version_is_read_once_per_binary() {
    let root = tempfile::tempdir().expect("temp dir");
    let counts = root.path().join("count.txt");
    let counts_arg = counts.to_string_lossy().into_owned();
    let child = copy_child(root.path(), &["count", &counts_arg, "opencode v2.3.2"]);
    for _ in 0..2 {
        let got = read_opencode_version(&child).expect("a version");
        assert_eq!((got.major, got.text.as_str()), (OpenCodeMajor::V2, "2.3.2"));
    }
    assert_eq!(run_count(&counts), 1, "two reads, one spawn");
}

#[test]
fn the_opencode_version_is_read_again_after_an_update() {
    let root = tempfile::tempdir().expect("temp dir");
    let counts = root.path().join("count.txt");
    let counts_arg = counts.to_string_lossy().into_owned();
    let child = copy_child(root.path(), &["count", &counts_arg, "1.19.0"]);
    read_opencode_version(&child).expect("a version");
    let file = std::fs::File::options()
        .write(true)
        .open(&child)
        .expect("open the copy");
    file.set_modified(SystemTime::now() + Duration::from_secs(3600))
        .expect("move the modification time");
    drop(file);
    read_opencode_version(&child).expect("a version");
    assert_eq!(run_count(&counts), 2, "a changed binary is read again");
}
