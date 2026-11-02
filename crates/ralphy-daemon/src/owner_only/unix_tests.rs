use std::os::unix::fs::PermissionsExt;
use std::path::Path;

fn mode(path: &Path) -> u32 {
    std::fs::metadata(path)
        .expect("the file under test exists")
        .permissions()
        .mode()
        & 0o777
}

/// The secret file never exists with a wider mode than `0o600`: the
/// temporary file has that mode the instant it is created, before a byte
/// is written. The final file keeps it, the store directory is `0o700`,
/// and no temporary file is left behind.
#[test]
fn write_owner_only_creates_the_file_0600() {
    let dir = tempfile::tempdir().unwrap();
    let store = dir.path().join("store");
    super::create_owner_only_dir(&store).unwrap();
    assert_eq!(mode(&store), 0o700);
    let path = store.join("secret.toml");
    let mut at_creation = None;
    super::write_owner_only_probed(&path, b"one", |tmp| at_creation = Some(mode(tmp))).unwrap();
    assert_eq!(at_creation, Some(0o600), "the temporary file at creation");
    assert_eq!(mode(&path), 0o600);
    assert!(super::is_owner_only(&path).unwrap());
    super::write_owner_only(&path, b"two").unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), b"two");
    let names: Vec<String> = std::fs::read_dir(&store)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, ["secret.toml"]);
}
