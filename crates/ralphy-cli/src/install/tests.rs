use super::*;

#[test]
fn only_an_elevated_run_is_warned() {
    assert_eq!(elevated_warning(false), None);
    let warning = elevated_warning(true).expect("an elevated run is warned");
    assert!(warning.contains("root or as administrator"), "{warning}");
}

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ralphy-install-{}-{tag}", std::process::id()));
    // the scratch directory may be gone
    match std::fs::remove_dir_all(&dir) {
        Ok(()) | Err(_) => {}
    }
    std::fs::create_dir_all(&dir).expect("scratch");
    dir
}

#[test]
fn replacing_parks_the_old_binary_rather_than_deleting_it() {
    // The whole point: Windows cannot delete a running image, so a
    // replacement that started with `remove_file` could never work over a
    // live daemon — which is the ordinary case.
    let dir = scratch("replace");
    let dest = dir.join(binary_name());
    let new = dir.join("staged");
    std::fs::write(&dest, b"old").expect("old");
    std::fs::write(&new, b"new").expect("new");

    let parked = replace_binary(&dest, &new)
        .expect("replace")
        .expect("an existing binary is parked, not deleted");
    assert_eq!(std::fs::read(&dest).expect("read"), b"new");
    assert_eq!(std::fs::read(&parked).expect("read parked"), b"old");
    assert_eq!(parked.file_name().and_then(|n| n.to_str()), {
        let mut expected = binary_name().to_owned();
        expected.push_str(".old");
        Some(expected).as_deref()
    });
    // the scratch directory may be gone
    match std::fs::remove_dir_all(&dir) {
        Ok(()) | Err(_) => {}
    }
}

#[test]
fn a_first_install_has_nothing_to_park() {
    let dir = scratch("fresh");
    let dest = dir.join(binary_name());
    let new = dir.join("staged");
    std::fs::write(&new, b"new").expect("new");
    assert_eq!(replace_binary(&dest, &new).expect("replace"), None);
    assert_eq!(std::fs::read(&dest).expect("read"), b"new");
    // the scratch directory may be gone
    match std::fs::remove_dir_all(&dir) {
        Ok(()) | Err(_) => {}
    }
}

#[test]
fn a_failed_put_restores_the_original() {
    let dir = scratch("restore");
    let dest = dir.join(binary_name());
    std::fs::write(&dest, b"old").expect("old");
    // A source that does not exist: the copy fails after the park.
    let err = replace_binary(&dest, &dir.join("missing")).expect_err("copy must fail");
    assert!(err.to_string().contains("putting the new binary"), "{err}");
    assert_eq!(
        std::fs::read(&dest).expect("read"),
        b"old",
        "a failed replacement must never leave the operator without a binary"
    );
    // the scratch directory may be gone
    match std::fs::remove_dir_all(&dir) {
        Ok(()) | Err(_) => {}
    }
}

#[test]
fn a_park_that_cannot_be_freed_steps_aside_instead_of_failing() {
    // The second consecutive install over a live daemon: the daemon is
    // executing the file the previous install parked, so `.old` can be
    // neither removed nor renamed over. A fixed name failed the install
    // outright — found by doing exactly this on a running daemon.
    let dir = scratch("held");
    let dest = dir.join(binary_name());
    let held = parked_path(&dest);
    std::fs::write(&dest, b"old").expect("old");
    std::fs::write(&held, b"still running").expect("held");

    // Hold the parked file open, which is what a running image does.
    let guard = std::fs::File::open(&held).expect("open");
    let next = parked_path(&dest);
    drop(guard);

    // On a platform that lets a held file be deleted (every Unix), the plain
    // name is reused; on Windows it is not, and a distinct one is chosen.
    assert!(
        next == held || next.to_string_lossy().starts_with(&*held.to_string_lossy()),
        "the park must stay beside the binary: {next:?}"
    );
    let new = dir.join("staged");
    std::fs::write(&new, b"new").expect("new");
    replace_binary(&dest, &new).expect("a held leftover must not fail the install");
    assert_eq!(std::fs::read(&dest).expect("read"), b"new");
    // the scratch directory may be gone
    match std::fs::remove_dir_all(&dir) {
        Ok(()) | Err(_) => {}
    }
}

#[test]
fn a_leftover_park_from_last_time_does_not_block_this_one() {
    let dir = scratch("leftover");
    let dest = dir.join(binary_name());
    std::fs::write(&dest, b"old").expect("old");
    std::fs::write(parked_path(&dest), b"older still").expect("leftover");
    let new = dir.join("staged");
    std::fs::write(&new, b"new").expect("new");

    let parked = replace_binary(&dest, &new)
        .expect("replace")
        .expect("parked");
    assert_eq!(std::fs::read(&dest).expect("read"), b"new");
    assert_eq!(
        std::fs::read(&parked).expect("read parked"),
        b"old",
        "the park holds the binary just replaced, not the one before it"
    );
    // the scratch directory may be gone
    match std::fs::remove_dir_all(&dir) {
        Ok(()) | Err(_) => {}
    }
}

#[test]
fn a_folder_on_path_is_found_with_or_without_a_trailing_separator() {
    let sep = if cfg!(windows) { ";" } else { ":" };
    let base = std::env::temp_dir().join("ralphy-on-path");
    let other = std::env::temp_dir().join("ralphy-elsewhere");
    let with_slash = format!("{}{}", base.display(), std::path::MAIN_SEPARATOR);
    let rows: &[(String, bool)] = &[
        (format!("{}{sep}{}", other.display(), base.display()), true),
        (format!("{}{sep}{with_slash}", other.display()), true),
        (other.display().to_string(), false),
        (String::new(), false),
    ];
    for (path, want) in rows {
        assert_eq!(
            on_path(std::ffi::OsStr::new(path), &base),
            *want,
            "{path:?}"
        );
    }
}

#[test]
fn an_update_adds_the_install_folder_only_when_it_runs_this_binary() {
    let root = scratch("after-update");
    let bin = root.join("bin");
    let on = root.join("on-path");
    let away = root.join("unzipped");
    for d in [&bin, &on, &away] {
        std::fs::create_dir_all(d).expect("dir");
    }
    let put = |dir: &Path| {
        let file = dir.join(binary_name());
        std::fs::write(&file, b"ralphy").expect("binary");
        std::fs::canonicalize(&file).expect("canonical")
    };
    let path = std::ffi::OsString::from(on.as_os_str());

    // The install folder holds the binary, and nothing on PATH runs it.
    let in_bin = put(&bin);
    assert_eq!(path_fix(&in_bin, &path, &bin), PathFix::Add(bin.clone()));

    // A folder on PATH runs it: nothing to do, even with the install folder
    // holding a copy of its own.
    let in_on = put(&on);
    assert_eq!(path_fix(&in_on, &path, &bin), PathFix::Nothing);

    // Neither runs it: the binary was never installed.
    let in_away = put(&away);
    assert_eq!(path_fix(&in_away, &path, &bin), PathFix::Install);

    // A link in the install folder runs the unzipped binary.
    std::fs::remove_file(bin.join(binary_name())).expect("remove");
    if symlink(&away.join(binary_name()), &bin.join(binary_name())).is_ok() {
        assert_eq!(path_fix(&in_away, &path, &bin), PathFix::Add(bin.clone()));
    }
    // the scratch directory may be gone
    match std::fs::remove_dir_all(&root) {
        Ok(()) | Err(_) => {}
    }
}

#[test]
fn a_dangling_symlink_counts_as_occupied() {
    // `exists()` follows the link and answers false; the entry is still in
    // the way of a new one.
    let dir = scratch("dangling");
    let dest = dir.join(binary_name());
    if symlink(&dir.join("nowhere"), &dest).is_err() {
        // Windows without Developer Mode: nothing to assert.
        // the scratch directory may be gone
        match std::fs::remove_dir_all(&dir) {
            Ok(()) | Err(_) => {}
        }
        return;
    }
    assert!(
        !dest.exists(),
        "the target is missing, so exists() is false"
    );
    assert!(occupied(&dest), "but something is there");
    let parked = park_existing(&dest).expect("park").expect("parked");
    assert!(!occupied(&dest), "the way is clear now");
    assert!(parked.symlink_metadata().is_ok());
    // the scratch directory may be gone
    match std::fs::remove_dir_all(&dir) {
        Ok(()) | Err(_) => {}
    }
}
