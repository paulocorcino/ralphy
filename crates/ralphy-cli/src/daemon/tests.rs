use super::*;
use std::io::Cursor;

#[test]
fn baptism_refuses_reserved_then_accepts() {
    // "run" is reserved → refused; "anvil" accepted; avatar #3 → AVATARS[2].
    let input = Cursor::new(b"run\nanvil\n3\n".to_vec());
    let mut out: Vec<u8> = Vec::new();
    let (name, avatar) = baptize_console(input, &mut out, "suggested").unwrap();

    let printed = String::from_utf8(out).unwrap();
    assert!(
        printed.contains("run") && printed.to_lowercase().contains("reserved"),
        "the console must show a refusal naming `run`; got: {printed}"
    );
    assert_eq!(name, "anvil");
    assert_eq!(avatar, AVATARS[2].to_string());
}

#[test]
fn empty_name_accepts_suggestion() {
    let input = Cursor::new(b"\n1\n".to_vec());
    let mut out: Vec<u8> = Vec::new();
    let (name, avatar) = baptize_console(input, &mut out, "anvil").unwrap();
    assert_eq!(name, "anvil");
    assert_eq!(avatar, AVATARS[0].to_string());
}

#[test]
fn totp_enrolment_is_mint_once_and_shows_secret_once() {
    // Path-explicit (temp seed path, no env mutation), mirroring
    // `register_repo_at`. The secret is printed on the first mint and NEVER on
    // a later setup — the mint-once display invariant (AC1).
    let dir = tempfile::tempdir().unwrap();
    let seed_path = dir.path().join("daemon-totp");

    let mut out1: Vec<u8> = Vec::new();
    assert!(
        enroll_totp(&mut out1, &seed_path, "anvil").unwrap(),
        "first enrol mints"
    );
    let printed1 = String::from_utf8(out1).unwrap();
    let seed = totp::load_seed_from(&seed_path).unwrap().unwrap();
    assert!(
        printed1.contains(&seed.secret_base32()),
        "first mint prints the base32 secret; got: {printed1}"
    );
    assert!(
        printed1.contains("otpauth://"),
        "first mint prints the otpauth URI; got: {printed1}"
    );

    let mut out2: Vec<u8> = Vec::new();
    assert!(
        !enroll_totp(&mut out2, &seed_path, "anvil").unwrap(),
        "second enrol does not re-mint"
    );
    let printed2 = String::from_utf8(out2).unwrap();
    assert!(
        !printed2.contains(&seed.secret_base32()),
        "the secret must NOT re-print on a later setup; got: {printed2}"
    );
    assert!(
        printed2.contains("already enrolled"),
        "a re-enrol reports already enrolled; got: {printed2}"
    );
}

#[test]
fn password_enrolment_skips_on_blank_and_saves_on_input() {
    use std::io::Cursor;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("daemon-password");

    // Blank line → skipped, nothing written.
    let mut blank = Cursor::new(b"\n".to_vec());
    let mut out: Vec<u8> = Vec::new();
    enroll_password(&mut blank, &mut out, &path).unwrap();
    assert!(password::load_from(&path).unwrap().is_none(), "blank skips");
    assert!(String::from_utf8(out).unwrap().contains("skipped"));

    // A non-blank line → hashed and saved; the hash verifies.
    let mut typed = Cursor::new(b"hunter2\n".to_vec());
    let mut out: Vec<u8> = Vec::new();
    enroll_password(&mut typed, &mut out, &path).unwrap();
    let saved = password::load_from(&path)
        .unwrap()
        .expect("a password was saved");
    assert!(saved.verify("hunter2"), "the saved hash verifies the input");
    assert!(String::from_utf8(out).unwrap().contains("set"));
}

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
