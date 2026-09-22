use super::*;
use encoding_rs::{SHIFT_JIS, WINDOWS_1252};

const FB: &Encoding = DEFAULT_FALLBACK;

fn utf16(text: &str, le: bool, bom: bool) -> Vec<u8> {
    encode(text, if le { UTF_16LE } else { UTF_16BE }, bom).unwrap()
}

#[test]
fn a_utf8_bom_is_stripped_and_reported() {
    let d = decode(b"\xEF\xBB\xBFol\xC3\xA1".to_vec(), None, FB).unwrap();
    assert_eq!((d.text.as_str(), d.encoding, d.bom), ("olá", UTF_8, true));
}

#[test]
fn utf16_with_a_bom_decodes_either_order() {
    for le in [true, false] {
        let d = decode(utf16("§ x", le, true), None, FB).unwrap();
        assert_eq!(d.text, "§ x");
        assert_eq!(d.encoding, if le { UTF_16LE } else { UTF_16BE });
        assert!(d.bom);
    }
}

#[test]
fn bare_utf16_is_read_by_its_nul_shape_not_refused_as_binary() {
    let d = decode(utf16("Out-File wrote this", true, false), None, FB).unwrap();
    assert_eq!(
        (d.text.as_str(), d.encoding, d.bom),
        ("Out-File wrote this", UTF_16LE, false)
    );
    let d = decode(utf16("big endian", false, false), None, FB).unwrap();
    assert_eq!(d.encoding, UTF_16BE);
}

#[test]
fn plain_utf8_is_utf8_without_a_bom() {
    let d = decode("caf\u{e9} — §".as_bytes().to_vec(), None, FB).unwrap();
    assert_eq!(
        (d.text.as_str(), d.encoding, d.bom),
        ("café — §", UTF_8, false)
    );
}

#[test]
fn invalid_utf8_without_a_nul_is_the_fallback_page() {
    // `§` (A7), `×` (D7) and `—` (97) as windows-1252 writes them.
    let d = decode(b"\xA76 240\xD7180 \x97 ok".to_vec(), None, FB).unwrap();
    assert_eq!(
        (d.text.as_str(), d.encoding, d.bom),
        ("§6 240×180 — ok", WINDOWS_1252, false)
    );
}

#[test]
fn a_hint_skips_detection_and_still_strips_its_own_bom() {
    let sjis = b"\x93\xFA\x96\x7B"; // 日本
    let d = decode(sjis.to_vec(), Some(SHIFT_JIS), FB).unwrap();
    assert_eq!((d.text.as_str(), d.encoding), ("日本", SHIFT_JIS));
    let d = decode(b"\xEF\xBB\xBFx".to_vec(), Some(UTF_8), FB).unwrap();
    assert_eq!((d.text.as_str(), d.bom), ("x", true));
    // A hint for another encoding leaves a foreign BOM in the text's bytes.
    let d = decode(b"\xEF\xBB\xBFx".to_vec(), Some(WINDOWS_1252), FB).unwrap();
    assert!(!d.bom);
    assert_eq!(d.text.chars().count(), 4);
}

#[test]
fn a_nul_that_is_not_utf16_shaped_is_binary() {
    assert!(decode(vec![0, 1, 2], None, FB).is_none());
    assert!(decode(b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec(), None, FB).is_none());
    assert!(decode(b"task\0task".to_vec(), None, FB).is_none());
    // Zeros that alternate but pair with control bytes are not UTF-16 text.
    assert!(decode(vec![0, 1], None, FB).is_none());
    assert!(decode(vec![0, 1, 0, 2, 0, 3], None, FB).is_none());
}

#[test]
fn a_file_of_only_high_bytes_and_no_nul_is_fallback_text_by_design() {
    // Rule 5 is NUL-only, as git's own heuristic is: the eight PNG signature
    // bytes alone have no NUL and read as windows-1252 text. A real PNG has
    // its IHDR length (`00 00 00 0D`) inside the first 16 bytes and is binary.
    assert!(decode(b"\x89PNG\r\n\x1a\n".to_vec(), None, FB).is_some());
    assert!(decode(b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec(), None, FB).is_none());
}

#[test]
fn a_multibyte_fallback_that_fails_is_binary_not_replacement_chars() {
    // A lone Shift_JIS lead byte at the end is an error for that decoder.
    assert!(decode(b"abc\x93".to_vec(), None, SHIFT_JIS).is_none());
    // The same bytes under the single-byte default always decode.
    assert!(decode(b"abc\x93".to_vec(), None, FB).is_some());
}

#[test]
fn utf8_split_at_the_window_boundary_is_still_utf8() {
    let mut bytes = vec![b'a'; SNIFF_BYTES - 1];
    bytes.extend_from_slice("é".as_bytes());
    let d = decode(bytes, None, FB).unwrap();
    assert_eq!(d.encoding, UTF_8);
    assert!(d.text.ends_with('é'));
}

#[test]
fn every_decoded_fixture_encodes_back_to_the_same_bytes() {
    let fixtures: Vec<Vec<u8>> = vec![
        b"\xEF\xBB\xBFol\xC3\xA1".to_vec(),
        utf16("§ x", true, true),
        utf16("§ x", false, true),
        utf16("bare le", true, false),
        "café — §".as_bytes().to_vec(),
        b"\xA76 240\xD7180 \x97 ok".to_vec(),
    ];
    for bytes in fixtures {
        let d = decode(bytes.clone(), None, FB).unwrap();
        assert_eq!(encode(&d.text, d.encoding, d.bom).unwrap(), bytes);
    }
}

#[test]
fn an_unrepresentable_char_refuses_with_its_index() {
    assert_eq!(encode("olá →", WINDOWS_1252, false), Err(4));
    assert_eq!(encode("→", WINDOWS_1252, false), Err(0));
    // The euro sign IS in windows-1252 (0x80), and the BOM flag is a no-op there.
    assert_eq!(encode("€", WINDOWS_1252, true), Ok(vec![0x80]));
}

#[test]
fn a_bom_is_prepended_only_where_one_exists() {
    assert_eq!(
        encode("x", UTF_8, true).unwrap(),
        vec![0xEF, 0xBB, 0xBF, b'x']
    );
    assert_eq!(
        encode("x", UTF_16LE, true).unwrap(),
        vec![0xFF, 0xFE, b'x', 0]
    );
    assert_eq!(
        encode("x", UTF_16BE, true).unwrap(),
        vec![0xFE, 0xFF, 0, b'x']
    );
    assert_eq!(encode("x", UTF_8, false).unwrap(), b"x".to_vec());
}

#[test]
fn labels_are_whatwg_and_the_replacement_pseudo_encoding_is_refused() {
    assert_eq!(label("windows-1252"), Some(WINDOWS_1252));
    assert_eq!(label(" Latin1 "), Some(WINDOWS_1252));
    assert_eq!(label("UTF-8"), Some(UTF_8));
    assert_eq!(label("utf-16le"), Some(UTF_16LE));
    assert_eq!(label("shift_jis"), Some(SHIFT_JIS));
    assert_eq!(label("iso-2022-kr"), None);
    assert_eq!(label("not-an-encoding"), None);
    assert_eq!(label(""), None);
}

#[test]
fn fallback_for_reads_files_encoding_and_defaults_on_every_failure() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(fallback_for(dir.path()), DEFAULT_FALLBACK);
    let ralphy = dir.path().join(".ralphy");
    std::fs::create_dir_all(&ralphy).unwrap();
    let settings = ralphy.join("settings.json");
    std::fs::write(&settings, "not json").unwrap();
    assert_eq!(fallback_for(dir.path()), DEFAULT_FALLBACK);
    std::fs::write(&settings, r#"{"files":{"encoding":"shift_jis"}}"#).unwrap();
    assert_eq!(fallback_for(dir.path()), SHIFT_JIS);
    std::fs::write(&settings, r#"{"files":{"encoding":"nope"}}"#).unwrap();
    assert_eq!(fallback_for(dir.path()), DEFAULT_FALLBACK);
    std::fs::write(&settings, r#"{"files":{}}"#).unwrap();
    assert_eq!(fallback_for(dir.path()), DEFAULT_FALLBACK);
}
