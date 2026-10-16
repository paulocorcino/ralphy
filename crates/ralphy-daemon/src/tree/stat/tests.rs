use std::time::{Duration, UNIX_EPOCH};

use super::*;
use crate::textcodec::DEFAULT_FALLBACK;

const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR";

fn stat_of(root: &Path, rel: &str) -> Result<Stat, ReadError> {
    stat(root, rel, DEFAULT_FALLBACK)
}

fn type_of(root: &Path, rel: &str) -> FileType {
    stat_of(root, rel)
        .expect("the path exists")
        .file_type
        .expect("a file has a type")
}

#[test]
fn a_file_answers_its_size_and_its_name() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("src")).unwrap();
    std::fs::write(dir.path().join("src/main.rs"), b"fn main() {}").unwrap();

    let got = stat_of(dir.path(), "src/main.rs").unwrap();

    assert_eq!(got.name, "main.rs");
    assert_eq!(got.kind, Kind::File);
    assert_eq!(got.size, Some(12));
    assert!(!got.readonly);
}

#[test]
fn a_folder_answers_no_size_and_no_type() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("src")).unwrap();

    let got = stat_of(dir.path(), "src").unwrap();

    assert_eq!(got.kind, Kind::Folder);
    assert_eq!(got.size, None);
    assert_eq!(got.file_type, None);
}

#[test]
fn the_root_is_the_folder_itself_and_is_never_ignored() {
    let dir = tempfile::tempdir().unwrap();
    // A rule that would match the root's own name, if the root were judged.
    let name = dir
        .path()
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    std::fs::write(dir.path().join(".gitignore"), format!("{name}\n*\n")).unwrap();

    let got = stat_of(dir.path(), "").unwrap();

    assert_eq!(got.kind, Kind::Folder);
    assert_eq!(got.name, name);
    assert!(!got.ignored);
}

#[test]
fn modified_is_milliseconds_since_the_epoch() {
    let dir = tempfile::tempdir().unwrap();
    let file = std::fs::File::create(dir.path().join("a.txt")).unwrap();
    file.set_modified(UNIX_EPOCH + Duration::from_millis(1_700_000_000_123))
        .unwrap();
    drop(file);

    let got = stat_of(dir.path(), "a.txt").unwrap();

    assert_eq!(got.modified, Some(1_700_000_000_123));
}

#[test]
fn a_read_only_file_says_so() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("locked.txt");
    std::fs::write(&path, b"x").unwrap();
    let mut perms = std::fs::metadata(&path).unwrap().permissions();
    perms.set_readonly(true);
    std::fs::set_permissions(&path, perms).unwrap();

    let got = stat_of(dir.path(), "locked.txt").unwrap();

    assert!(got.readonly);
    // The control: a writable file next to it says the opposite.
    std::fs::write(dir.path().join("open.txt"), b"x").unwrap();
    assert!(!stat_of(dir.path(), "open.txt").unwrap().readonly);
}

#[test]
fn the_ignored_mark_follows_the_rules_of_the_tree() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join(".gitignore"), b"build/\n*.log\n").unwrap();
    std::fs::create_dir(dir.path().join("build")).unwrap();
    std::fs::write(dir.path().join("build/out.txt"), b"x").unwrap();
    std::fs::write(dir.path().join("run.log"), b"x").unwrap();
    std::fs::write(dir.path().join("kept.txt"), b"x").unwrap();

    let marks: Vec<bool> = ["build", "build/out.txt", "run.log", "kept.txt"]
        .iter()
        .map(|rel| stat_of(dir.path(), rel).unwrap().ignored)
        .collect();

    // A folder, a file inside an ignored folder, a pattern match, and the control.
    assert_eq!(marks, [true, true, true, false]);
}

#[test]
fn utf8_text_answers_its_encoding_and_no_bom() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.txt"), "olá").unwrap();

    assert_eq!(
        type_of(dir.path(), "a.txt"),
        FileType {
            encoding: Some("UTF-8"),
            bom: Some(false),
            ..FileType::of(Class::Text)
        }
    );
}

#[test]
fn a_byte_order_mark_is_reported() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("bom.txt"), b"\xEF\xBB\xBFol\xC3\xA1").unwrap();

    let got = type_of(dir.path(), "bom.txt");

    assert_eq!((got.class, got.bom), (Class::Text, Some(true)));
}

#[test]
fn a_binary_file_is_binary() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("bin.dat"), [0x00, 0x01, 0x02]).unwrap();

    assert_eq!(type_of(dir.path(), "bin.dat"), FileType::of(Class::Binary));
}

#[test]
fn a_file_over_the_read_limit_is_too_large() {
    let dir = tempfile::tempdir().unwrap();
    let big = vec![b'a'; usize::try_from(crate::tree::MAX_READ_BYTES).unwrap() + 1];
    std::fs::write(dir.path().join("big.txt"), big).unwrap();

    assert_eq!(
        type_of(dir.path(), "big.txt"),
        FileType::of(Class::TooLarge)
    );
}

#[test]
fn an_image_is_named_by_its_extension() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("logo.png"), PNG).unwrap();

    assert_eq!(
        type_of(dir.path(), "logo.png"),
        FileType {
            image: Some("png"),
            ..FileType::of(Class::Image)
        }
    );
}

#[test]
fn bytes_that_belie_the_image_extension_are_judged_as_text() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("evil.png"), b"<html><script>x</script>").unwrap();

    assert_eq!(type_of(dir.path(), "evil.png").class, Class::Text);
}

#[test]
fn a_missing_path_and_an_escape_are_both_not_found() {
    let outer = tempfile::tempdir().unwrap();
    std::fs::write(outer.path().join("secret"), b"token").unwrap();
    let root = outer.path().join("repo");
    std::fs::create_dir(&root).unwrap();

    assert_eq!(stat_of(&root, "nope.txt"), Err(ReadError::NotFound));
    // The control: `secret` exists, so only confinement can refuse it. A file
    // is also refused by its type reader; a folder (`..` is `outer`, which
    // exists) is refused by the stat alone.
    assert!(outer.path().join("secret").exists());
    assert_eq!(stat_of(&root, "../secret"), Err(ReadError::NotFound));
    assert_eq!(stat_of(&root, ".."), Err(ReadError::NotFound));
}
