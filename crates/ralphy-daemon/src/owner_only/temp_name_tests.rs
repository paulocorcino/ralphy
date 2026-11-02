/// Two writes in one process never share a temporary file, so one cannot
/// remove or rename the other's half-written bytes.
#[test]
fn each_write_has_its_own_temporary_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("secret.toml");
    let mut seen = Vec::new();
    for bytes in [b"one", b"two"] {
        super::write_owner_only_probed(&path, bytes, |tmp| seen.push(tmp.to_path_buf())).unwrap();
    }
    assert_ne!(seen[0], seen[1]);
    assert_eq!(std::fs::read(&path).unwrap(), b"two");
}
