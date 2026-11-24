use super::*;

#[test]
fn remove_is_idempotent() {
    let reg_dir = tempfile::tempdir().unwrap();
    let registry_path = reg_dir.path().join("repos.toml");
    let mut store = registry::RegistryStore::default();
    store.upsert("owner/repo", "/some/path");
    registry::save_to(&store, &registry_path).unwrap();

    assert!(
        remove_repo_at(&registry_path, "owner/repo").unwrap(),
        "first remove reports true"
    );
    assert!(
        !remove_repo_at(&registry_path, "owner/repo").unwrap(),
        "second remove reports false"
    );
}

#[test]
fn autostart_status_names_a_turned_off_entry_and_an_old_registration() {
    use ralphy_daemon::autostart::AutostartStatus;
    let on = AutostartStatus {
        registered: true,
        ..AutostartStatus::default()
    };
    assert_eq!(autostart_lines(on), vec!["autostart: registered"]);
    let off = AutostartStatus {
        registered: true,
        turned_off: true,
        old_registration: false,
    };
    assert_eq!(
        autostart_lines(off),
        vec!["autostart: turned off in Task Manager (Startup apps)"]
    );
    let old = AutostartStatus {
        old_registration: true,
        ..AutostartStatus::default()
    };
    let lines = autostart_lines(old);
    assert_eq!(lines[0], "autostart: not registered");
    assert!(lines[1].contains("earlier version"), "{lines:?}");
}
