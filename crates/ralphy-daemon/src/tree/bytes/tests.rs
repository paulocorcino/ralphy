use super::*;
use std::fs;

const PNG: &[u8] = b"\x89PNG\r\n\x1a\n";

fn whole() -> ByteRange {
    ByteRange {
        start: 0,
        end: None,
    }
}

fn png_of(len: usize) -> Vec<u8> {
    let mut bytes = PNG.to_vec();
    bytes.extend((0..len - PNG.len()).map(|i| u8::try_from(i % 251).expect("below 251")));
    bytes
}

#[test]
fn bytes_that_disagree_with_the_extension_are_not_found() {
    let dir = tempfile::tempdir().expect("tempdir");
    fs::write(dir.path().join("a.png"), b"<html>not a png</html>").expect("write");
    fs::write(dir.path().join("b.pdf"), PNG).expect("write");
    fs::write(dir.path().join("c.txt"), b"plain text").expect("write");
    for name in ["a.png", "b.pdf", "c.txt"] {
        assert_eq!(
            read_range(dir.path(), name, whole()),
            Err(BytesError::NotFound)
        );
    }
    fs::write(dir.path().join("ok.png"), PNG).expect("write");
    let ok = read_range(dir.path(), "ok.png", whole()).expect("a real png");
    assert_eq!(ok.media_type, "image/png");
}

#[test]
fn a_range_reads_only_its_bytes() {
    let dir = tempfile::tempdir().expect("tempdir");
    let file = png_of(100);
    fs::write(dir.path().join("a.png"), &file).expect("write");
    let range = ByteRange {
        start: 10,
        end: Some(19),
    };
    let got = read_range(dir.path(), "a.png", range).expect("range");
    assert_eq!(got.bytes, file[10..20]);
    assert_eq!((got.start, got.total), (10, 100));
}

#[test]
fn an_open_range_stops_at_the_answer_cap() {
    let dir = tempfile::tempdir().expect("tempdir");
    let size = usize::try_from(MAX_ANSWER_BYTES).expect("fits") + 1000;
    fs::write(
        dir.path().join("a.pdf"),
        [b"%PDF-".to_vec(), vec![0; size]].concat(),
    )
    .expect("write");
    let got = read_range(dir.path(), "a.pdf", whole()).expect("range");
    assert_eq!(
        u64::try_from(got.bytes.len()).expect("fits"),
        MAX_ANSWER_BYTES
    );
    assert_eq!(got.total, u64::try_from(size).expect("fits") + 5);
}

#[test]
fn a_path_outside_the_checkout_is_not_found() {
    let outer = tempfile::tempdir().expect("tempdir");
    let root = outer.path().join("repo");
    fs::create_dir(&root).expect("mkdir");
    fs::write(outer.path().join("secret.png"), PNG).expect("write");
    for rel in ["../secret.png", "missing.png"] {
        assert_eq!(read_range(&root, rel, whole()), Err(BytesError::NotFound));
    }
}

fn zip_head(extra: &[u8]) -> Vec<u8> {
    let mut bytes = b"PK\x03\x04".to_vec();
    bytes.resize(30, 0);
    bytes.extend_from_slice(extra);
    bytes
}

#[test]
fn a_sheet_over_its_cap_is_too_large() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut big = zip_head(b"");
    big.resize(usize::try_from(MAX_SHEET_BYTES).expect("fits") + 1, 0);
    fs::write(dir.path().join("big.xlsx"), &big).expect("write");
    assert_eq!(
        read_range(dir.path(), "big.xlsx", whole()),
        Err(BytesError::TooLarge)
    );
    big.pop();
    fs::write(dir.path().join("fit.xlsx"), &big).expect("write");
    assert!(read_range(dir.path(), "fit.xlsx", whole()).is_ok());
}

#[test]
fn an_avif_file_is_an_image() {
    let dir = tempfile::tempdir().expect("tempdir");
    fs::write(
        dir.path().join("a.avif"),
        b"\x00\x00\x00\x1cftypavif\x00\x00",
    )
    .expect("write");
    let got = read_range(dir.path(), "a.avif", whole()).expect("avif");
    assert_eq!(got.media_type, "image/avif");
    fs::write(
        dir.path().join("b.avif"),
        b"\x00\x00\x00\x1cftypheic\x00\x00",
    )
    .expect("write");
    assert_eq!(
        read_range(dir.path(), "b.avif", whole()),
        Err(BytesError::NotFound)
    );
}

#[test]
fn an_odt_needs_its_mimetype_entry_and_a_range_past_the_end_is_refused() {
    let dir = tempfile::tempdir().expect("tempdir");
    let odt = zip_head(&[b"mimetype".as_slice(), ODT_MIME].concat());
    fs::write(dir.path().join("a.odt"), &odt).expect("write");
    fs::write(dir.path().join("b.odt"), zip_head(b"other-entry")).expect("write");
    assert!(read_range(dir.path(), "a.odt", whole()).is_ok());
    assert_eq!(
        read_range(dir.path(), "b.odt", whole()),
        Err(BytesError::NotFound)
    );
    let past = ByteRange {
        start: u64::try_from(odt.len()).expect("fits"),
        end: None,
    };
    assert_eq!(
        read_range(dir.path(), "a.odt", past),
        Err(BytesError::Unsatisfiable)
    );
}
