/// The nvm layout that made this necessary: the CLI and its interpreter are
/// siblings in a directory nothing has on `PATH`.
#[test]
fn path_with_dir_prepends_a_missing_directory() {
    use std::path::Path;

    let dir = Path::new("/home/me/.nvm/versions/node/v24.13.0/bin");
    let current = std::env::join_paths(["/usr/bin", "/bin"]).expect("fixture joins");
    let widened = path_with_dir(dir, Some(current.as_os_str()))
        .expect("a directory absent from PATH is prepended");

    let entries: Vec<_> = std::env::split_paths(&widened).collect();
    assert_eq!(
        entries.first().map(PathBuf::as_path),
        Some(dir),
        "{entries:?}"
    );
    assert!(
        entries.iter().any(|p| p == Path::new("/usr/bin")),
        "{entries:?}"
    );
    assert!(
        entries.iter().any(|p| p == Path::new("/bin")),
        "{entries:?}"
    );
}

/// Idempotent: re-running must not grow `PATH` without bound.
#[test]
fn path_with_dir_declines_a_directory_already_present() {
    use std::path::Path;

    let dir = Path::new("/opt/tools/bin");
    let current = std::env::join_paths(["/usr/bin", "/opt/tools/bin"]).expect("fixture joins");
    assert!(path_with_dir(dir, Some(current.as_os_str())).is_none());
}

/// An empty or absent `PATH` still yields a usable one-entry `PATH`.
#[test]
fn path_with_dir_handles_an_absent_path() {
    use std::path::Path;

    let dir = Path::new("/opt/tools/bin");
    let widened = path_with_dir(dir, None).expect("an absent PATH still widens");
    let entries: Vec<_> = std::env::split_paths(&widened).collect();
    assert!(entries.contains(&dir.to_path_buf()), "{entries:?}");
}

/// The `Command` wrapper extends a `PATH` the caller already scrubbed, rather
/// than replacing it with the parent's — a vendor's containment still wins.
#[test]
fn program_dir_on_path_extends_a_declared_path() {
    use std::path::{Path, PathBuf};
    use std::process::Command;

    let program = PathBuf::from("/home/me/.nvm/versions/node/v24.13.0/bin/codex");
    let scrubbed = std::env::join_paths(["/scrubbed/bin"]).expect("fixture joins");
    let mut cmd = Command::new(&program);
    cmd.env("PATH", &scrubbed);
    program_dir_on_path(&mut cmd);

    let path = cmd
        .get_envs()
        .find(|(k, _)| *k == std::ffi::OsStr::new("PATH"))
        .and_then(|(_, v)| v)
        .expect("PATH is set on the command");
    let entries: Vec<_> = std::env::split_paths(path).collect();
    assert_eq!(
        entries.first().map(PathBuf::as_path),
        program.parent(),
        "the program's own directory leads: {entries:?}"
    );
    assert!(
        entries.iter().any(|p| p == Path::new("/scrubbed/bin")),
        "the caller's scrub survives: {entries:?}"
    );
}

/// A bare program name resolved nothing, so there is no directory to add and
/// the child's `PATH` must be left exactly as it was.
#[test]
fn program_dir_on_path_leaves_a_bare_program_alone() {
    use std::process::Command;

    let mut cmd = Command::new("codex");
    program_dir_on_path(&mut cmd);
    assert!(
        cmd.get_envs()
            .all(|(k, _)| k != std::ffi::OsStr::new("PATH")),
        "a bare name must not touch PATH"
    );
}

use super::*;
use std::fs;

mod find;
mod git_bash;
mod tree;

/// Mark a freshly-written fixture executable so `find_program`'s Unix
/// execute-bit check accepts it. A no-op off Unix (Windows gates on PATHEXT).
fn mark_executable(p: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(p).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(p, perms).unwrap();
    }
    #[cfg(not(unix))]
    let _ = p;
}

/// Remove a scratch directory a test made. It may be gone already, and a
/// leftover directory in the temp dir fails nothing.
fn remove_scratch(dir: &Path) {
    #[expect(
        clippy::let_underscore_must_use,
        reason = "cleanup of a test directory that may be gone, and no test reads it after"
    )]
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn home_scoped_path_override_replaces_the_home_base() {
    // The override branch: `base.join(tail)`, with `home_rel` ignored.
    assert_eq!(
        home_scoped_path(
            Some("X".into()),
            Path::new(".codex"),
            Path::new("config.toml")
        ),
        Some(PathBuf::from("X").join("config.toml"))
    );
}

#[test]
fn home_scoped_path_home_branch_joins_home_rel_and_tail() {
    // The home branch: `home_dir()/home_rel/tail`. Deterministic against the
    // same `home_dir()` the helper uses — no env mutation.
    assert_eq!(
        home_scoped_path(None, Path::new(".codex"), Path::new("config.toml")),
        home_dir().map(|h| h.join(".codex").join("config.toml"))
    );
}

#[test]
fn find_program_locates_a_file_on_the_search_path() {
    let tmp = std::env::temp_dir().join(format!("ralphy-find-prog-{}", std::process::id()));
    remove_scratch(&tmp);
    fs::create_dir_all(&tmp).unwrap();
    // A file with the searched extension (".EXE" via PATHEXT on Windows; on
    // non-Windows the bare name is what is matched).
    let bare = tmp.join("tool");
    let exe = tmp.join("tool.exe");
    let target = if cfg!(windows) { &exe } else { &bare };
    fs::write(target, b"x").unwrap();
    mark_executable(target);

    let path_var = tmp.clone().into_os_string();
    let got = find_program("tool", Some(path_var), Some(".EXE".into()))
        .expect("must locate the file on PATH");
    // The resolved path must point at a real file whose stem is `tool` (the
    // extension casing may follow PATHEXT, which is harmless on Windows'
    // case-insensitive filesystem).
    assert!(got.is_file(), "resolved path must exist: {got:?}");
    assert_eq!(got.file_stem().and_then(|s| s.to_str()), Some("tool"));

    remove_scratch(&tmp);
}

#[test]
#[cfg(windows)]
fn find_program_resolves_the_cmd_of_an_npm_shim() {
    // An npm CLI present only as `name.cmd` (no `.exe`) must still resolve,
    // since `Command::new("name")` would not find it. In the exact npm layout a
    // bare `opencode` shell shim sits next to `opencode.cmd`; the bare file is
    // not a valid Win32 application (os error 193), so the resolver must return
    // the `.cmd`, not the shim.
    // (case, files in the PATH directory)
    let rows: [(&str, &[&str]); 2] = [
        ("only the .cmd", &["opencode.cmd"]),
        ("bare shim next to the .cmd", &["opencode", "opencode.cmd"]),
    ];
    for (i, (case, files)) in rows.into_iter().enumerate() {
        let tmp = std::env::temp_dir().join(format!("ralphy-find-cmd-{}-{i}", std::process::id()));
        remove_scratch(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        for file in files {
            fs::write(
                tmp.join(file),
                b"@echo off
",
            )
            .unwrap();
        }
        let got = find_program(
            "opencode",
            Some(tmp.clone().into_os_string()),
            Some(".EXE;.CMD".into()),
        );
        remove_scratch(&tmp);
        // The extension casing follows PATHEXT, which is harmless on a
        // case-insensitive filesystem.
        let got = got.unwrap_or_else(|| panic!("{case}: must resolve"));
        assert_eq!(got.parent(), Some(tmp.as_path()), "{case}: {got:?}");
        assert!(
            got.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.eq_ignore_ascii_case("opencode.cmd")),
            "{case}: must return the .cmd: {got:?}"
        );
    }
}

/// ADR-0043 D16: a `/mnt/<drive>/…` directory is a Windows mount, and a Linux
/// `PATH` search must skip it. The helper is pure and OS-independent so both
/// directions are asserted on Windows and Linux CI alike.
#[test]
fn a_windows_mount_path_is_rejected_off_windows() {
    assert!(is_windows_mount_path(Path::new(
        "/mnt/c/Users/x/AppData/Roaming/npm"
    )));
    assert!(is_windows_mount_path(Path::new("/mnt/d")));
    // `/mnt/data` is an ordinary Linux mount: the second component must be a
    // SINGLE letter, or every `/mnt/*` volume would be skipped.
    assert!(!is_windows_mount_path(Path::new("/mnt/data")));
    assert!(!is_windows_mount_path(Path::new("/usr/local/bin")));
    assert!(!is_windows_mount_path(Path::new("/mnt")));
    // Relative paths never qualify — the layout is rooted by definition.
    assert!(!is_windows_mount_path(Path::new("mnt/c/npm")));
}

/// ADR-0043 D16: npm's global bin under a version-managed Node install is off
/// `PATH` in a non-login shell, so the locator has to look there by hand.
#[test]
fn nvm_candidates_cover_a_version_managed_install() {
    let home = tempfile::tempdir().unwrap();
    assert!(
        nvm_candidates(home.path(), "gemini").is_empty(),
        "no .nvm at all is not an error"
    );

    // Deliberately spanning the decade boundary a lexicographic sort gets
    // wrong (`v10` sorts before `v9` as strings).
    for v in ["v9.11.2", "v22.22.2"] {
        let bin = home.path().join(".nvm/versions/node").join(v).join("bin");
        fs::create_dir_all(&bin).unwrap();
        let exe = bin.join("gemini");
        fs::write(&exe, b"#!/usr/bin/env node\n").unwrap();
        mark_executable(&exe);
    }

    let got = nvm_candidates(home.path(), "gemini");
    assert_eq!(got.len(), 2, "{got:?}");
    // NEWEST first: a lexicographic sort would put v10 before v9 and v20
    // before v22, handing the locator the older runtime every time.
    assert!(
        got[0].to_string_lossy().contains("v22.22.2"),
        "newest version must come first: {got:?}"
    );
    assert!(got[1].to_string_lossy().contains("v9.11.2"), "{got:?}");
    // Every candidate is `<version>/bin/<name>` — the `bin` segment is what a
    // path built from the wrong join would silently lose.
    for p in &got {
        assert_eq!(p.file_name().and_then(|n| n.to_str()), Some("gemini"));
        assert_eq!(
            p.parent()
                .and_then(|d| d.file_name())
                .and_then(|n| n.to_str()),
            Some("bin"),
            "{p:?}"
        );
    }

    // …and the locator actually reaches them: `PATH` is empty, `~/.local/bin`
    // holds nothing, so only the nvm fallback can answer. (Windows gates
    // executability on PATHEXT, so the extensionless fixture only resolves on
    // Unix — the CANDIDATE list above is what this asserts cross-platform.)
    if cfg!(unix) {
        let found = locate_program_with(
            "gemini",
            Some(std::ffi::OsString::new()),
            None,
            Some(home.path().to_path_buf()),
        );
        assert_eq!(found, got.first().cloned(), "the nvm fallback must be hit");
    }
}

#[test]
fn find_program_returns_none_when_absent() {
    let path_var = std::env::temp_dir().into_os_string();
    assert!(find_program(
        "definitely-not-a-real-prog-xyz",
        Some(path_var),
        Some(".EXE".into())
    )
    .is_none());
}

#[test]
fn locate_program_falls_back_to_local_bin_when_off_path() {
    // A program absent from PATH but present in ~/.local/bin must still be
    // located — this is the gate/execution unification: the env gate would
    // otherwise miss it, while the adapter (resolve_program) would still run it.
    let home = std::env::temp_dir().join(format!("ralphy-locate-home-{}", std::process::id()));
    remove_scratch(&home);
    let bin = home.join(".local").join("bin");
    fs::create_dir_all(&bin).unwrap();
    let name = "myagent";
    let file = if cfg!(windows) {
        bin.join("myagent.exe")
    } else {
        bin.join("myagent")
    };
    fs::write(&file, b"x").unwrap();
    mark_executable(&file);

    // Empty PATH so only the ~/.local/bin fallback can match.
    let got = locate_program_with(
        name,
        Some(std::ffi::OsString::new()),
        Some(".EXE".into()),
        Some(home.clone()),
    );
    assert_eq!(got.as_deref(), Some(file.as_path()));

    // Without a home, the fallback can't fire → None.
    assert!(locate_program_with(
        name,
        Some(std::ffi::OsString::new()),
        Some(".EXE".into()),
        None
    )
    .is_none());

    remove_scratch(&home);
}

#[test]
fn locate_program_prefers_path_over_local_bin() {
    // When the program is on PATH, that wins — the fallback is only consulted
    // when PATH has nothing.
    let tmp = std::env::temp_dir().join(format!("ralphy-locate-path-{}", std::process::id()));
    remove_scratch(&tmp);
    let exe = if cfg!(windows) { "tool.exe" } else { "tool" };
    // The same program on PATH and in ~/.local/bin, so only the order decides.
    let path_dir = tmp.join("path");
    let local_bin = tmp.join("home").join(".local").join("bin");
    for dir in [&path_dir, &local_bin] {
        fs::create_dir_all(dir).unwrap();
        fs::write(dir.join(exe), b"x").unwrap();
        mark_executable(&dir.join(exe));
    }

    let got = locate_program_with(
        "tool",
        Some(path_dir.clone().into_os_string()),
        Some(".EXE".into()),
        Some(tmp.join("home")),
    )
    .expect("PATH hit must win");
    // Compare by parent + stem: on Windows the resolved extension casing follows
    // PATHEXT (`.EXE`) rather than the file's `.exe`, which is harmless.
    assert_eq!(got.parent(), Some(path_dir.as_path()));
    assert_eq!(got.file_stem(), Some(std::ffi::OsStr::new("tool")));
    remove_scratch(&tmp);
}

/// A detached grandchild must not hold the pipe its parent writes to: a reader
/// of `ralphy daemon restart` waited until the daemon stopped (2026-10-02).
#[cfg(windows)]
#[test]
fn std_handles_do_not_reach_a_detached_grandchild() {
    use std::io::Read;
    use std::process::Stdio;
    use std::time::Duration;

    let mut parent = Command::new(std::env::current_exe().expect("current_exe"))
        .args([
            "--exact",
            "tests::a_parent_that_leaves_a_detached_child",
            "--ignored",
            "--nocapture",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn the parent");
    let mut stdout = parent.stdout.take().expect("the parent's stdout");
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut text = String::new();
        let read = stdout.read_to_string(&mut text).map(|_| text);
        #[expect(
            clippy::let_underscore_must_use,
            reason = "the receiver is gone once the test has its answer"
        )]
        let _ = tx.send(read);
    });
    let ended = rx.recv_timeout(Duration::from_secs(20));
    parent.wait().expect("reap the parent");
    let grandchild = |text: &str| {
        text.lines()
            .find_map(|l| l.strip_prefix("grandchild pid "))
            .and_then(|p| p.trim().parse::<u32>().ok())
    };
    match ended {
        Ok(Ok(text)) => {
            if let Some(pid) = grandchild(&text) {
                kill_tree_by_pid(pid);
            }
            assert!(grandchild(&text).is_some(), "the parent said: {text}");
        }
        other => {
            panic!("the pipe did not reach its end while the grandchild ran: {other:?}");
        }
    }
}

/// The parent of the test above: it starts a detached child and exits.
#[cfg(windows)]
#[test]
#[ignore = "run only as the child of std_handles_do_not_reach_a_detached_grandchild"]
#[expect(
    clippy::zombie_processes,
    reason = "the grandchild must outlive this process; the test above ends it"
)]
fn a_parent_that_leaves_a_detached_child() {
    use std::os::windows::process::CommandExt;
    use std::process::Stdio;
    const DETACHED_PROCESS: u32 = 0x0000_0008;

    keep_std_handles_from_children();
    let child = Command::new(std::env::current_exe().expect("current_exe"))
        .args(["--exact", "tests::a_child_that_waits_a_minute", "--ignored"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(DETACHED_PROCESS)
        .spawn()
        .expect("spawn the grandchild");
    println!("grandchild pid {}", child.id());
}

/// The grandchild: it waits until it is ended.
#[cfg(windows)]
#[test]
#[ignore = "run only as the grandchild of std_handles_do_not_reach_a_detached_grandchild"]
fn a_child_that_waits_a_minute() {
    std::thread::sleep(std::time::Duration::from_secs(60));
}
