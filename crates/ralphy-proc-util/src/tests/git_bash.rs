use super::*;

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
        .expect(
            "no \"git\"-free tempdir in 64 tries: \
             the ambient temp root itself must contain \"git\"",
        );

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
