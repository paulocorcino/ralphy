use super::*;

use std::path::Path;

/// Write an empty file that the OS would run: on Unix it gets the execute bit.
fn put_program(path: &Path) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, b"").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
}

fn file_name(stem: &str) -> String {
    if cfg!(windows) {
        format!("{stem}.exe")
    } else {
        stem.to_owned()
    }
}

fn installer_copy(home: &Path) -> PathBuf {
    home.join(".opencode")
        .join("bin")
        .join(file_name("opencode"))
}

#[test]
fn the_installer_folder_is_found_when_path_has_none() {
    let tmp = tempfile::tempdir().unwrap();
    let (home, empty_path) = (tmp.path().join("home"), tmp.path().join("on-path"));
    std::fs::create_dir_all(&empty_path).unwrap();
    let installed = installer_copy(&home);
    put_program(&installed);

    let found = locate_opencode_with(
        Some(empty_path.into_os_string()),
        Some(".exe;.cmd".into()),
        Some(home),
    );
    assert_eq!(found, Some(installed));
}

#[test]
fn a_path_copy_wins_over_the_installer_folder() {
    let tmp = tempfile::tempdir().unwrap();
    let (home, on_path) = (tmp.path().join("home"), tmp.path().join("on-path"));
    let path_copy = on_path.join(file_name("opencode"));
    put_program(&path_copy);
    put_program(&installer_copy(&home));

    let found = locate_opencode_with(
        Some(on_path.into_os_string()),
        Some(".exe;.cmd".into()),
        Some(home),
    );
    assert_eq!(found, Some(path_copy));
}

#[test]
fn the_opencode2_shim_is_never_found() {
    let tmp = tempfile::tempdir().unwrap();
    let (home, empty_path) = (tmp.path().join("home"), tmp.path().join("on-path"));
    std::fs::create_dir_all(&empty_path).unwrap();
    let shim = if cfg!(windows) {
        "opencode2.cmd"
    } else {
        "opencode2"
    };
    put_program(&home.join(".opencode").join("bin").join(shim));

    let found = locate_opencode_with(
        Some(empty_path.into_os_string()),
        Some(".exe;.cmd".into()),
        Some(home),
    );
    assert_eq!(found, None);
}

#[test]
fn the_program_is_the_located_path_else_the_bare_name() {
    assert_eq!(opencode_program_with(None), OsString::from("opencode"));
    let found = PathBuf::from("/x/opencode");
    assert_eq!(
        opencode_program_with(Some(found.clone())),
        found.into_os_string()
    );
}

#[test]
fn the_version_is_read_from_both_outputs() {
    let cases = [
        ("1.19.0", OpenCodeMajor::V1, "1.19.0"),
        ("1.16.2", OpenCodeMajor::V1, "1.16.2"),
        ("opencode v2.3.2", OpenCodeMajor::V2, "2.3.2"),
        ("opencode v2.4.1", OpenCodeMajor::V2, "2.4.1"),
    ];
    for (line, major, text) in cases {
        let got = parse_opencode_version(line).unwrap();
        assert_eq!((got.major, got.text.as_str()), (major, text), "{line}");
    }
}

#[test]
fn an_unreadable_version_is_refused() {
    for line in ["", "opencode", "vX"] {
        assert!(
            matches!(
                parse_opencode_version(line),
                Err(OpenCodeVersionError::Unreadable { .. })
            ),
            "{line:?}"
        );
    }
    let missing = read_opencode_version(Path::new("/no/such/opencode-program")).unwrap_err();
    assert!(matches!(missing, OpenCodeVersionError::Unreadable { .. }));
    assert!(
        missing.to_string().contains("opencode-program"),
        "{missing}"
    );
}

#[test]
fn a_third_major_is_refused_with_the_text_it_read() {
    let err = parse_opencode_version("3.0.0").unwrap_err();
    assert_eq!(
        err,
        OpenCodeVersionError::Unsupported {
            text: "3.0.0".into()
        }
    );
    let message = err.to_string();
    for part in ["3.0.0", "1.x", "2.x"] {
        assert!(message.contains(part), "{message}");
    }
}

#[test]
fn the_store_follows_xdg_data_home_when_it_is_absolute() {
    let home = std::env::temp_dir().join("home");
    let xdg = std::env::temp_dir().join("xdg");
    let store = |x: Option<&std::path::Path>| {
        opencode_db_path_with(x.map(|p| p.as_os_str().to_owned()), Some(home.clone()))
    };
    let in_xdg = xdg.join("opencode").join("opencode.db");
    let in_home = home
        .join(".local")
        .join("share")
        .join("opencode")
        .join("opencode.db");
    assert_eq!(store(Some(&xdg)), Some(in_xdg));
    assert_eq!(store(None), Some(in_home.clone()));
    assert_eq!(
        store(Some(std::path::Path::new("relative"))),
        Some(in_home.clone())
    );
    assert_eq!(store(Some(std::path::Path::new(""))), Some(in_home));
    assert_eq!(opencode_db_path_with(None, None), None);
}
