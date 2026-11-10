use super::*;

#[test]
fn extract_keeps_only_user_attachments_deduped_body_first() {
    let body = "See [diagnose.log](https://github.com/user-attachments/files/1/diagnose.log) \
                and https://evil.example/x.log for details.";
    let comments =
        vec!["re-pasting https://github.com/user-attachments/files/1/diagnose.log".to_owned()];
    let links = extract_user_attachment_links(body, &comments);
    assert_eq!(
        links,
        vec!["https://github.com/user-attachments/files/1/diagnose.log".to_owned()],
        "evil excluded, deduped, body-first"
    );
}

#[test]
fn classify_format_allowlists() {
    for f in ["a.log", "a.txt", "a.md", "a.diff", "a.patch"] {
        assert_eq!(classify_format(f), FormatClass::FreeText, "{f}");
    }
    for f in ["a.json", "a.yaml", "a.yml", "a.toml", "a.csv"] {
        assert_eq!(classify_format(f), FormatClass::Structured, "{f}");
    }
    for f in ["a.png", "a.jpg", "a.jpeg", "a.webp", "a.gif"] {
        assert_eq!(classify_format(f), FormatClass::Image, "{f}");
    }
    for f in ["a.exe", "a.zip", "foo"] {
        assert_eq!(classify_format(f), FormatClass::Denied, "{f}");
    }
}

#[test]
fn extensionless_asset_is_only_the_inline_upload_shape() {
    assert!(is_extensionless_asset(
        "https://github.com/user-attachments/assets/7a4d4bd2-c46f-48eb-b11a-9c53a0364b5e"
    ));
    // A named file under `/files/` classifies by extension as before.
    assert!(!is_extensionless_asset(
        "https://github.com/user-attachments/files/1/diagnose.log"
    ));
    // An asset that *does* carry an extension needs no sniffing.
    assert!(!is_extensionless_asset(
        "https://github.com/user-attachments/assets/abc.png"
    ));
}

#[test]
fn sniff_image_ext_accepts_only_real_image_headers() {
    assert_eq!(sniff_image_ext(b"\x89PNG\r\n\x1a\n\x00\x00"), Some("png"));
    assert_eq!(sniff_image_ext(&[0xFF, 0xD8, 0xFF, 0xE0]), Some("jpg"));
    assert_eq!(sniff_image_ext(b"GIF89a...."), Some("gif"));
    assert_eq!(
        sniff_image_ext(b"RIFF\x00\x00\x00\x00WEBPVP8 "),
        Some("webp")
    );
    // Deny-by-default: anything that is not one of the four is not an image.
    assert_eq!(sniff_image_ext(b"<!DOCTYPE html>"), None);
    assert_eq!(sniff_image_ext(b"PK\x03\x04"), None);
    assert_eq!(sniff_image_ext(b""), None);
    // A truncated RIFF header must not index out of bounds.
    assert_eq!(sniff_image_ext(b"RIFF\x00\x00"), None);
}

#[test]
fn login_html_guard() {
    assert!(looks_like_login_html(Some("text/html"), b"anything"));
    assert!(looks_like_login_html(None, b"<!DOCTYPE html>\n<html>"));
    assert!(looks_like_login_html(None, b"  <html lang=\"en\">"));
    assert!(!looks_like_login_html(None, b"{\"ok\":true}"));
}

#[test]
fn truncate_free_text_keeps_head_and_tail_with_marker() {
    let cap = 100;
    let mut input = vec![b'H'; cap / 2];
    input.extend(vec![b'M'; 100]); // middle to elide
    input.extend(vec![b'T'; cap / 2]);
    let out = truncate_free_text(&input, cap);
    let rendered = String::from_utf8_lossy(&out);
    assert!(out.len() < input.len());
    assert!(rendered.contains("[... "), "{rendered}");
    assert!(rendered.contains("bytes elided ...]"), "{rendered}");
    assert_eq!(&out[..8], &[b'H'; 8], "head survives");
    assert_eq!(&out[out.len() - 8..], &[b'T'; 8], "tail survives");
}

#[test]
fn truncate_free_text_passthrough_under_cap() {
    assert_eq!(truncate_free_text(b"short", 100), b"short");
}

#[test]
fn render_manifest_emits_heading_and_each_outcome() {
    let entries = vec![
        (
            "diagnose.log".to_owned(),
            AttachmentOutcome::Fetched {
                path: PathBuf::from("/tmp/t/133/diagnose.log"),
            },
        ),
        (
            "report.exe".to_owned(),
            AttachmentOutcome::NotFetched {
                reason: "denied format".to_owned(),
            },
        ),
        (
            "big.json".to_owned(),
            AttachmentOutcome::NotFetched {
                reason: "too large".to_owned(),
            },
        ),
    ];
    let m = render_manifest(133, &entries);
    assert!(m.contains("## Attachments (issue #133)"));
    assert!(
        m.contains("report.exe → not fetched (denied format)"),
        "{m}"
    );
    assert!(m.contains("big.json → not fetched (too large)"), "{m}");
    assert!(m.contains("diagnose.log → "));
    assert!(m.contains("(fetched)"));
}

#[test]
fn render_manifest_empty_is_blank() {
    assert_eq!(render_manifest(1, &[]), "");
}

#[test]
fn classify_payload_structured_over_cap_is_too_large() {
    let big = vec![b'{'; STRUCTURED_CAP + 1];
    assert_eq!(
        classify_payload(FormatClass::Structured, big),
        Err("too large")
    );
    // Under-cap structured passes through untouched.
    assert_eq!(
        classify_payload(FormatClass::Structured, b"{\"ok\":true}".to_vec()),
        Ok(b"{\"ok\":true}".to_vec())
    );
}

#[test]
fn classify_payload_image_over_cap_is_too_large() {
    let big = vec![0u8; IMAGE_CAP + 1];
    assert_eq!(classify_payload(FormatClass::Image, big), Err("too large"));
    // A tiny PNG stub passes through byte-for-byte (never truncated).
    let stub = vec![0x89u8, b'P', b'N'];
    assert_eq!(classify_payload(FormatClass::Image, stub.clone()), Ok(stub));
}

#[test]
fn gated_image_outcome_blocks_only_no_vision_images() {
    let no_vision = ImageCapability {
        accepts: false,
        adapter_name: "opencode",
    };
    assert_eq!(
        gated_image_outcome(FormatClass::Image, &no_vision),
        Some(AttachmentOutcome::NotFetched {
            reason: "opencode has no image input".to_owned(),
        })
    );
    let vision = ImageCapability {
        accepts: true,
        adapter_name: "claude",
    };
    assert_eq!(gated_image_outcome(FormatClass::Image, &vision), None);
    // A non-image class is never gated, even for a no-vision adapter.
    assert_eq!(gated_image_outcome(FormatClass::FreeText, &no_vision), None);
}

#[test]
fn classify_payload_free_text_over_cap_truncates() {
    let big = vec![b'x'; TEXT_CAP + 100];
    let out = classify_payload(FormatClass::FreeText, big.clone()).unwrap();
    assert!(out.len() < big.len(), "free-text over cap is truncated");
    assert!(String::from_utf8_lossy(&out).contains("bytes elided ...]"));
}

#[test]
fn count_cap_drops_the_last_after_ten() {
    let links: Vec<String> = (0..12).map(|i| format!("https://x/{i}")).collect();
    let capped = count_capped(&links);
    assert_eq!(capped.len(), 12);
    assert!(capped[..COUNT_CAP].iter().all(|(_, w)| *w), "first 10 kept");
    assert!(
        capped[COUNT_CAP..].iter().all(|(_, w)| !*w),
        "last two over cap"
    );
}

#[test]
fn filename_from_url_last_segment() {
    assert_eq!(
        filename_from_url("https://github.com/user-attachments/files/29722076/diagnose.log"),
        "diagnose.log"
    );
    assert_eq!(filename_from_url("https://x/y/a.json?sig=abc"), "a.json");
}

#[test]
fn filename_from_url_rejects_path_traversal() {
    // `\`-separated traversal (Windows) and drive prefixes must never yield a
    // name that could escape the per-issue temp dir on `dir.join(name)`.
    for url in [
        "https://github.com/user-attachments/files/1/..\\..\\evil.log",
        "https://github.com/user-attachments/files/1/C:\\x.log",
    ] {
        let name = filename_from_url(url);
        assert!(
            !name.contains('\\') && !name.contains('/') && !name.contains(':') && name != "..",
            "unsafe name survived: {name}"
        );
    }
    // Forward-slash traversal collapses to the final component (`evil.log`),
    // which cannot escape the dir; `..` alone falls back.
    assert_eq!(filename_from_url("https://x/y/../../evil.log"), "evil.log");
    assert_eq!(filename_from_url("https://x/y/.."), "attachment");
}

#[test]
fn login_html_guard_catches_bom_prefixed_page() {
    assert!(looks_like_login_html(
        None,
        "\u{feff}<!DOCTYPE html><html>".as_bytes()
    ));
}

#[test]
fn the_written_name_replaces_the_url_name_and_collects_an_image() {
    let mut name = "0a1b2c".to_owned();
    let mut images = Vec::new();
    let outcome = AttachmentOutcome::Fetched {
        path: PathBuf::from("run").join("1").join("0a1b2c.png"),
    };
    written_name_and_image(&outcome, &mut name, &mut images);
    assert_eq!(name, "0a1b2c.png");
    assert_eq!(
        images,
        vec![PathBuf::from("run").join("1").join("0a1b2c.png")]
    );
}

#[test]
fn a_fetched_text_file_takes_the_written_name_and_is_not_an_image() {
    let mut name = "log".to_owned();
    let mut images = Vec::new();
    let outcome = AttachmentOutcome::Fetched {
        path: PathBuf::from("run").join("diagnose.log"),
    };
    written_name_and_image(&outcome, &mut name, &mut images);
    assert_eq!(name, "diagnose.log");
    assert_eq!(images, Vec::<PathBuf>::new());
}
