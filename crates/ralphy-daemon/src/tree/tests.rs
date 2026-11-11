#[test]
fn read_error_keeps_its_text() {
    use super::ReadError;
    assert_eq!(ReadError::Binary.to_string(), "binary file");
    assert_eq!(ReadError::TooLarge.to_string(), "file too large");
    assert_eq!(ReadError::NotFound.to_string(), "file not found");
    assert_eq!(ReadError::NotImage.to_string(), "not an image");
}

/// The cap is the base64 size of the largest image plus 64 KiB, so a
/// 4 MiB paste and its envelope fit and nothing much larger does.
#[test]
fn max_command_bytes_is_derived_from_the_image_cap() {
    assert_eq!(
        MAX_COMMAND_BYTES,
        (4 * 1024 * 1024usize).div_ceil(3) * 4 + 65_536
    );
    assert_eq!(MAX_COMMAND_BYTES, 5_657_944);
    let max = usize::try_from(MAX_IMAGE_BYTES).expect("4 MiB fits usize");
    assert!(MAX_COMMAND_BYTES > max.div_ceil(3) * 4 + 4096);
}

use super::*;
use std::fs;

/// The smallest byte string that passes each type's magic check.
fn magic(kind: ImageType) -> Vec<u8> {
    match kind {
        ImageType::Png => b"\x89PNG\r\n\x1a\n".to_vec(),
        ImageType::Jpeg => b"\xff\xd8\xff\xe0".to_vec(),
        ImageType::Gif => b"GIF89a".to_vec(),
        ImageType::Webp => b"RIFF\x00\x00\x00\x00WEBPVP8 ".to_vec(),
        ImageType::Bmp => b"BM\x00\x00".to_vec(),
        ImageType::Icon => b"\x00\x00\x01\x00".to_vec(),
        ImageType::Svg => b"<svg xmlns=\"http://www.w3.org/2000/svg\"/>".to_vec(),
    }
}

#[test]
fn detect_raster_recognizes_each_raster_type_by_its_bytes_alone() {
    // ADR-0055 §2: the paste path has no extension, so the bytes dispose.
    for kind in [
        ImageType::Png,
        ImageType::Jpeg,
        ImageType::Gif,
        ImageType::Webp,
    ] {
        assert_eq!(ImageType::detect_raster(&magic(kind)), Some(kind));
    }
}

#[test]
fn detect_raster_refuses_everything_outside_the_raster_allowlist() {
    // SVG is script-bearing text, BMP/ICO are never what a browser pastes:
    // all three are on the READ allowlist and deliberately not on this one.
    for kind in [ImageType::Svg, ImageType::Bmp, ImageType::Icon] {
        assert_eq!(ImageType::detect_raster(&magic(kind)), None, "{kind:?}");
    }
    assert_eq!(ImageType::detect_raster(b""), None);
    assert_eq!(ImageType::detect_raster(b"<html><script>x</script>"), None);
    // A truncated signature is not a signature.
    assert_eq!(ImageType::detect_raster(b"\x89PNG"), None);
}

#[test]
fn extension_round_trips_through_from_extension() {
    for kind in [
        ImageType::Png,
        ImageType::Jpeg,
        ImageType::Gif,
        ImageType::Webp,
        ImageType::Bmp,
        ImageType::Icon,
        ImageType::Svg,
    ] {
        assert_eq!(ImageType::from_extension(kind.extension()), Some(kind));
    }
}

#[test]
fn list_filters_noise() {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("visible.txt"), b"x").unwrap();
    fs::create_dir(root.path().join("node_modules")).unwrap();
    fs::write(root.path().join("node_modules/x"), b"x").unwrap();
    let names: Vec<String> = list(root.path(), "")
        .unwrap()
        .into_iter()
        .map(|e| e.name)
        .collect();
    assert!(names.contains(&"visible.txt".to_owned()));
    assert!(!names.contains(&"node_modules".to_owned()));
}

#[test]
fn read_refuses_binary() {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("bin.dat"), [0x00, 0x01]).unwrap();
    assert_eq!(read(root.path(), "bin.dat"), Err(ReadError::Binary));
    // Latin-1 bytes are text now (the fallback page), not binary.
    fs::write(root.path().join("latin.txt"), b"\xA7").unwrap();
    assert_eq!(read(root.path(), "latin.txt"), Ok("§".to_owned()));
}

#[test]
fn refusal_reasons_are_the_wire_vocabulary() {
    // These four literals are the wire contract the workbench matches on;
    // the first three are shared with `ralphy blob read --format json`'s
    // HEAD side, and `not an image` is `read_image`'s alone (ADR-0049 §4).
    assert_eq!(ReadError::Binary.reason(), "binary");
    assert_eq!(ReadError::TooLarge.reason(), "too large");
    assert_eq!(ReadError::NotFound.reason(), "not found");
    assert_eq!(ReadError::NotImage.reason(), "not an image");
}

#[test]
fn read_refuses_oversized() {
    let root = tempfile::tempdir().unwrap();
    let big = vec![b'a'; usize::try_from(MAX_READ_BYTES + 1).expect("2 MiB fits usize")];
    fs::write(root.path().join("big.txt"), &big).unwrap();
    assert_eq!(read(root.path(), "big.txt"), Err(ReadError::TooLarge));
}

#[test]
fn read_returns_text() {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("f.txt"), b"hello world").unwrap();
    assert_eq!(read(root.path(), "f.txt"), Ok("hello world".to_owned()));
}

#[test]
fn read_accepts_large_utf8_split_at_window_boundary() {
    // A valid UTF-8 file whose byte at SNIFF_BYTES lands mid-multibyte-char
    // must NOT be misread as binary (the window UTF-8 check regression).
    let root = tempfile::tempdir().unwrap();
    // 'é' is 2 bytes; pad so a char straddles the 8 KiB boundary.
    let mut s = "a".repeat(textcodec::SNIFF_BYTES - 1);
    s.push('é');
    s.push_str(&"b".repeat(100));
    fs::write(root.path().join("big.txt"), s.as_bytes()).unwrap();
    assert_eq!(read(root.path(), "big.txt"), Ok(s));
}

#[test]
fn read_image_serves_every_allowlisted_type() {
    // The whole allowlist round-trips: extension → verified type → bytes.
    let root = tempfile::tempdir().unwrap();
    for (name, kind) in [
        ("a.png", ImageType::Png),
        ("a.jpg", ImageType::Jpeg),
        ("a.jpeg", ImageType::Jpeg),
        ("a.gif", ImageType::Gif),
        ("a.webp", ImageType::Webp),
        ("a.bmp", ImageType::Bmp),
        ("a.ico", ImageType::Icon),
        ("a.svg", ImageType::Svg),
    ] {
        let bytes = magic(kind);
        fs::write(root.path().join(name), &bytes).unwrap();
        assert_eq!(
            read_image(root.path(), name),
            Ok(Image {
                media_type: kind.media_type(),
                bytes
            }),
            "{name}"
        );
    }
}

#[test]
fn read_image_is_case_insensitive_on_the_extension() {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("A.PNG"), magic(ImageType::Png)).unwrap();
    assert_eq!(
        read_image(root.path(), "A.PNG").map(|i| i.media_type),
        Ok("image/png")
    );
}

#[test]
fn read_image_refuses_bytes_that_belie_the_extension() {
    // The core of ADR-0049 §3: the extension proposes, the magic disposes.
    // HTML named `.png` must never be served AS `image/png`.
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("evil.png"), b"<html><script>x</script>").unwrap();
    assert_eq!(
        read_image(root.path(), "evil.png"),
        Err(ReadError::NotImage)
    );
    // ...and the mismatch is symmetric: a real PNG under a `.svg` name.
    fs::write(root.path().join("mislabelled.svg"), magic(ImageType::Png)).unwrap();
    assert_eq!(
        read_image(root.path(), "mislabelled.svg"),
        Err(ReadError::NotImage)
    );
}

#[test]
fn read_image_refuses_a_non_image_extension() {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("f.txt"), b"hello").unwrap();
    assert_eq!(read_image(root.path(), "f.txt"), Err(ReadError::NotImage));
    // A PDF is binary and NOT an image: it stays refused (ADR-0049 §1).
    fs::write(root.path().join("d.pdf"), b"%PDF-1.7").unwrap();
    assert_eq!(read_image(root.path(), "d.pdf"), Err(ReadError::NotImage));
    // No extension at all.
    fs::write(root.path().join("LICENSE"), b"x").unwrap();
    assert_eq!(read_image(root.path(), "LICENSE"), Err(ReadError::NotImage));
}

#[test]
fn read_image_accepts_an_svg_behind_a_prologue() {
    // Declaration, comment and doctype may all precede the root element.
    let root = tempfile::tempdir().unwrap();
    let svg = "<?xml version=\"1.0\"?>\n<!-- drawn by hand -->\n\
               <!DOCTYPE svg PUBLIC \"-//W3C//DTD SVG 1.1//EN\" \"svg11.dtd\">\n\
               <svg xmlns=\"http://www.w3.org/2000/svg\"><rect/></svg>";
    fs::write(root.path().join("logo.svg"), svg).unwrap();
    assert_eq!(
        read_image(root.path(), "logo.svg").map(|i| i.media_type),
        Ok("image/svg+xml")
    );
}

#[test]
fn read_image_skips_a_comment_that_holds_a_closing_bracket() {
    let root = tempfile::tempdir().unwrap();
    let svg = "<!-- a > b <html> -->\n<svg xmlns=\"http://www.w3.org/2000/svg\"/>";
    fs::write(root.path().join("note.svg"), svg).unwrap();
    assert_eq!(
        read_image(root.path(), "note.svg").map(|i| i.media_type),
        Ok("image/svg+xml")
    );
}

#[test]
fn read_image_refuses_html_and_an_unterminated_prologue_as_svg() {
    let root = tempfile::tempdir().unwrap();
    // A document whose root is NOT <svg> — the XSS-shaped case.
    fs::write(root.path().join("page.svg"), b"<html><svg/></html>").unwrap();
    assert_eq!(
        read_image(root.path(), "page.svg"),
        Err(ReadError::NotImage)
    );
    // An unterminated comment must refuse, not scan forever.
    fs::write(root.path().join("open.svg"), b"<!-- <svg/>").unwrap();
    assert_eq!(
        read_image(root.path(), "open.svg"),
        Err(ReadError::NotImage)
    );
    // Invalid UTF-8 can never be an SVG.
    fs::write(root.path().join("bin.svg"), [0xff, 0xfe, 0x00]).unwrap();
    assert_eq!(read_image(root.path(), "bin.svg"), Err(ReadError::NotImage));
}

#[test]
fn read_image_refuses_oversized() {
    let root = tempfile::tempdir().unwrap();
    let mut big = magic(ImageType::Png);
    big.resize(
        usize::try_from(MAX_IMAGE_BYTES + 1).expect("4 MiB fits usize"),
        0,
    );
    fs::write(root.path().join("big.png"), &big).unwrap();
    assert_eq!(read_image(root.path(), "big.png"), Err(ReadError::TooLarge));
}

#[test]
fn read_image_masks_escape_as_not_found() {
    // Same ADR-0036 §5 masking as `read`: an out-of-root image read is
    // indistinguishable from a plain miss.
    // A real, valid image sits right outside the root, so only
    // confinement can answer `NotFound`.
    let outer = tempfile::tempdir().unwrap();
    let root = outer.path().join("root");
    fs::create_dir(&root).unwrap();
    fs::write(outer.path().join("secret.png"), magic(ImageType::Png)).unwrap();
    assert_eq!(read_image(&root, "../secret.png"), Err(ReadError::NotFound));
    assert_eq!(read_image(&root, "absent.png"), Err(ReadError::NotFound));
}

#[test]
fn read_and_read_image_refuse_each_others_inputs() {
    // The two readers do not overlap (ADR-0049 §1): an image is never a text
    // read, and a text file is never an image read.
    let root = tempfile::tempdir().unwrap();
    // The signature plus the IHDR length: a real PNG carries a NUL inside
    // its first 16 bytes, which is what makes it binary to `read` (the
    // eight signature bytes alone would decode as windows-1252 text).
    let mut png = magic(ImageType::Png);
    png.extend_from_slice(b"\0\0\0\rIHDR");
    fs::write(root.path().join("a.png"), png).unwrap();
    fs::write(root.path().join("a.txt"), b"hello").unwrap();
    assert_eq!(read(root.path(), "a.png"), Err(ReadError::Binary));
    assert_eq!(read_image(root.path(), "a.txt"), Err(ReadError::NotImage));
}

#[test]
fn read_masks_escape_as_not_found() {
    // Security: an out-of-root read must be indistinguishable from a plain
    // miss — `Escape` collapses to `NotFound`, never leaking existence.
    // A real text file sits right outside the root, so only confinement
    // can answer `NotFound`.
    let outer = tempfile::tempdir().unwrap();
    let root = outer.path().join("root");
    fs::create_dir(&root).unwrap();
    fs::write(outer.path().join("secret"), b"token").unwrap();
    assert_eq!(read(&root, "../secret"), Err(ReadError::NotFound));
}
