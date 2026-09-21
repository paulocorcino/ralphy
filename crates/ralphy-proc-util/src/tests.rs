/// The nvm layout that made this necessary: the CLI and its interpreter are
/// siblings in a directory nothing has on `PATH`.
#[test]
fn path_with_dir_prepends_a_missing_directory() {
    use std::path::Path;

    let dir = Path::new("/home/me/.nvm/versions/node/v24.13.0/bin");
    let current = std::env::join_paths(["/usr/bin", "/bin"]).expect("fixture joins");
    let widened = super::path_with_dir(dir, Some(current.as_os_str()))
        .expect("a directory absent from PATH is prepended");

    let entries: Vec<_> = std::env::split_paths(&widened).collect();
    assert_eq!(
        entries.first().map(|p| p.as_path()),
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
    assert!(super::path_with_dir(dir, Some(current.as_os_str())).is_none());
}

/// An empty or absent `PATH` still yields a usable one-entry `PATH`.
#[test]
fn path_with_dir_handles_an_absent_path() {
    use std::path::Path;

    let dir = Path::new("/opt/tools/bin");
    let widened = super::path_with_dir(dir, None).expect("an absent PATH still widens");
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
    super::program_dir_on_path(&mut cmd);

    let path = cmd
        .get_envs()
        .find(|(k, _)| *k == std::ffi::OsStr::new("PATH"))
        .and_then(|(_, v)| v)
        .expect("PATH is set on the command");
    let entries: Vec<_> = std::env::split_paths(path).collect();
    assert_eq!(
        entries.first().map(|p| p.as_path()),
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
    super::program_dir_on_path(&mut cmd);
    assert!(
        cmd.get_envs()
            .all(|(k, _)| k != std::ffi::OsStr::new("PATH")),
        "a bare name must not touch PATH"
    );
}

use super::*;
use std::fs;

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

/// A group kill reaches its target group and stops there — never the caller's.
///
/// `kill_tree_by_pid` used to spend the pgid as *text* on `kill(1)`, which has
/// to guess whether a leading `-` opens an option or names a group. procps-ng
/// guessed wrong and killed the calling process's group, so every
/// `run_headless` teardown SIGKILLed the process that asked for it: locally the
/// test, on CI the runner itself — which then uploaded no log, leaving the gate
/// red with nothing to read.
///
/// Unix-only because process groups are: the Windows arm walks the process
/// table by parent-PID and never had this exposure.
#[cfg(unix)]
#[test]
fn a_group_kill_spares_the_callers_own_group() {
    use std::time::Duration;

    // A *dead* group leader's pid — the shape production actually hits, since
    // the natural-exit path reaps the child before tearing down its tree.
    let mut leader = Command::new("true");
    own_process_group(&mut leader);
    let mut leader = leader.spawn().expect("spawning `true` should succeed");
    let dead_pgid = leader.id();
    leader.wait().expect("reaping `true` should succeed");

    // The witness deliberately stays in OUR process group (no
    // `own_process_group`), so a kill that escapes its target takes it with us.
    let mut bystander = Command::new("sleep")
        .arg("30")
        .spawn()
        .expect("spawning `sleep` should succeed");

    kill_tree_by_pid(dead_pgid);

    // SIGKILL is delivered without the target getting a say, but delivery is
    // still asynchronous — give it a beat before concluding the witness lived.
    std::thread::sleep(Duration::from_millis(200));
    // `try_wait`, not `pid_is_alive`: a killed-but-unreaped child is a zombie,
    // and `kill(pid, 0)` reports a zombie as alive.
    let died = bystander
        .try_wait()
        .expect("polling the bystander should succeed")
        .is_some();

    let _ = bystander.kill();
    let _ = bystander.wait();

    assert!(
        !died,
        "a kill aimed at group {dead_pgid} escaped and took out a bystander \
             sharing the caller's process group"
    );
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
    let _ = fs::remove_dir_all(&tmp);
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

    let _ = fs::remove_dir_all(&tmp);
}

#[test]
fn find_program_resolves_a_windows_cmd_shim() {
    // The defect this guards: an npm CLI present only as `name.cmd` (no
    // `.exe`) must still resolve, since `Command::new("name")` would not find
    // it. On non-Windows there is no PATHEXT, so this asserts the bare-name
    // branch instead.
    let tmp = std::env::temp_dir().join(format!("ralphy-find-cmd-{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(&tmp).unwrap();
    let path_var = tmp.clone().into_os_string();

    if cfg!(windows) {
        let shim = tmp.join("opencode.cmd");
        fs::write(&shim, b"@echo off").unwrap();
        let got = find_program("opencode", Some(path_var), Some(".EXE;.CMD".into()))
            .expect("must resolve the .cmd shim");
        assert!(got.is_file(), "resolved shim must exist: {got:?}");
        assert_eq!(got.file_stem().and_then(|s| s.to_str()), Some("opencode"));
        assert!(
            got.extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("cmd")),
            "must resolve the .cmd extension, not .exe: {got:?}"
        );
    } else {
        let bare = tmp.join("opencode");
        fs::write(&bare, b"#!/bin/sh").unwrap();
        mark_executable(&bare);
        let got = find_program("opencode", Some(path_var), None);
        assert_eq!(got.as_deref(), Some(bare.as_path()));
    }
    let _ = fs::remove_dir_all(&tmp);
}

#[test]
#[cfg(windows)]
fn find_program_skips_extensionless_shim_when_cmd_present() {
    // The exact npm-on-Windows layout: a bare `opencode` shell shim sits next
    // to `opencode.cmd`. The bare file is not a valid Win32 application
    // (os error 193), so the resolver must return the `.cmd`, not the shim.
    let tmp = std::env::temp_dir().join(format!("ralphy-find-pair-{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(&tmp).unwrap();
    fs::write(tmp.join("opencode"), b"#!/bin/sh\n").unwrap();
    fs::write(tmp.join("opencode.cmd"), b"@echo off\n").unwrap();

    let got = find_program(
        "opencode",
        Some(tmp.clone().into_os_string()),
        Some(".EXE;.CMD".into()),
    )
    .expect("must resolve a runnable candidate");
    assert!(
        got.extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("cmd")),
        "must return the .cmd, not the extensionless shim: {got:?}"
    );
    let _ = fs::remove_dir_all(&tmp);
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

/// ADR-0042 D20: git-bash under either standard root is the shape the vendor's
/// classifier accepts; the WSL launcher (a `System32\bash.exe`) and git.exe are
/// not, and pinning `SHELL` to the WSL bash would spawn WSL instead of a POSIX
/// shell on the host.
#[test]
fn is_git_bash_shape_matches_the_vendor_regex() {
    assert!(is_git_bash_shape(Path::new(
        r"C:\Program Files\Git\bin\bash.exe"
    )));
    // Case-insensitive, like the vendor's `/…/i` regex.
    assert!(is_git_bash_shape(Path::new(
        r"C:\Program Files (x86)\Git\bin\BASH.EXE"
    )));
    // The WSL launcher: a bash.exe that is NOT git-bash.
    assert!(!is_git_bash_shape(Path::new(
        r"C:\Windows\System32\bash.exe"
    )));
    // git.exe is not a shell.
    assert!(!is_git_bash_shape(Path::new(
        r"C:\Program Files\Git\cmd\git.exe"
    )));
}

/// D20: the two standard Git-for-Windows roots are probed first, `bin\bash.exe`
/// under each. `is_file()` is OS-independent, so this resolves on every platform.
#[test]
fn locate_git_bash_prefers_the_program_files_roots() {
    for from_x86 in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let want = root.path().join("Git").join("bin").join("bash.exe");
        fs::create_dir_all(want.parent().unwrap()).unwrap();
        fs::write(&want, b"").unwrap();
        let (pf, pf86) = if from_x86 {
            (None, Some(root.path().to_path_buf()))
        } else {
            (Some(root.path().to_path_buf()), None)
        };
        let got = locate_git_bash_with(Some(std::ffi::OsString::new()), None, pf, pf86);
        assert_eq!(got.as_deref(), Some(want.as_path()), "x86={from_x86}");
    }
}

/// No install anywhere resolves to nothing — the caller then leaves `SHELL`
/// alone rather than pinning it to a missing binary.
#[test]
fn locate_git_bash_is_none_when_nothing_is_installed() {
    assert_eq!(
        locate_git_bash_with(Some(std::ffi::OsString::new()), None, None, None),
        None
    );
}

/// D20: a `bash.exe` on `PATH` that is not git-bash (the WSL launcher shape) is
/// rejected, while a git-shaped one is accepted. Windows-only: the `PATH` search
/// keys on `PATHEXT`, which is a no-op off Windows.
#[test]
#[cfg(windows)]
fn locate_git_bash_rejects_a_non_git_bash_on_path() {
    // `is_git_bash_shape` scans the whole path for "git", so a tempdir whose
    // random name happens to spell it (a real CI hit: `.tmpgiT4KM`) makes the
    // fake `System32\bash.exe` below look git-shaped and flakes this test.
    // Re-roll until the root is free of it.
    let base = std::iter::repeat_with(|| tempfile::tempdir().unwrap())
            .take(64)
            .find(|d| {
                !d.path()
                    .to_string_lossy()
                    .to_ascii_lowercase()
                    .contains("git")
            })
            .expect("no \"git\"-free tempdir in 64 tries: the ambient temp root itself must contain \"git\"");

    // A `System32\bash.exe` on PATH — the WSL launcher — must be rejected.
    let sys = base.path().join("System32");
    fs::create_dir_all(&sys).unwrap();
    fs::write(sys.join("bash.exe"), b"").unwrap();
    assert_eq!(
        locate_git_bash_with(
            Some(sys.clone().into_os_string()),
            Some(".EXE".into()),
            None,
            None
        ),
        None,
        "a non-git bash.exe on PATH must be rejected"
    );

    // A git-shaped `Git\bin\bash.exe` on PATH IS accepted.
    let gitbin = base.path().join("Git").join("bin");
    fs::create_dir_all(&gitbin).unwrap();
    let want = gitbin.join("bash.exe");
    fs::write(&want, b"").unwrap();
    let got = locate_git_bash_with(
        Some(gitbin.into_os_string()),
        Some(".EXE".into()),
        None,
        None,
    )
    .expect("a git-shaped bash.exe on PATH must resolve");
    assert!(is_git_bash_shape(&got), "resolved {got:?}");
    assert_eq!(got.file_stem().and_then(|s| s.to_str()), Some("bash"));
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
    let _ = fs::remove_dir_all(&home);
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

    let _ = fs::remove_dir_all(&home);
}

#[test]
fn locate_program_prefers_path_over_local_bin() {
    // When the program is on PATH, that wins — the fallback is only consulted
    // when PATH has nothing.
    let tmp = std::env::temp_dir().join(format!("ralphy-locate-path-{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(&tmp).unwrap();
    let on_path = if cfg!(windows) {
        tmp.join("tool.exe")
    } else {
        tmp.join("tool")
    };
    fs::write(&on_path, b"x").unwrap();
    mark_executable(&on_path);

    let got = locate_program_with(
        "tool",
        Some(tmp.clone().into_os_string()),
        Some(".EXE".into()),
        // A bogus home whose ~/.local/bin doesn't exist — PATH must win anyway.
        Some(tmp.join("nonexistent-home")),
    )
    .expect("PATH hit must win");
    // Compare by parent + stem: on Windows the resolved extension casing follows
    // PATHEXT (`.EXE`) rather than the file's `.exe`, which is harmless.
    assert_eq!(got.parent(), on_path.parent());
    assert_eq!(got.file_stem(), on_path.file_stem());
    let _ = fs::remove_dir_all(&tmp);
}
