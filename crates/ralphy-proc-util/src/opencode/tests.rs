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
