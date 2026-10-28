//! Hook form tests that run the `version_test_child` helper.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant, SystemTime};

use ralphy_proc_util::hook_form::{
    check, claude_hook, hook_for_version, read_version, HookCheck, HookForm,
};

const CHILD: &str = env!("CARGO_BIN_EXE_version_test_child");

/// A copy of the child in `dir` that runs `mode` (one argument per line in
/// its `.args` file), so a test may change its modification time.
fn copy_child(dir: &Path, mode: &[&str]) -> PathBuf {
    let name = if cfg!(windows) { "child.exe" } else { "child" };
    let target = dir.join(name);
    std::fs::copy(CHILD, &target).expect("copy the child");
    let mut sidecar = target.clone().into_os_string();
    sidecar.push(".args");
    std::fs::write(sidecar, mode.join("\n")).expect("write the mode");
    target
}

fn shell() -> Option<Command> {
    let program = if cfg!(windows) {
        ralphy_proc_util::locate_git_bash()?
    } else {
        PathBuf::from("sh")
    };
    let mut cmd = Command::new(program);
    cmd.arg("-c");
    Some(cmd)
}

#[test]
fn a_shell_form_runs_from_a_path_with_shell_characters() {
    let Some(mut sh) = shell() else {
        println!("skipped: no Git Bash");
        return;
    };
    let root = tempfile::tempdir().expect("temp dir");
    // `"` cannot be in a Windows file name, so the folder has the other four.
    let dir = root.path().join("a b$x`y`it's");
    std::fs::create_dir_all(&dir).expect("create dir");
    let child = copy_child(&dir, &["print", "hello"]);
    let hook = hook_for_version(Some("2.1.100"), &child, &[]);
    assert_eq!(hook.form(), HookForm::Shell);
    let entry = hook.entry();
    let command = entry["command"].as_str().expect("a command string");
    let out = sh.arg(command).output().expect("run the shell");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "hello");
}

#[test]
fn a_hung_version_read_gives_up() {
    let root = tempfile::tempdir().expect("temp dir");
    let child = copy_child(root.path(), &["hang"]);
    let started = Instant::now();
    assert_eq!(read_version(&child, Duration::from_secs(1)), None);
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "{:?}",
        started.elapsed()
    );
    let hook = claude_hook(&child, Path::new("/tools/ralphy"), &["hook", "status"]);
    assert_eq!(hook.form(), HookForm::Shell);
}

#[test]
fn the_version_is_read_once_per_binary_change() {
    let root = tempfile::tempdir().expect("temp dir");
    let counts = root.path().join("count.txt");
    let counts_arg = counts.to_string_lossy().into_owned();
    let child = copy_child(
        root.path(),
        &["count", &counts_arg, "2.1.139 (Claude Code)"],
    );
    let lines = || std::fs::read_to_string(&counts).map_or(0, |t| t.lines().count());

    let limit = Duration::from_secs(10);
    assert_eq!(
        read_version(&child, limit).as_deref(),
        Some("2.1.139 (Claude Code)")
    );
    assert_eq!(
        read_version(&child, limit).as_deref(),
        Some("2.1.139 (Claude Code)")
    );
    assert_eq!(lines(), 1, "two reads, one spawn");

    let file = std::fs::File::options()
        .write(true)
        .open(&child)
        .expect("open the copy");
    file.set_modified(SystemTime::now() + Duration::from_secs(3600))
        .expect("move the modification time");
    drop(file);
    assert_eq!(
        read_version(&child, limit).as_deref(),
        Some("2.1.139 (Claude Code)")
    );
    assert_eq!(lines(), 2, "a changed binary is read again");
}

const EXEC_VERSION: Option<&str> = Some("2.1.139 (Claude Code)");

fn cause(got: HookCheck) -> String {
    match got {
        HookCheck::Failed { cause } => cause,
        other => panic!("expected a failed check, got {other:?}"),
    }
}

#[test]
fn a_missing_hook_program_fails_the_check() {
    let root = tempfile::tempdir().expect("temp dir");
    let missing = root.path().join("no-such-ralphy");
    let hook = hook_for_version(EXEC_VERSION, &missing, &["hook", "status"]);
    let got = cause(check(&hook, Duration::from_secs(5), Some(root.path())));
    assert!(got.contains(&*missing.to_string_lossy()), "{got}");
}

#[test]
fn a_hook_that_writes_no_line_fails_the_check() {
    let root = tempfile::tempdir().expect("temp dir");
    let child = copy_child(root.path(), &["exit0"]);
    let hook = hook_for_version(EXEC_VERSION, &child, &[]);
    let got = cause(check(&hook, Duration::from_secs(10), Some(root.path())));
    assert!(got.contains("wrote no status line"), "{got}");
}

#[test]
fn a_shell_form_is_not_checked() {
    let root = tempfile::tempdir().expect("temp dir");
    let child = copy_child(root.path(), &["exit0"]);
    let hook = hook_for_version(Some("2.1.100 (Claude Code)"), &child, &[]);
    let got = check(&hook, Duration::from_secs(10), Some(root.path()));
    assert!(matches!(got, HookCheck::NotChecked { .. }), "{got:?}");
}

#[test]
fn a_failed_check_names_disable_all_hooks() {
    let root = tempfile::tempdir().expect("temp dir");
    let dir = root.path().join(".claude");
    std::fs::create_dir_all(&dir).expect("create dir");
    let file = dir.join("settings.json");
    std::fs::write(&file, r#"{"disableAllHooks": true}"#).expect("write file");
    let child = copy_child(root.path(), &["exit0"]);
    let hook = hook_for_version(EXEC_VERSION, &child, &[]);
    let got = cause(check(&hook, Duration::from_secs(10), Some(root.path())));
    assert!(got.starts_with(&*file.to_string_lossy()), "{got}");
    assert!(got.contains("wrote no status line"), "{got}");
}

#[test]
fn a_hung_hook_fails_the_check_in_time() {
    let root = tempfile::tempdir().expect("temp dir");
    let child = copy_child(root.path(), &["hang"]);
    let hook = hook_for_version(EXEC_VERSION, &child, &[]);
    let started = Instant::now();
    let got = check(&hook, Duration::from_secs(1), Some(root.path()));
    assert!(matches!(got, HookCheck::Failed { .. }), "{got:?}");
    assert!(
        started.elapsed() < Duration::from_secs(4),
        "{:?}",
        started.elapsed()
    );
}
