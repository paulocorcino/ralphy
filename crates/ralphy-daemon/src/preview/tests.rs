use super::*;

const PNG: &[u8] = b"\x89PNG\r\n\x1a\n";

/// What a read answers: the media type and the bytes, or `None` when refused.
type Served<'a> = Option<(&'a str, &'a [u8])>;

#[test]
fn a_token_opens_its_root_until_it_is_idle_or_closed() {
    let previews = Previews::default();
    let start = Instant::now();
    let token = previews.open(PathBuf::from("the-root"), start);
    assert_eq!(token.len(), 64, "32 random bytes as hex: {token}");

    let nearly = IDLE_EXPIRY
        .checked_sub(Duration::from_secs(1))
        .expect("the expiry is longer than a second");
    let almost = start + nearly;
    assert_eq!(
        previews.root_of(&token, almost),
        Some(PathBuf::from("the-root"))
    );
    // That read was a request: the idle time counts from it.
    let later = almost + nearly;
    assert_eq!(
        previews.root_of(&token, later),
        Some(PathBuf::from("the-root"))
    );
    assert_eq!(previews.root_of(&token, later + IDLE_EXPIRY), None);

    let second = previews.open(PathBuf::from("the-root"), start);
    assert_ne!(second, token);
    assert_eq!(previews.root_of("not-a-token", start), None);
    previews.close(&second);
    assert_eq!(previews.root_of(&second, start), None);
}

#[test]
fn the_oldest_token_makes_room_for_a_new_one() {
    let previews = Previews::default();
    let now = Instant::now();
    let tokens: Vec<String> = (0..=MAX_PREVIEWS)
        .map(|n| previews.open(PathBuf::from(format!("root-{n}")), now))
        .collect();
    assert_eq!(previews.root_of(&tokens[0], now), None);
    assert_eq!(
        previews.root_of(&tokens[1], now),
        Some(PathBuf::from("root-1"))
    );
    assert_eq!(
        previews.root_of(&tokens[MAX_PREVIEWS], now),
        Some(PathBuf::from(format!("root-{MAX_PREVIEWS}")))
    );
}

#[test]
fn only_the_closed_list_of_file_kinds_is_read() {
    let outer = tempfile::tempdir().expect("a temp dir is writable");
    let root = outer.path().join("repo");
    std::fs::create_dir_all(root.join("docs")).expect("the fixture folder is made");
    let write = |rel: &str, bytes: &[u8]| {
        std::fs::write(root.join(rel), bytes).expect("a fixture file is written");
    };
    write("docs/page.html", b"<p>page</p>");
    write("docs/OLD.HTM", b"<p>old</p>");
    write("docs/a.css", b"p{}");
    write("docs/a.js", b"1");
    write("docs/m.mjs", b"2");
    write("docs/d.json", b"{}");
    write("docs/pic.png", PNG);
    write("docs/fake.png", b"<html>not an image</html>");
    write(".env", b"SECRET=1");
    write("docs/notes.txt", b"text");
    write("docs/key.pem", b"key");
    write("docs/README", b"no extension");
    write("docs/f.woff", b"wOFF");
    write("docs/f.woff2", b"wOF2");
    write("docs/f.ttf", b"\x00\x01\x00\x00");
    write("docs/f.otf", b"OTTO");
    write("docs/f.eot", b"eot");
    let cap = usize::try_from(MAX_PREVIEW_BYTES).expect("16 MiB fits usize");
    write("docs/at-cap.js", &vec![b'1'; cap]);
    write("docs/over-cap.js", &vec![b'1'; cap + 1]);
    assert_eq!(
        read(&root, "docs/at-cap.js").map(|(_, b)| b.len()),
        Some(cap)
    );
    assert_eq!(read(&root, "docs/over-cap.js"), None);
    // The control of the confinement row: the file exists.
    std::fs::write(outer.path().join("out.html"), b"<p>outside</p>")
        .expect("the outside file is written");

    // [path, the media type and the bytes, or None when refused]
    let rows: [(&str, Served<'_>); 19] = [
        ("docs/page.html", Some(("text/html", b"<p>page</p>"))),
        ("docs/OLD.HTM", Some(("text/html", b"<p>old</p>"))),
        ("docs/a.css", Some(("text/css; charset=utf-8", b"p{}"))),
        ("docs/a.js", Some(("text/javascript; charset=utf-8", b"1"))),
        ("docs/m.mjs", Some(("text/javascript; charset=utf-8", b"2"))),
        ("docs/d.json", Some(("application/json", b"{}"))),
        ("docs/pic.png", Some(("image/png", PNG))),
        ("docs/f.woff", Some(("font/woff", b"wOFF"))),
        ("docs/f.woff2", Some(("font/woff2", b"wOF2"))),
        ("docs/f.ttf", Some(("font/ttf", b"\x00\x01\x00\x00"))),
        ("docs/f.otf", Some(("font/otf", b"OTTO"))),
        ("docs/f.eot", None),
        ("docs/fake.png", None),
        (".env", None),
        ("docs/notes.txt", None),
        ("docs/key.pem", None),
        ("docs/README", None),
        ("docs/missing.html", None),
        ("../out.html", None),
    ];
    for (rel, want) in rows {
        let got = read(&root, rel);
        let got = got.as_ref().map(|(kind, bytes)| (*kind, bytes.as_slice()));
        assert_eq!(got, want, "{rel}");
    }
}

#[test]
fn the_policy_names_only_the_route_and_takes_web_images_when_allowed() {
    let prefix = "http://127.0.0.1:7257/preview/abc/";
    assert_eq!(
        policy(prefix, false),
        "sandbox allow-scripts; default-src 'none'; \
         script-src 'unsafe-inline' 'unsafe-eval' http://127.0.0.1:7257/preview/abc/; \
         style-src 'unsafe-inline' http://127.0.0.1:7257/preview/abc/; \
         img-src data: http://127.0.0.1:7257/preview/abc/; \
         font-src http://127.0.0.1:7257/preview/abc/; \
         connect-src 'none'; form-action 'none'; base-uri 'none'; \
         frame-ancestors 'self'"
    );
    assert_eq!(
        policy(prefix, true),
        "sandbox allow-scripts; default-src 'none'; \
         script-src 'unsafe-inline' 'unsafe-eval' http://127.0.0.1:7257/preview/abc/; \
         style-src 'unsafe-inline' http://127.0.0.1:7257/preview/abc/; \
         img-src data: http://127.0.0.1:7257/preview/abc/ https:; \
         font-src http://127.0.0.1:7257/preview/abc/; \
         connect-src 'none'; form-action 'none'; base-uri 'none'; \
         frame-ancestors 'self'"
    );
}
