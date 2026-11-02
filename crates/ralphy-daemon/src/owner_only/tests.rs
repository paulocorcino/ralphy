use std::path::Path;

pub(crate) use super::win::Dacl;

pub(crate) fn read_dacl(path: &Path) -> Dacl {
    super::win::read_dacl(path).expect("reading the DACL")
}

/// The written file carries the owner-only DACL, a rewrite keeps it, and
/// no temporary file is left beside it.
#[test]
fn write_owner_only_is_protected() {
    let dir = tempfile::tempdir().unwrap();
    let store = dir.path().join("store");
    super::create_owner_only_dir(&store).unwrap();
    let path = store.join("secret.toml");
    let mut seen = None;
    super::write_owner_only_probed(&path, b"one", |tmp| seen = Some(read_dacl(tmp))).unwrap();
    let owner_only = Dacl {
        protected: true,
        ace_count: 1,
        first_is_user_full_access: true,
    };
    assert_eq!(seen, Some(owner_only), "the empty temporary file");
    super::write_owner_only(&path, b"two").unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), b"two");
    assert!(super::is_owner_only(&path).unwrap());
    let plain = store.join("plain.txt");
    std::fs::write(&plain, "x").unwrap();
    assert!(!super::is_owner_only(&plain).unwrap());
    let names: Vec<String> = std::fs::read_dir(&store)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert!(!names.iter().any(|n| n.contains(".tmp-")), "{names:?}");
}

#[test]
fn daemon_files_are_owner_only_with_inheritance_removed() {
    let dir = tempfile::tempdir().unwrap();

    let control = dir.path().join("plain.txt");
    std::fs::write(&control, "x").unwrap();
    assert!(
        !read_dacl(&control).protected,
        "a plain write inherits its folder's ACL, so the reader can tell the two apart"
    );

    let token = dir.path().join("daemon-token");
    crate::auth::save_token_to("tok", &token).unwrap();
    let repos = dir.path().join("repos.toml");
    crate::registry::save_to(&crate::registry::RegistryStore::default(), &repos).unwrap();
    let identity = dir.path().join("daemon.toml");
    crate::identity::save_to(
        &crate::identity::Identity {
            id: ulid::Ulid::nil(),
            name: "anvil".into(),
            avatar: "🐙".into(),
        },
        &identity,
    )
    .unwrap();

    for file in [&token, &repos, &identity] {
        assert_eq!(
            read_dacl(file),
            Dacl {
                protected: true,
                ace_count: 1,
                first_is_user_full_access: true,
            },
            "{} must grant the current user only, with inheritance removed",
            file.display()
        );
    }
}
