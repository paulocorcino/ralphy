use super::*;
use std::fs;

const ID: &str = "0123456789abcdef0123456789abcdef";
const MB: u64 = 1024 * 1024;

fn part(path: &str, offset: u64, total: u64, data: &[u8], on_conflict: OnConflict) -> Part {
    Part {
        id: ID.to_owned(),
        path: path.to_owned(),
        offset,
        total,
        data: data.to_vec(),
        on_conflict,
    }
}

fn send(root: &Path, p: &Part) -> Result<Step, UploadError> {
    accept(root, p, SystemTime::now(), 25 * MB)
}

#[test]
fn two_parts_land_the_whole_file_at_the_target() {
    let root = tempfile::tempdir().unwrap();
    let first = part("a.bin", 0, 6, b"abc", OnConflict::Fail);
    assert_eq!(send(root.path(), &first), Ok(Step::Received(3)));
    assert!(
        !root.path().join("a.bin").exists(),
        "not before the last part"
    );
    let last = part("a.bin", 3, 6, b"def", OnConflict::Fail);
    assert_eq!(
        send(root.path(), &last),
        Ok(Step::Done {
            path: "a.bin".to_owned(),
            placed: Placed::Created
        })
    );
    assert_eq!(fs::read(root.path().join("a.bin")).unwrap(), b"abcdef");
    assert!(
        !root.path().join(DIR).join(format!("{ID}.part")).exists(),
        "the part file moved"
    );
}

#[test]
fn a_part_at_a_wrong_offset_answers_the_confirmed_length() {
    let root = tempfile::tempdir().unwrap();
    send(root.path(), &part("a.bin", 0, 9, b"abc", OnConflict::Fail)).unwrap();
    let ahead = part("a.bin", 6, 9, b"ghi", OnConflict::Fail);
    assert_eq!(
        send(root.path(), &ahead),
        Err(UploadError::Offset { expected: 3 })
    );
    let again = part("a.bin", 0, 9, b"abc", OnConflict::Fail);
    assert_eq!(
        send(root.path(), &again),
        Err(UploadError::Offset { expected: 3 }),
        "a part sent twice is not appended twice"
    );
}

#[test]
fn a_file_over_the_limit_is_refused_at_the_first_part() {
    let root = tempfile::tempdir().unwrap();
    let p = part("big.bin", 0, 2 * MB + 1, b"x", OnConflict::Fail);
    assert_eq!(
        accept(root.path(), &p, SystemTime::now(), 2 * MB),
        Err(UploadError::TooLarge)
    );
    assert!(!root.path().join(DIR).join(format!("{ID}.part")).exists());
}

#[test]
fn a_part_past_total_is_refused() {
    let root = tempfile::tempdir().unwrap();
    let p = part("a.bin", 0, 2, b"abc", OnConflict::Fail);
    assert_eq!(send(root.path(), &p), Err(UploadError::TooLarge));
}

#[test]
fn fail_refuses_an_existing_target_and_keeps_it() {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("a.txt"), b"old").unwrap();
    let p = part("a.txt", 0, 3, b"new", OnConflict::Fail);
    assert_eq!(send(root.path(), &p), Err(UploadError::Conflict));
    assert_eq!(fs::read(root.path().join("a.txt")).unwrap(), b"old");
}

#[test]
fn replace_overwrites_an_existing_file() {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("a.txt"), b"old").unwrap();
    let p = part("a.txt", 0, 3, b"new", OnConflict::Replace);
    assert_eq!(
        send(root.path(), &p),
        Ok(Step::Done {
            path: "a.txt".to_owned(),
            placed: Placed::Replaced
        })
    );
    assert_eq!(fs::read(root.path().join("a.txt")).unwrap(), b"new");
}

#[test]
fn replace_never_replaces_a_directory() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join("docs")).unwrap();
    let p = part("docs", 0, 1, b"x", OnConflict::Replace);
    assert_eq!(send(root.path(), &p), Err(UploadError::Conflict));
    assert!(root.path().join("docs").is_dir());
}

#[test]
fn keep_both_picks_the_first_free_numbered_name() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join("docs")).unwrap();
    fs::write(root.path().join("docs/r.pdf"), b"1").unwrap();
    fs::write(root.path().join("docs/r (1).pdf"), b"2").unwrap();
    let p = part("docs/r.pdf", 0, 1, b"3", OnConflict::KeepBoth);
    assert_eq!(
        send(root.path(), &p),
        Ok(Step::Done {
            path: "docs/r (2).pdf".to_owned(),
            placed: Placed::Created
        })
    );
    assert_eq!(fs::read(root.path().join("docs/r.pdf")).unwrap(), b"1");
    assert_eq!(fs::read(root.path().join("docs/r (2).pdf")).unwrap(), b"3");
}

#[test]
fn copy_name_numbers_before_the_last_extension() {
    assert_eq!(copy_name("r.pdf", 1), "r (1).pdf");
    assert_eq!(copy_name("a.tar.gz", 2), "a.tar (2).gz");
    assert_eq!(copy_name("Makefile", 1), "Makefile (1)");
    assert_eq!(copy_name(".env", 1), ".env (1)");
}

#[test]
fn a_target_inside_git_or_ralphy_is_refused() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join(".git")).unwrap();
    for target in [".git/config", ".ralphy/settings.json", ".GIT./hooks"] {
        let p = part(target, 0, 1, b"x", OnConflict::Replace);
        assert_eq!(
            send(root.path(), &p),
            Err(UploadError::Write(WriteError::Confined)),
            "{target}"
        );
        fs::remove_file(root.path().join(DIR).join(format!("{ID}.part"))).unwrap();
    }
    assert!(!root.path().join(".git/config").exists());
    assert!(!root.path().join(".ralphy/settings.json").exists());
}

#[test]
fn the_first_part_deletes_an_old_part_file_and_keeps_a_new_one() {
    let root = tempfile::tempdir().unwrap();
    let old = "ffffffffffffffffffffffffffffffff";
    let mut p = part("a.bin", 0, 9, b"abc", OnConflict::Fail);
    p.id = old.to_owned();
    send(root.path(), &p).unwrap();
    let old_file = root.path().join(DIR).join(format!("{old}.part"));
    assert!(old_file.exists());

    let soon = SystemTime::now() + Duration::from_secs(60);
    accept(
        root.path(),
        &part("b.bin", 0, 9, b"abc", OnConflict::Fail),
        soon,
        25 * MB,
    )
    .unwrap();
    assert!(old_file.exists(), "a part written a minute ago stays");

    let later = SystemTime::now() + STALE_AFTER + Duration::from_secs(60);
    let mut other = part("c.bin", 0, 9, b"abc", OnConflict::Fail);
    other.id = "11111111111111111111111111111111".to_owned();
    accept(root.path(), &other, later, 25 * MB).unwrap();
    assert!(!old_file.exists(), "a part older than one hour is deleted");
}

#[test]
fn parse_refuses_a_bad_id_and_an_unknown_choice() {
    let good = serde_json::json!({
        "id": ID, "path": "a", "offset": 0, "total": 1, "data": "eA==",
    });
    assert_eq!(parse(&good).unwrap().on_conflict, OnConflict::Fail);
    for (key, value) in [
        ("id", serde_json::json!("../../etc")),
        ("id", serde_json::json!("0123456789ABCDEF0123456789ABCDEF")),
        ("on_conflict", serde_json::json!("merge")),
        ("data", serde_json::json!("!!")),
        ("path", serde_json::json!("")),
    ] {
        let mut bad = good.clone();
        bad[key] = value;
        assert_eq!(parse(&bad).unwrap_err(), UploadError::Invalid, "{key}");
    }
}

#[test]
fn parse_refuses_a_part_over_one_mib() {
    let big = data_encoding::BASE64.encode(&vec![0u8; PART_MAX + 1]);
    let payload = serde_json::json!({
        "id": ID, "path": "a", "offset": 0, "total": 9 * MB, "data": big,
    });
    assert_eq!(parse(&payload).unwrap_err(), UploadError::TooLarge);
}

#[test]
fn the_limit_is_the_setting_capped_at_the_ceiling() {
    assert_eq!(limit_from(None), 25);
    assert_eq!(limit_from(Some(100)), 100);
    assert_eq!(limit_from(Some(5000)), 1024);
    let root = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join(".ralphy")).unwrap();
    fs::write(
        root.path().join(".ralphy/settings.json"),
        r#"{"files":{"upload_max_mb":3}}"#,
    )
    .unwrap();
    assert_eq!(limit_bytes(root.path()), 3 * MB);
}
