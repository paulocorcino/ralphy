use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;

use super::*;

fn write_zip(path: &Path, files: &[(&str, &[u8])]) {
    let file = File::create(path).expect("creating the zip");
    let mut zip = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    for (name, bytes) in files {
        zip.start_file(*name, options)
            .expect("starting a zip entry");
        zip.write_all(bytes).expect("writing a zip entry");
    }
    zip.finish().expect("finishing the zip");
}

fn tar_bytes(files: &[(&str, Vec<u8>)]) -> Vec<u8> {
    let mut builder = tar::Builder::new(Vec::new());
    for (name, bytes) in files {
        let mut header = tar::Header::new_gnu();
        header.set_size(u64::try_from(bytes.len()).expect("a small file"));
        header.set_mode(0o644);
        header.set_mtime(1_700_000_000);
        header.set_cksum();
        builder
            .append_data(&mut header, name, bytes.as_slice())
            .expect("appending a tar entry");
    }
    builder.into_inner().expect("finishing the tar")
}

fn gzip(bytes: &[u8]) -> Vec<u8> {
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::best());
    gz.write_all(bytes).expect("compressing");
    gz.finish().expect("finishing the gzip")
}

/// Each file under `dir` with its length.
fn files_of(dir: &Path) -> BTreeMap<String, u64> {
    let mut found = BTreeMap::new();
    for entry in walkdir(dir) {
        let len = std::fs::metadata(&entry).expect("reading a file").len();
        found.insert(entry.display().to_string(), len);
    }
    found
}

fn walkdir(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).expect("reading the folder") {
        let path = entry.expect("reading an entry").path();
        if path.is_dir() {
            out.extend(walkdir(&path));
        }
        out.push(path);
    }
    out
}

#[test]
fn a_zip_lists_its_entries() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let text = b"hello hello hello hello hello hello hello hello".repeat(20);
    write_zip(
        &dir.path().join("a.zip"),
        &[("docs/readme.txt", &text), ("b.bin", b"xy")],
    );

    let listing = entries(dir.path(), "a.zip").expect("a zip lists");

    assert!(!listing.truncated);
    let paths: Vec<&str> = listing.entries.iter().map(|e| e.path.as_str()).collect();
    assert_eq!(paths, ["docs/readme.txt", "b.bin"]);
    let readme = &listing.entries[0];
    assert_eq!(usize::try_from(readme.size).ok(), Some(text.len()));
    let packed = readme
        .compressed_size
        .expect("a zip entry has a compressed size");
    assert!(
        packed > 0 && packed < readme.size,
        "compressed {packed} of {}",
        readme.size
    );
    assert_eq!(listing.entries[1].size, 2);
}

#[test]
fn a_tar_gz_lists_its_entries() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let tar = tar_bytes(&[
        ("src/main.rs", b"fn main() {}".to_vec()),
        ("empty", Vec::new()),
    ]);
    std::fs::write(dir.path().join("a.tar.gz"), gzip(&tar)).expect("writing the tar.gz");
    std::fs::write(dir.path().join("b.tgz"), gzip(&tar)).expect("writing the tgz");

    for name in ["a.tar.gz", "b.tgz"] {
        let listing = entries(dir.path(), name).expect("a tar.gz lists");
        assert!(!listing.truncated);
        let got: Vec<(&str, u64)> = listing
            .entries
            .iter()
            .map(|e| (e.path.as_str(), e.size))
            .collect();
        assert_eq!(got, [("src/main.rs", 12), ("empty", 0)]);
        assert_eq!(listing.entries[0].modified, Some(1_700_000_000_000));
        assert_eq!(listing.entries[0].compressed_size, None);
    }
}

#[test]
fn a_long_archive_is_truncated() {
    assert_eq!(MAX_ENTRIES, 10_000);
    let dir = tempfile::tempdir().expect("a temp dir");
    let names: Vec<String> = (0..=MAX_ENTRIES).map(|i| format!("f{i}")).collect();
    let tar: Vec<(&str, Vec<u8>)> = names.iter().map(|n| (n.as_str(), Vec::new())).collect();
    std::fs::write(dir.path().join("long.tar"), tar_bytes(&tar)).expect("writing the tar");
    let zip: Vec<(&str, &[u8])> = names.iter().map(|n| (n.as_str(), &b""[..])).collect();
    write_zip(&dir.path().join("long.zip"), &zip);

    for name in ["long.tar", "long.zip"] {
        let listing = entries(dir.path(), name).expect("a long archive lists");
        assert_eq!(listing.entries.len(), MAX_ENTRIES, "{name}");
        assert!(listing.truncated, "{name}");
    }

    let exact: Vec<(&str, Vec<u8>)> = tar.into_iter().take(MAX_ENTRIES).collect();
    std::fs::write(dir.path().join("exact.tar"), tar_bytes(&exact)).expect("writing the tar");
    let listing = entries(dir.path(), "exact.tar").expect("an exact archive lists");
    assert_eq!(listing.entries.len(), MAX_ENTRIES);
    assert!(!listing.truncated);
}

#[test]
fn an_inflating_bomb_stops_at_the_cap() {
    assert_eq!(MAX_INFLATED_BYTES, 1 << 30);
    let dir = tempfile::tempdir().expect("a temp dir");
    let tar = tar_bytes(&[("zeros", vec![0; 4 << 20]), ("after", b"x".to_vec())]);
    let bomb = gzip(&tar);
    assert!(bomb.len() < 64 << 10, "the gzip is small: {}", bomb.len());
    std::fs::write(dir.path().join("bomb.tar.gz"), bomb).expect("writing the bomb");
    let caps = Caps {
        entries: MAX_ENTRIES,
        inflated: 1 << 20,
    };

    let listing = entries_with(dir.path(), "bomb.tar.gz", caps).expect("a bomb lists in part");

    assert!(listing.truncated);
    assert!(
        listing.entries.iter().all(|e| e.path != "after"),
        "{listing:?}"
    );
}

#[test]
fn a_listing_writes_nothing() {
    let top = tempfile::tempdir().expect("a temp dir");
    std::fs::create_dir(top.path().join("repo")).expect("creating the repo");
    let root = top.path().join("repo");
    let dir = &tempfile::TempDir::new_in(&root).expect("a folder in the repo");
    write_zip(
        &dir.path().join("a.zip"),
        &[("../escape.txt", b"x"), ("in/side.txt", b"y")],
    );
    let tar = tar_bytes(&[("in/side.txt", b"y".to_vec())]);
    std::fs::write(dir.path().join("a.tar.gz"), gzip(&tar)).expect("writing the tar.gz");
    std::fs::write(dir.path().join("a.tar"), &tar).expect("writing the tar");
    let before = files_of(dir.path());

    for name in ["a.zip", "a.tar.gz", "a.tar"] {
        let listing = entries(dir.path(), name).expect("an archive lists");
        assert!(!listing.entries.is_empty(), "{name}");
    }

    assert_eq!(files_of(dir.path()), before);
    let all: Vec<_> = walkdir(top.path());
    assert_eq!(all.len(), before.len() + 2, "{all:?}");
}

#[test]
fn a_file_that_is_no_archive_is_refused() {
    let dir = tempfile::tempdir().expect("a temp dir");
    std::fs::write(dir.path().join("a.zip"), b"not a zip at all").expect("writing");
    std::fs::write(dir.path().join("a.txt"), b"text").expect("writing");

    assert_eq!(entries(dir.path(), "a.zip"), Err(EntriesError::NotArchive));
    assert_eq!(entries(dir.path(), "a.txt"), Err(EntriesError::NotArchive));
    assert_eq!(entries(dir.path(), "none.zip"), Err(EntriesError::NotFound));
    assert_eq!(entries(dir.path(), "../a.zip"), Err(EntriesError::NotFound));
}

#[test]
fn file_entries_is_a_verb() {
    use crate::dispatch::{EffectClass, Verb};
    let verb = Verb::from_query("file.entries").expect("file.entries is a verb");
    assert_eq!(verb, Verb::FileEntries);
    assert!(Verb::ALL.contains(&verb));
    assert_eq!(verb.effect_class(), EffectClass::Observe);
}
