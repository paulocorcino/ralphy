use super::*;

#[test]
fn map_round_trip_preserves_exact_pairs() {
    let dir = tempfile::tempdir().unwrap();
    let path = session_model_map_path(dir.path());
    let mut map = SessionModelMap::default();
    map.merge([
        ("session-b".into(), "model-b".into()),
        ("session-a".into(), "model-a".into()),
    ]);
    map.persist(&path).unwrap();
    assert_eq!(SessionModelMap::load(&path).unwrap(), map);
    map.merge([("session-c".into(), "model-c".into())]);
    map.persist(&path).unwrap();
    assert_eq!(SessionModelMap::load(&path).unwrap(), map);
    assert_eq!(map.entries().len(), 3);
}

#[test]
fn merge_is_idempotent_and_old_value_wins_conflicts() {
    let mut map = SessionModelMap::default();
    assert_eq!(map.merge([("session-a".into(), "model-a".into())]).added, 1);
    assert_eq!(
        map.merge([
            ("session-a".into(), "model-a".into()),
            ("session-a".into(), "replacement".into()),
            ("session-b".into(), "model-b".into()),
        ]),
        MergeReport {
            added: 1,
            conflicts: vec![ModelConflict {
                session_id: "session-a".into(),
                stored_model: "model-a".into(),
                proposed_model: "replacement".into(),
            }],
        }
    );
    assert_eq!(map.get("session-a"), Some("model-a"));
    assert_eq!(map.entries().len(), 2);
}

#[test]
fn atomic_write_failure_preserves_previous_map_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let path = session_model_map_path(dir.path());
    let original = br#"{"session-a":"model-a"}"#;
    std::fs::write(&path, original).unwrap();
    let mut map = SessionModelMap::load(&path).unwrap();
    map.merge([("session-b".into(), "model-b".into())]);

    let error = map
        .persist_with(&path, |_, _| {
            Err(std::io::Error::other("injected rename failure"))
        })
        .unwrap_err();
    assert!(error.to_string().contains("replacing model recovery map"));
    assert_eq!(std::fs::read(&path).unwrap(), original);
    let leftovers: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .flatten()
        .filter(|entry| entry.file_name().to_string_lossy().ends_with(".tmp"))
        .collect();
    assert!(leftovers.is_empty());
}

fn wait_for(path: &Path) {
    let started = std::time::Instant::now();
    while !path.exists() {
        assert!(
            started.elapsed() < std::time::Duration::from_secs(10),
            "timed out waiting for {}",
            path.display()
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

/// The child half of `separate_process_transactions_are_serialized`. It is
/// ignored so a plain run does not spawn it; the parent runs it with
/// `--ignored` and the env vars it reads.
#[test]
#[ignore = "child process of separate_process_transactions_are_serialized"]
fn locked_merge_child() {
    let Some(root) = std::env::var_os("RALPHY_MODEL_LOCK_CHILD_ROOT") else {
        return;
    };
    let session = std::env::var("RALPHY_MODEL_LOCK_CHILD_SESSION").unwrap();
    let root = PathBuf::from(root);
    if std::env::var_os("RALPHY_MODEL_LOCK_CHILD_TRY").is_some() {
        let file = MapLock::open(&session_model_map_path(&root)).unwrap();
        assert!(
            !try_lock_exclusive(&file).unwrap(),
            "nonblocking lock unexpectedly succeeded"
        );
        std::fs::write(root.join(format!("try-blocked-{session}")), b"blocked").unwrap();
        return;
    }
    let ready = root.join(format!("{session}.ready"));
    let release = root.join(format!("{session}.release"));
    SessionModelMap::merge_persist_locked_with(
        &session_model_map_path(&root),
        [(session.clone(), format!("model-{session}"))],
        || {
            std::fs::write(&ready, b"ready")?;
            wait_for(&release);
            Ok(())
        },
    )
    .unwrap();
}

#[test]
fn separate_process_transactions_are_serialized() {
    let dir = tempfile::tempdir().unwrap();
    let path = session_model_map_path(dir.path());
    let spawn = |session: &str, try_only: bool, contended_marker: Option<&Path>| {
        let mut command = std::process::Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--ignored",
                "--exact",
                "model_recovery::tests::locked_merge_child",
                "--nocapture",
            ])
            .env("RALPHY_MODEL_LOCK_CHILD_ROOT", dir.path())
            .env("RALPHY_MODEL_LOCK_CHILD_SESSION", session);
        if try_only {
            command.env("RALPHY_MODEL_LOCK_CHILD_TRY", "1");
        }
        if let Some(marker) = contended_marker {
            command.env("RALPHY_MODEL_LOCK_CONTENDED_MARKER", marker);
        }
        command.spawn().unwrap()
    };
    let ready_a = dir.path().join("session-a.ready");
    let ready_b = dir.path().join("session-b.ready");
    let contended_b = dir.path().join("session-b.contended");
    let mut first = spawn("session-a", false, None);
    wait_for(&ready_a);
    let mut probe_a = spawn("probe-a", true, None);
    assert!(probe_a.wait().unwrap().success());
    assert!(dir.path().join("try-blocked-probe-a").exists());
    let mut second = spawn("session-b", false, Some(&contended_b));
    wait_for(&contended_b);
    assert!(
        !ready_b.exists(),
        "contended child passed load before release"
    );
    std::fs::write(dir.path().join("session-a.release"), b"release").unwrap();
    assert!(first.wait().unwrap().success());
    wait_for(&ready_b);
    let mut probe_b = spawn("probe-b", true, None);
    assert!(probe_b.wait().unwrap().success());
    assert!(
        dir.path().join("try-blocked-probe-b").exists(),
        "blocking acquisition returned without owning the OS lock"
    );
    std::fs::write(dir.path().join("session-b.release"), b"release").unwrap();
    assert!(second.wait().unwrap().success());

    let map = SessionModelMap::load(&path).unwrap();
    assert_eq!(map.entries().len(), 2);
    assert_eq!(map.get("session-a"), Some("model-session-a"));
    assert_eq!(map.get("session-b"), Some("model-session-b"));
}

#[test]
fn contended_acquire_always_calls_the_blocking_operation() {
    let dir = tempfile::tempdir().unwrap();
    let map_path = session_model_map_path(dir.path());
    let blocked = std::cell::Cell::new(false);

    let file = MapLock::acquire_with(
        &map_path,
        |_| Ok(false),
        |_| {
            blocked.set(true);
            Ok(())
        },
    )
    .unwrap();

    assert!(blocked.get());
    drop(file);
}

#[test]
fn uncontended_acquire_never_calls_the_blocking_operation() {
    let dir = tempfile::tempdir().unwrap();
    let map_path = session_model_map_path(dir.path());
    let file = MapLock::acquire_with(
        &map_path,
        |_| Ok(true),
        |_| panic!("blocking operation called after successful try-lock"),
    )
    .unwrap();

    drop(file);
}
