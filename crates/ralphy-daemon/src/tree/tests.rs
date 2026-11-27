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
        ImageType::Avif => b"\x00\x00\x00\x1cftypavif".to_vec(),
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
fn read_image_serves_what_the_extension_and_the_bytes_agree_on() {
    let prologue = "<?xml version=\"1.0\"?>
<!-- drawn by hand -->
               <!DOCTYPE svg PUBLIC \"-//W3C//DTD SVG 1.1//EN\" \"svg11.dtd\">
               <svg xmlns=\"http://www.w3.org/2000/svg\"><rect/></svg>";
    let bracket = "<!-- a > b <html> -->
<svg xmlns=\"http://www.w3.org/2000/svg\"/>";
    let by_kind = |kind: ImageType| (kind.media_type(), magic(kind));
    let text = |svg: &str| ("image/svg+xml", svg.as_bytes().to_vec());
    let rows = [
        ("png", "a.png", by_kind(ImageType::Png)),
        ("jpg", "a.jpg", by_kind(ImageType::Jpeg)),
        ("jpeg", "a.jpeg", by_kind(ImageType::Jpeg)),
        ("gif", "a.gif", by_kind(ImageType::Gif)),
        ("webp", "a.webp", by_kind(ImageType::Webp)),
        ("bmp", "a.bmp", by_kind(ImageType::Bmp)),
        ("ico", "a.ico", by_kind(ImageType::Icon)),
        ("svg", "a.svg", by_kind(ImageType::Svg)),
        ("upper case extension", "A.PNG", by_kind(ImageType::Png)),
        ("svg behind a prologue", "logo.svg", text(prologue)),
        ("svg after a comment with >", "note.svg", text(bracket)),
    ];
    let root = tempfile::tempdir().unwrap();
    for (name, file, (media_type, bytes)) in rows {
        fs::write(root.path().join(file), &bytes).unwrap();
        let want = Ok(Image { media_type, bytes });
        assert_eq!(read_image(root.path(), file), want, "{name}");
    }
}

#[test]
fn read_image_refuses_what_is_not_an_image() {
    // The extension proposes, the magic disposes (ADR-0049 §3).
    let big = {
        let mut big = magic(ImageType::Png);
        let size = usize::try_from(MAX_IMAGE_BYTES + 1).expect("4 MiB fits usize");
        big.resize(size, 0);
        big
    };
    let rows: [(&str, &str, Vec<u8>, ReadError); 10] = [
        (
            "html named png",
            "evil.png",
            b"<html><script>x</script>".to_vec(),
            ReadError::NotImage,
        ),
        (
            "png named svg",
            "mislabelled.svg",
            magic(ImageType::Png),
            ReadError::NotImage,
        ),
        (
            "text extension",
            "f.txt",
            b"hello".to_vec(),
            ReadError::NotImage,
        ),
        (
            "pdf is not an image",
            "d.pdf",
            b"%PDF-1.7".to_vec(),
            ReadError::NotImage,
        ),
        (
            "no extension",
            "LICENSE",
            b"x".to_vec(),
            ReadError::NotImage,
        ),
        (
            "svg root is html",
            "page.svg",
            b"<html><svg/></html>".to_vec(),
            ReadError::NotImage,
        ),
        (
            "unterminated comment",
            "open.svg",
            b"<!-- <svg/>".to_vec(),
            ReadError::NotImage,
        ),
        (
            "svg with invalid utf-8",
            "bin.svg",
            vec![0xff, 0xfe, 0x00],
            ReadError::NotImage,
        ),
        ("over the size limit", "big.png", big, ReadError::TooLarge),
        ("absent file", "absent.png", Vec::new(), ReadError::NotFound),
    ];
    let root = tempfile::tempdir().unwrap();
    for (name, file, bytes, want) in rows {
        if file != "absent.png" {
            fs::write(root.path().join(file), bytes).unwrap();
        }
        assert_eq!(read_image(root.path(), file), Err(want), "{name}");
    }
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
