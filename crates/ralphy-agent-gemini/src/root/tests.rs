use super::*;

/// D4: the root is per-workspace and persistent, so `ensure` must be a no-op
/// on a root that is already correct — a rewrite every run would churn the
/// installation identity the vendor keys on. It leaves every file as it was,
/// sessions and the installation id included, down to the mtime.
#[test]
fn ensure_is_idempotent() {
    let base = tempfile::tempdir().unwrap();
    let first = ensure(base.path()).unwrap();

    // Exactly one file, in exactly one place.
    let entries: Vec<_> = std::fs::read_dir(first.cli_dir())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(entries, ["settings.json"], "{entries:?}");
    assert_eq!(first.home, base.path().join("gemini-home"));
    assert!(first.settings.starts_with(&first.home));

    let chats = first.cli_dir().join("tmp").join("proj-abc").join("chats");
    std::fs::create_dir_all(&chats).unwrap();
    for i in 0..3 {
        write_session_pair(&chats, i);
    }
    std::fs::write(first.cli_dir().join("installation_id"), b"b54f6a30-stable").unwrap();

    fn snapshot(dir: &Path) -> Vec<(PathBuf, Vec<u8>, std::time::SystemTime)> {
        let mut out = Vec::new();
        for entry in walkdir(dir) {
            let mtime = entry.metadata().unwrap().modified().unwrap();
            let bytes = std::fs::read(entry.path()).unwrap();
            out.push((entry.path(), bytes, mtime));
        }
        out.sort();
        out
    }
    fn walkdir(dir: &Path) -> Vec<std::fs::DirEntry> {
        let mut out = Vec::new();
        for entry in std::fs::read_dir(dir).unwrap() {
            let entry = entry.unwrap();
            if entry.file_type().unwrap().is_dir() {
                out.extend(walkdir(&entry.path()));
            } else {
                out.push(entry);
            }
        }
        out
    }

    let before = snapshot(&first.cli_dir());
    let second = ensure(base.path()).unwrap();
    assert_eq!(first.settings, second.settings);
    assert_eq!(snapshot(&first.cli_dir()), before);
}

/// A truncated or hand-edited settings file is repaired — and anything else
/// the root accumulated across runs (session state, the installation id) is
/// left exactly as it was.
#[test]
fn ensure_restores_a_drifted_settings_file() {
    let base = tempfile::tempdir().unwrap();
    let root = ensure(base.path()).unwrap();
    let want = std::fs::read_to_string(&root.settings).unwrap();

    let sibling = root.cli_dir().join("installation_id");
    std::fs::write(&sibling, b"keep-me").unwrap();
    std::fs::write(&root.settings, b"{ corrupted").unwrap();

    ensure(base.path()).unwrap();
    let restored = std::fs::read_to_string(&root.settings).unwrap();
    assert_eq!(restored, want);
    assert!(
        restored.contains("sessionRetention"),
        "the restored document must still bound session retention: {restored}"
    );
    assert_eq!(
        std::fs::read_to_string(&sibling).unwrap(),
        "keep-me",
        "an unrelated sibling file must survive"
    );
}

/// The two keys Ralphy forces regardless of the operator, and the one it
/// mirrors. An absent auth mode omits the key entirely rather than guessing.
#[test]
fn the_settings_document_forces_the_privacy_and_agents_keys() {
    let doc = settings_document(Some("gemini-api-key"));
    assert_eq!(doc["privacy"]["usageStatisticsEnabled"], json!(false));
    assert_eq!(doc["experimental"]["enableAgents"], json!(false));
    assert_eq!(
        doc["security"]["auth"]["selectedType"],
        json!("gemini-api-key")
    );

    let unknown = settings_document(None);
    assert!(
        unknown.get("security").is_none(),
        "an unknown auth mode must not be guessed: {unknown}"
    );
    assert_eq!(unknown["privacy"]["usageStatisticsEnabled"], json!(false));
}

/// The settings document declares the vendor's own retention bound —
/// belt-and-suspenders alongside Ralphy's own [`prune_sessions`], since the
/// vendor's cleanup is fire-and-forget at startup.
#[test]
fn the_settings_document_bounds_session_retention() {
    let doc = settings_document(None);
    assert_eq!(doc["general"]["sessionRetention"]["enabled"], json!(true));
    assert_eq!(doc["general"]["sessionRetention"]["maxAge"], json!("30d"));
    assert_eq!(doc["general"]["sessionRetention"]["maxCount"], json!(50));
}

/// The auth-mode read is a pointer lookup that fails to `None` on every bad
/// shape — never a panic, never a partial guess.
#[test]
fn the_operator_auth_type_fails_soft() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(operator_auth_type(None), None);
    assert_eq!(operator_auth_type(Some(dir.path())), None, "no file");

    std::fs::write(dir.path().join("settings.json"), b"not json").unwrap();
    assert_eq!(operator_auth_type(Some(dir.path())), None, "bad json");

    std::fs::write(dir.path().join("settings.json"), br#"{"security":{}}"#).unwrap();
    assert_eq!(operator_auth_type(Some(dir.path())), None, "missing key");

    std::fs::write(
        dir.path().join("settings.json"),
        br#"{"security":{"auth":{"selectedType":"vertex-ai"}}}"#,
    )
    .unwrap();
    assert_eq!(
        operator_auth_type(Some(dir.path())).as_deref(),
        Some("vertex-ai")
    );
}

/// Writes `<chats>/session-<i>.json` + `.jsonl`, both stamped `i` seconds
/// after the Unix epoch so mtime order is deterministic across the pair
/// and across the whole synthetic set.
fn write_session_pair(chats: &Path, i: u32) {
    let mtime = std::time::UNIX_EPOCH + std::time::Duration::from_secs(i as u64);
    for ext in ["json", "jsonl"] {
        let path = chats.join(format!("session-{i:02}-{i:02}.{ext}"));
        let file = std::fs::File::create(&path).unwrap();
        file.set_modified(mtime).unwrap();
    }
}

#[test]
fn ensure_prunes_sessions_beyond_the_keep_count() {
    let base = tempfile::tempdir().unwrap();
    let root = ensure(base.path()).unwrap();
    let chats = root.cli_dir().join("tmp").join("proj-abc").join("chats");
    std::fs::create_dir_all(&chats).unwrap();
    for i in 0..60 {
        write_session_pair(&chats, i);
    }

    ensure(base.path()).unwrap();

    let mut stems = std::fs::read_dir(&chats)
        .unwrap()
        .map(|e| {
            e.unwrap()
                .path()
                .file_stem()
                .unwrap()
                .to_string_lossy()
                .into_owned()
        })
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(stems.len(), 50, "{stems:?}");
    for i in 0..10 {
        assert!(
            !stems.contains(&format!("session-{i:02}-{i:02}")),
            "the oldest sessions must be pruned: {stems:?}"
        );
    }
    assert!(stems.remove("session-59-59"));
}

#[test]
fn ensure_leaves_unowned_files_in_the_session_dir_alone() {
    let base = tempfile::tempdir().unwrap();
    let root = ensure(base.path()).unwrap();
    let chats = root.cli_dir().join("tmp").join("proj-abc").join("chats");
    std::fs::create_dir_all(&chats).unwrap();
    for i in 0..60 {
        write_session_pair(&chats, i);
    }
    let notes = chats.join("notes.txt");
    let decoy = chats.join("session-decoy.txt");
    std::fs::write(&notes, b"mine").unwrap();
    std::fs::write(&decoy, b"mine").unwrap();

    ensure(base.path()).unwrap();

    assert_eq!(std::fs::read(&notes).unwrap(), b"mine");
    assert_eq!(std::fs::read(&decoy).unwrap(), b"mine");
}

#[test]
fn session_stems_skips_a_project_dir_with_no_chats_folder() {
    let cli_dir = tempfile::tempdir().unwrap();
    let tmp = cli_dir.path().join("tmp");
    for empty in ["a-empty", "b-empty"] {
        std::fs::create_dir_all(tmp.join(empty)).unwrap();
    }
    let chats = tmp.join("m-proj").join("chats");
    std::fs::create_dir_all(&chats).unwrap();
    write_session_pair(&chats, 1);
    for empty in ["y-empty", "z-empty"] {
        std::fs::create_dir_all(tmp.join(empty)).unwrap();
    }

    let groups = session_stems(cli_dir.path());

    assert_eq!(groups.len(), 1, "{groups:?}");
    assert_eq!(groups[0].1, "session-01-01");
    assert_eq!(groups[0].2.len(), 2, "the json+jsonl pair is one group");
}

#[test]
fn session_stems_ignores_a_file_that_is_not_a_session() {
    let cli_dir = tempfile::tempdir().unwrap();
    let chats = cli_dir.path().join("tmp").join("proj").join("chats");
    std::fs::create_dir_all(&chats).unwrap();
    std::fs::write(chats.join("checkpoint-1.json"), b"{}").unwrap();
    std::fs::write(chats.join("session-2.txt"), b"x").unwrap();

    assert!(session_stems(cli_dir.path()).is_empty());
}

#[test]
fn two_workspaces_get_two_independent_roots() {
    let base_a = tempfile::tempdir().unwrap();
    let base_b = tempfile::tempdir().unwrap();
    let root_a = ensure(base_a.path()).unwrap();
    let root_b = ensure(base_b.path()).unwrap();
    assert_ne!(root_a.home, root_b.home);

    let b_bytes = std::fs::read(&root_b.settings).unwrap();
    let b_mtime = std::fs::metadata(&root_b.settings)
        .unwrap()
        .modified()
        .unwrap();

    std::fs::remove_file(&root_a.settings).unwrap();
    ensure(base_a.path()).unwrap();

    assert!(root_a.settings.exists(), "base A's root must be restored");
    assert_eq!(
        std::fs::read(&root_b.settings).unwrap(),
        b_bytes,
        "base B's settings must be untouched by an ensure() on base A"
    );
    assert_eq!(
        std::fs::metadata(&root_b.settings)
            .unwrap()
            .modified()
            .unwrap(),
        b_mtime,
        "base B's mtime must be untouched by an ensure() on base A"
    );
}

/// D17: the operator's root is reached for the auth POINTER and their policy
/// rules only — no credential file is ever named here.
#[test]
fn the_root_module_names_no_credential_file() {
    let production = crate::tests::production_text(include_str!("../root.rs"));
    for banned in ["oauth_creds", "google_accounts", "keytar", "access_token"] {
        assert!(
            !production.contains(banned),
            "the credential is never read (D17); found {banned}"
        );
    }
}
