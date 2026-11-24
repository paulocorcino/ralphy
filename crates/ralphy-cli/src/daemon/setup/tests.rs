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
    assert_eq!(avatar, AVATARS[2].to_owned());
}

#[test]
fn empty_name_accepts_suggestion() {
    let input = Cursor::new(b"\n1\n".to_vec());
    let mut out: Vec<u8> = Vec::new();
    let (name, avatar) = baptize_console(input, &mut out, "anvil").unwrap();
    assert_eq!(name, "anvil");
    assert_eq!(avatar, AVATARS[0].to_owned());
}

#[test]
fn totp_enrolment_is_mint_once_and_shows_secret_once() {
    let dir = tempfile::tempdir().unwrap();
    let seed_path = dir.path().join("daemon-totp");

    let mut out1: Vec<u8> = Vec::new();
    let mut skip = Cursor::new(b"\n".to_vec());
    let first = enroll_totp(&mut skip, &mut out1, &seed_path, "anvil").unwrap();
    assert_eq!(first, Authenticator::Skipped, "first enrol mints and asks");
    let printed1 = String::from_utf8(out1).unwrap();
    let seed = totp::load_seed_from(&seed_path).unwrap().unwrap();
    assert!(printed1.contains(&seed.secret_base32()), "got: {printed1}");
    assert!(printed1.contains("otpauth://"), "got: {printed1}");

    let mut out2: Vec<u8> = Vec::new();
    let mut none = Cursor::new(Vec::new());
    let second = enroll_totp(&mut none, &mut out2, &seed_path, "anvil").unwrap();
    assert_eq!(
        second,
        Authenticator::AlreadySet,
        "a later setup does not re-mint"
    );
    let printed2 = String::from_utf8(out2).unwrap();
    assert!(!printed2.contains(&seed.secret_base32()), "got: {printed2}");
}

/// The RFC 6238 test seed; its 6-digit code at unix time 59 is 287082.
fn rfc_seed() -> totp::Seed {
    totp::Seed::from_bytes(b"12345678901234567890".to_vec())
}

fn run_confirm(input: &[u8]) -> (Result<Authenticator>, String) {
    let mut out: Vec<u8> = Vec::new();
    let mut lines = Cursor::new(input.to_vec());
    let result = confirm_code(&mut lines, &mut out, &rfc_seed(), || 59);
    (result, String::from_utf8(out).unwrap())
}

#[test]
fn the_right_code_confirms_the_authenticator() {
    let (result, printed) = run_confirm(b"287082\n");
    assert_eq!(result.unwrap(), Authenticator::Confirmed, "got: {printed}");
}

#[test]
fn a_wrong_code_asks_again_then_the_right_one_confirms() {
    let (result, printed) = run_confirm(b"000000\n287082\n");
    assert_eq!(result.unwrap(), Authenticator::Confirmed, "got: {printed}");
    assert!(printed.contains("not right"), "got: {printed}");
}

#[test]
fn a_blank_line_or_three_wrong_codes_skip_and_keep_the_qr_valid() {
    let (blank, printed) = run_confirm(b"\n");
    assert_eq!(blank.unwrap(), Authenticator::Skipped);
    assert!(printed.contains("stays valid"), "got: {printed}");
    let (wrong, printed) = run_confirm(b"1\n2\n3\n");
    assert_eq!(wrong.unwrap(), Authenticator::Skipped, "got: {printed}");
}

#[test]
fn the_code_check_ends_on_end_of_input() {
    let (result, _) = run_confirm(b"000000\n");
    assert!(result.is_err(), "end of input must not loop");
}

fn run_password(input: &[u8], path: &Path) -> (bool, String) {
    let mut lines = Cursor::new(input.to_vec());
    let mut read = || read_line(&mut lines);
    let mut out: Vec<u8> = Vec::new();
    let set = enroll_password(&mut read, &mut out, path).unwrap();
    (set, String::from_utf8(out).unwrap())
}

#[test]
fn password_enrolment_skips_on_blank_and_saves_a_confirmed_password() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("daemon-password");

    let (set, printed) = run_password(b"\n", &path);
    assert!(!set && printed.contains("skipped"), "got: {printed}");
    assert!(password::load_from(&path).unwrap().is_none(), "blank skips");

    let (set, _) = run_password(b"hunter2\nhunter2\n", &path);
    assert!(set);
    let saved = password::load_from(&path)
        .unwrap()
        .expect("a password was saved");
    assert!(saved.verify("hunter2"), "the saved hash verifies the input");
}

#[test]
fn password_mismatch_asks_again_and_saves_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("daemon-password");
    let (set, printed) = run_password(b"hunter2\nhunter3\n\n", &path);
    assert!(!set, "got: {printed}");
    assert!(printed.contains("not the same"), "got: {printed}");
    assert!(
        password::load_from(&path).unwrap().is_none(),
        "a mismatch saves nothing"
    );
}

#[test]
fn the_steps_are_numbered_in_order() {
    let dir = tempfile::tempdir().unwrap();
    let mut out: Vec<u8> = Vec::new();
    baptize_console(Cursor::new(b"anvil\n1\n".to_vec()), &mut out, "anvil").unwrap();
    let mut code = Cursor::new(b"\n".to_vec());
    enroll_totp(&mut code, &mut out, &dir.path().join("s"), "anvil").unwrap();
    let mut lines = Cursor::new(b"\n".to_vec());
    enroll_password(
        &mut || read_line(&mut lines),
        &mut out,
        &dir.path().join("p"),
    )
    .unwrap();
    let printed = String::from_utf8(out).unwrap();
    println!("{printed}");
    let steps: Vec<&str> = printed.lines().filter(|l| l.starts_with("Step ")).collect();
    assert_eq!(
        steps,
        [
            "Step 1 of 4: Name",
            "Step 2 of 4: Avatar",
            "Step 3 of 4: Authenticator app",
            "Step 4 of 4: Login password (optional)",
        ]
    );
}
