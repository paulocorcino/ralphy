use super::*;
use std::fs;

#[test]
fn note_error_keeps_its_text() {
    assert_eq!(NoteError::NotNote.to_string(), "not a note");
    assert_eq!(NoteError::TooLarge.to_string(), "note too large");
    assert_eq!(NoteError::NotFound.to_string(), "note not found");
    assert_eq!(NoteError::Confined.to_string(), "path refused");
    assert_eq!(NoteError::Io.to_string(), "io error");
}

#[test]
fn a_container_round_trips_every_text_a_note_can_hold() {
    for text in [
        "",
        "# Title\n\n- [ ] one\n",
        "acentuação, 日本語, \u{1F5D2}\r\nCRLF kept\r\n",
        &"x".repeat(1024 * 1024),
    ] {
        assert_eq!(decode(&encode(text)).unwrap(), text);
    }
}

#[test]
fn a_container_carries_no_plaintext() {
    // The whole point of the format (ADR-0064 §3, "`strings` finds
    // nothing"). The case that broke it before the mask is a text deflate
    // cannot compress, which it emits as a STORED block — the markdown in
    // the clear behind a five-byte header.
    //
    // The stored-block ASSERTION is what keeps this honest: without it, a
    // compressor that shaved a byte off this exact input would move it
    // onto the Huffman path, where no needle could appear whatever the
    // mask did, and the test would stay green while proving nothing.
    // High-entropy and comfortably over miniz_oxide's 32-byte raw-block
    // floor, so the STORED path below is not a coin flip on the exact
    // length of a sentence (measured: the 45-byte version this replaces
    // cleared that floor by one byte).
    let text = "# 7pm

the secret is celery zq7#Kp!9Lv~Wm2@Xr4$Tn6%Yb8^Hc0&Jd1*Fg3(
";
    let bytes = encode(text);
    assert!(bytes.starts_with(MAGIC));
    let mut payload = bytes[HEADER..].to_vec();
    mask(&mut payload);
    assert_eq!(
        payload[0] & 0b110,
        0,
        "this fixture must take the STORED path — the one the mask exists for"
    );
    // EVERY window of the source, not one needle: a mask that covered the
    // first bytes and left the tail would pass a single-needle check.
    for needle in text.as_bytes().windows(6) {
        assert!(
            !bytes[HEADER..].windows(6).any(|w| w == needle),
            "{:?} is readable in the container",
            String::from_utf8_lossy(needle)
        );
    }
    // An empty note has no text to leak, but its SHAPE is pinned so a
    // future version cannot quietly start storing something beside it.
    assert_eq!(encode("").len(), HEADER + 2);
}

/// Containers built outside Rust: Python's `zlib` (raw deflate, `wbits=-15`)
/// at level 9 (a Huffman block) and level 0 (a STORED block), then masked.
/// Notes stay on disk and in git, so a note written by an earlier build must
/// read back whichever deflate backend decodes it (#443).
#[test]
fn a_container_from_another_deflate_encoder_reads_back() {
    let text = "# Groceries\n\n- [ ] celery\n- [ ] celery again, celery again\n";
    let huffman: &[u8] = &[
        0x52, 0x4e, 0x4f, 0x54, 0x01, 0x3b, 0x00, 0x00, 0x00, 0x01, 0x18, 0x3f, 0x7b, 0x59, 0x15,
        0x1f, 0x32, 0x98, 0x02, 0x62, 0xb2, 0x71, 0x88, 0x04, 0x97, 0x04, 0xc6, 0x1a, 0x1c, 0xdd,
        0x97, 0x18, 0x32, 0xf8, 0x0a, 0xae, 0x7c, 0xb7, 0xfc, 0x76, 0x79, 0xb4, 0xa7, 0xef, 0xa4,
        0x2b, 0x5a,
    ];
    let stored: &[u8] = &[
        0x52, 0x4e, 0x4f, 0x54, 0x01, 0x3b, 0x00, 0x00, 0x00, 0x53, 0x75, 0x4f, 0x90, 0x6c, 0x79,
        0x71, 0x58, 0x20, 0x21, 0x2c, 0x31, 0xe1, 0x33, 0x34, 0x6c, 0x58, 0x44, 0x62, 0x74, 0xc8,
        0x7a, 0x0c, 0x3f, 0x31, 0x2b, 0x23, 0x31, 0xe1, 0x23, 0x5b, 0x32, 0x72, 0x15, 0x6f, 0x09,
        0xb3, 0x39, 0x34, 0x73, 0x37, 0x3c, 0x36, 0x74, 0xf2, 0x3d, 0x30, 0x76, 0x3c, 0x62, 0x6f,
        0x37, 0xf6, 0x36, 0x34, 0x6d, 0x2b, 0x6e, 0x2e, 0x33, 0xf2, 0x33, 0x3f, 0x15,
    ];
    assert_eq!(decode(huffman).unwrap(), text);
    assert_eq!(decode(stored).unwrap(), text);
}

#[test]
fn a_tampered_header_is_not_a_note() {
    // The length is the integrity check: a stream that inflates to more or
    // to less than the header claims is refused, never half-read.
    let good = encode("# Title\n\nbody enough to compress, body enough\n");
    let mut short = good.clone();
    short[5] = 3;
    assert_eq!(decode(&short), Err(NoteError::NotNote));
    let mut long = good.clone();
    long[5] = long[5].wrapping_add(1);
    assert_eq!(decode(&long), Err(NoteError::NotNote));
    // An unmasked payload is not a stream at all.
    let mut naked = good.clone();
    mask(&mut naked[HEADER..]);
    assert_eq!(decode(&naked), Err(NoteError::NotNote));
}

#[test]
fn decode_refuses_what_is_not_this_version_of_the_container() {
    assert_eq!(decode(b""), Err(NoteError::NotNote));
    assert_eq!(decode(b"RNOT"), Err(NoteError::NotNote));
    assert_eq!(decode(b"# plain markdown"), Err(NoteError::NotNote));
    // Right magic, wrong version.
    let mut future = encode("hi");
    future[MAGIC.len()] = VERSION + 1;
    assert_eq!(decode(&future), Err(NoteError::NotNote));
    // Right header, truncated stream.
    let good = encode(&"hello ".repeat(500));
    assert_eq!(
        decode(&good[..good.len() - 10]),
        Err(NoteError::NotNote),
        "a truncated deflate stream is not a note"
    );
}

#[test]
fn decode_refuses_a_text_that_inflates_past_the_read_cap() {
    // A small file whose inflated text is over the cap: refused on OUTPUT
    // size, so the bomb is never held whole beyond one byte of slack.
    let bomb = encode(&"a".repeat(MAX_READ_BYTES as usize + 1));
    assert!((bomb.len() as u64) < MAX_READ_BYTES, "{}", bomb.len());
    assert_eq!(decode(&bomb), Err(NoteError::TooLarge));
}

#[test]
fn refusal_reasons_are_the_wire_vocabulary() {
    assert_eq!(NoteError::NotNote.reason(), "not a note");
    assert_eq!(NoteError::TooLarge.reason(), "too large");
    assert_eq!(NoteError::NotFound.reason(), "not found");
    assert_eq!(NoteError::Confined.reason(), "refused");
    assert_eq!(NoteError::Io.reason(), "io error");
}

#[test]
fn write_creates_the_default_dir_on_the_first_save_and_read_returns_the_text() {
    let root = tempfile::tempdir().unwrap();
    assert!(!root.path().join(DIR).exists());
    let rel = format!("{DIR}/groceries.note");
    write(root.path(), &rel, "# Groceries\n").unwrap();
    assert_eq!(read(root.path(), &rel).unwrap(), "# Groceries\n");
    // Overwriting is the autosave, not a conflict.
    write(root.path(), &rel, "# Groceries\n\n- celery\n").unwrap();
    assert_eq!(
        read(root.path(), &rel).unwrap(),
        "# Groceries\n\n- celery\n"
    );
    assert!(fs::read(root.path().join(&rel)).unwrap().starts_with(MAGIC));
}

#[test]
fn write_lands_in_a_directory_the_operator_chose_inside_the_checkout() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join("docs")).unwrap();
    write(root.path(), "docs/plan.note", "# Plan\n").unwrap();
    assert_eq!(read(root.path(), "docs/plan.note").unwrap(), "# Plan\n");
    // A directory that does not exist is a miss, not an invention: only
    // the default landing dir is created.
    assert_eq!(
        write(root.path(), "elsewhere/plan.note", "# Plan\n"),
        Err(NoteError::NotFound)
    );
    assert!(!root.path().join("elsewhere").exists());
}

#[test]
fn only_a_dot_note_path_is_a_note_in_either_direction() {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("readme.md"), "# hi").unwrap();
    assert_eq!(read(root.path(), "readme.md"), Err(NoteError::NotNote));
    assert_eq!(
        write(root.path(), ".ralphy/notes/x.md", "# hi"),
        Err(NoteError::NotNote)
    );
    assert_eq!(read(root.path(), ""), Err(NoteError::NotNote));
    // A `.note` whose bytes are something else refuses on the content.
    fs::write(root.path().join("fake.note"), "# not a container").unwrap();
    assert_eq!(read(root.path(), "fake.note"), Err(NoteError::NotNote));
}

#[test]
fn read_masks_a_missing_or_escaping_target_as_a_miss() {
    // A valid note sits right outside the root, so only confinement can
    // answer `NotFound` for the escape.
    let outer = tempfile::tempdir().unwrap();
    let root = outer.path().join("root");
    fs::create_dir(&root).unwrap();
    fs::write(outer.path().join("outside.note"), encode("# secret")).unwrap();
    assert_eq!(read(&root, "gone.note"), Err(NoteError::NotFound));
    assert_eq!(read(&root, "../outside.note"), Err(NoteError::NotFound));
}

#[test]
fn write_refuses_a_target_the_denylist_owns() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join(".ralphy")).unwrap();
    // The carve-out is exactly `.ralphy/notes/<name>.note`; a sibling of
    // the landing dir is still `.ralphy`, and still refused.
    assert_eq!(
        write(root.path(), ".ralphy/x.note", "# hi"),
        Err(NoteError::Confined)
    );
    assert_eq!(
        write(root.path(), ".ralphy/notes/deep/x.note", "# hi"),
        Err(NoteError::Confined)
    );
    assert_eq!(
        write(root.path(), "../outside.note", "# hi"),
        Err(NoteError::Confined)
    );
}

#[test]
fn a_written_note_can_say_when_it_was_written() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir_all(root.path().join(".ralphy/notes")).unwrap();
    let rel = ".ralphy/notes/a.note";
    // Nothing there yet has no time — not an error, and not a zero a card
    // would render as 1970.
    assert_eq!(modified(root.path(), rel), None);
    write(root.path(), rel, "# hi").unwrap();
    let at = modified(root.path(), rel).expect("a file that was just written has an mtime");
    // Sane rather than exact: a filesystem's clock is its own (FAT rounds
    // to two seconds), so this pins the ORDER OF MAGNITUDE — milliseconds
    // since the epoch, not seconds and not nanoseconds.
    assert!(at > 1_600_000_000_000, "not milliseconds: {at}");
    // The same refusals `read` makes, answered as "no time" rather than as
    // an error: a card asking the clock must never be told off for it.
    assert_eq!(modified(root.path(), "../outside.note"), None);
    assert_eq!(modified(root.path(), ".ralphy/notes/a.txt"), None);
}
