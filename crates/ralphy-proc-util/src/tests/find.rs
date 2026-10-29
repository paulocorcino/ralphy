use super::*;

/// The file name a bare `tool` has on this platform: Windows runs only a
/// PATHEXT extension.
fn tool_file() -> &'static str {
    if cfg!(windows) {
        "tool.exe"
    } else {
        "tool"
    }
}

#[test]
fn the_first_search_dir_that_holds_the_program_wins() {
    let first = tempfile::tempdir().unwrap();
    let second = tempfile::tempdir().unwrap();
    for dir in [first.path(), second.path()] {
        let file = dir.join(tool_file());
        fs::write(&file, b"x").unwrap();
        mark_executable(&file);
    }
    let path_var = std::env::join_paths([first.path(), second.path()]).unwrap();

    let got = find_program("tool", Some(path_var), Some(".EXE".into()))
        .expect("both dirs hold the program");

    assert_eq!(got.parent(), Some(first.path()), "{got:?}");
}

/// With `PATHEXT` unset, Windows falls back to `.EXE`.
#[test]
#[cfg(windows)]
fn an_unset_pathext_still_finds_an_exe() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("tool.exe"), b"x").unwrap();

    let got = find_program("tool", Some(dir.path().as_os_str().to_owned()), None)
        .expect("tool.exe is found without PATHEXT");

    assert!(
        got.file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.eq_ignore_ascii_case("tool.exe")),
        "{got:?}"
    );
}

/// A name that already holds a dot is not an extension: `python3.11` + `.EXE`.
#[test]
#[cfg(windows)]
fn a_name_with_a_dot_keeps_it_when_the_extension_is_added() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("python3.11.EXE"), b"x").unwrap();

    let got = find_program(
        "python3.11",
        Some(dir.path().as_os_str().to_owned()),
        Some(".EXE".into()),
    )
    .expect("python3.11.EXE is found");

    assert!(
        got.file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.eq_ignore_ascii_case("python3.11.EXE")),
        "{got:?}"
    );
}
