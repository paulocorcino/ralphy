//! Text has an encoding at the file verbs (ADR-0036, amendment 2026-09-22):
//! `file.read` answers `{content, encoding, bom}` after a deterministic
//! detection (BOM → bare UTF-16 → UTF-8 → the repo's `files.encoding`
//! fallback, default windows-1252) and honours an explicit `encoding` to
//! reopen with; `file.write` encodes back with the encoding it is given and
//! refuses `unencodable` rather than substituting; `tree.grep` decodes the
//! same way, so a windows-1252 file is a hit. Fixtures are byte literals seeded
//! into a temp repo — there is no `.gitattributes` protecting a checked-in
//! UTF-16 file from `autocrlf`. Mirrors `tests/observe_read.rs` and
//! `tests/workspace_write.rs`.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use ralphy_daemon::protocol::{self, Command, Frame};
use ralphy_daemon::{registry, router};
use tokio_tungstenite::tungstenite::Message;

/// `§6 240×180 — ok` as windows-1252 writes it.
const CP1252: &[u8] = b"\xA76 240\xD7180 \x97 ok";
/// `§ x` as UTF-16 LE with a BOM (what `Out-File` writes in Windows PowerShell 5).
const UTF16LE_BOM: &[u8] = b"\xFF\xFE\xA7\x00\x20\x00x\x00";
/// `bare le` as UTF-16 LE without a BOM.
const UTF16LE_BARE: &[u8] = b"b\x00a\x00r\x00e\x00 \x00l\x00e\x00";
/// `olá` as UTF-8 with a BOM.
const UTF8_BOM: &[u8] = b"\xEF\xBB\xBFol\xC3\xA1";
/// `日本` as Shift_JIS.
const SJIS: &[u8] = b"\x93\xFA\x96\x7B";

/// Bind a daemon over a temp repo seeded with one file per encoding; return the
/// `ws://…/ws/command` URL, the repo slug, and the leaked repo root.
async fn serve_repo() -> (String, String, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("cp1252.md"), CP1252).unwrap();
    std::fs::write(dir.path().join("utf16.txt"), UTF16LE_BOM).unwrap();
    std::fs::write(dir.path().join("bare16.txt"), UTF16LE_BARE).unwrap();
    std::fs::write(dir.path().join("bom8.txt"), UTF8_BOM).unwrap();
    std::fs::write(dir.path().join("sjis.txt"), SJIS).unwrap();
    std::fs::write(dir.path().join("plain.txt"), "caf\u{e9}").unwrap();
    std::fs::write(dir.path().join("bin.dat"), [0x00, 0x01, 0x02]).unwrap();
    let root = dir.path().to_path_buf();

    let registry_path = dir.path().join("repos.toml");
    let mut store = registry::RegistryStore::default();
    let slug = "owner/encoding";
    store.upsert(slug, &dir.path().to_string_lossy());
    registry::save_to(&store, &registry_path).unwrap();
    // Leak the tempdir so the registered repo outlives this fn (the daemon reads
    // it on every command); the OS reclaims it when the test process exits.
    std::mem::forget(dir);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let (_tx, rx) = tokio::sync::watch::channel(false);
    let app = router(
        None,
        registry_path,
        std::path::PathBuf::from("does-not-exist"),
        ralphy_daemon::StorePaths::default(),
        Instant::now(),
        rx,
        ralphy_daemon::auth::AuthState::localhost(),
    );
    std::mem::forget(_tx);
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (
        format!("ws://127.0.0.1:{port}/ws/command"),
        slug.to_string(),
        root,
    )
}

/// Send one `Command` and return the single reply on `id`.
async fn ask(url: &str, id: u64, verb: &str, payload: serde_json::Value) -> serde_json::Value {
    let (mut ws, _resp) = tokio_tungstenite::connect_async(url)
        .await
        .expect("connecting to /ws/command");
    ws.send(Message::Binary(protocol::encode(&Frame::Command(
        Command {
            id,
            verb: verb.to_string(),
            payload,
        },
    ))))
    .await
    .unwrap();

    tokio::time::timeout(Duration::from_secs(10), async {
        let mut replies = Vec::new();
        while let Some(msg) = ws.next().await {
            let bytes = match msg {
                Ok(Message::Binary(b)) => b,
                Ok(Message::Close(_)) | Err(_) => break,
                _ => continue,
            };
            if let Ok(Frame::Command(cmd)) = protocol::decode(&bytes) {
                if cmd.id == id {
                    replies.push(cmd.payload);
                }
            }
        }
        assert_eq!(replies.len(), 1, "exactly one reply on the id: {replies:?}");
        replies.remove(0)
    })
    .await
    .expect("the reply must arrive and the socket close within 10s")
}

async fn read(url: &str, slug: &str, path: &str) -> serde_json::Value {
    ask(
        url,
        1,
        "file.read",
        serde_json::json!({ "repo": slug, "path": path }),
    )
    .await
}

#[tokio::test]
async fn file_read_declares_the_encoding_it_detected() {
    let (url, slug, _) = serve_repo().await;
    let cases: [(&str, &str, &str, bool); 5] = [
        ("cp1252.md", "§6 240×180 — ok", "windows-1252", false),
        ("utf16.txt", "§ x", "UTF-16LE", true),
        ("bare16.txt", "bare le", "UTF-16LE", false),
        ("bom8.txt", "olá", "UTF-8", true),
        ("plain.txt", "café", "UTF-8", false),
    ];
    for (path, text, encoding, bom) in cases {
        let reply = read(&url, &slug, path).await;
        assert_eq!(reply["status"], "ok", "{path}: {reply}");
        assert_eq!(reply["content"], text, "{path}");
        assert_eq!(reply["encoding"], encoding, "{path}");
        assert_eq!(reply["bom"], bom, "{path}");
    }
}

#[tokio::test]
async fn file_read_still_refuses_binary_and_an_unknown_label() {
    let (url, slug, _) = serve_repo().await;
    let reply = read(&url, &slug, "bin.dat").await;
    assert_eq!(reply["status"], "error");
    assert_eq!(reply["reason"], "binary");

    let reply = ask(
        &url,
        2,
        "file.read",
        serde_json::json!({ "repo": slug, "path": "plain.txt", "encoding": "klingon" }),
    )
    .await;
    assert_eq!(reply["status"], "error");
    assert_eq!(reply["reason"], "unknown encoding");
}

#[tokio::test]
async fn file_read_reopens_with_the_encoding_the_client_names() {
    let (url, slug, _) = serve_repo().await;
    // Detection reads Shift_JIS bytes as the fallback page; the reopen names it.
    let detected = read(&url, &slug, "sjis.txt").await;
    assert_eq!(detected["encoding"], "windows-1252");
    let reply = ask(
        &url,
        3,
        "file.read",
        serde_json::json!({ "repo": slug, "path": "sjis.txt", "encoding": "shift_jis" }),
    )
    .await;
    assert_eq!(reply["status"], "ok", "{reply}");
    assert_eq!(reply["content"], "日本");
    assert_eq!(reply["encoding"], "Shift_JIS");
    assert_eq!(reply["bom"], false);
}

#[tokio::test]
async fn the_repos_files_encoding_is_the_fallback() {
    let (url, slug, root) = serve_repo().await;
    std::fs::create_dir_all(root.join(".ralphy")).unwrap();
    std::fs::write(
        root.join(".ralphy/settings.json"),
        r#"{ "files": { "encoding": "shift_jis" } }"#,
    )
    .unwrap();
    let reply = read(&url, &slug, "sjis.txt").await;
    assert_eq!(reply["content"], "日本", "{reply}");
    assert_eq!(reply["encoding"], "Shift_JIS");
    // UTF-8 and the BOM'd files are recognised before the fallback applies.
    assert_eq!(read(&url, &slug, "plain.txt").await["encoding"], "UTF-8");
    assert_eq!(read(&url, &slug, "utf16.txt").await["encoding"], "UTF-16LE");
}

#[tokio::test]
async fn tree_grep_finds_text_in_a_windows_1252_file() {
    let (url, slug, _) = serve_repo().await;
    let reply = ask(
        &url,
        4,
        "tree.grep",
        serde_json::json!({ "repo": slug, "query": "240×180" }),
    )
    .await;
    assert_eq!(reply["status"], "ok", "{reply}");
    let hits: Vec<&str> = reply["hits"]
        .as_array()
        .expect("hits array")
        .iter()
        .map(|h| h["path"].as_str().unwrap())
        .collect();
    assert_eq!(hits, vec!["cp1252.md"], "{reply}");
}

async fn write(
    url: &str,
    slug: &str,
    path: &str,
    content: &str,
    encoding: Option<&str>,
    bom: bool,
) -> serde_json::Value {
    let mut payload =
        serde_json::json!({ "repo": slug, "path": path, "content": content, "bom": bom });
    if let Some(enc) = encoding {
        payload["encoding"] = serde_json::json!(enc);
    }
    ask(url, 9, "file.write", payload).await
}

/// `(path, text, encoding param, bom param, expected bytes)`.
type WriteCase = (
    &'static str,
    &'static str,
    Option<&'static str>,
    bool,
    &'static [u8],
);

#[tokio::test]
async fn file_write_encodes_with_the_encoding_it_is_given() {
    let (url, slug, root) = serve_repo().await;
    let cases: [WriteCase; 5] = [
        ("w1.txt", "\u{a7}", Some("windows-1252"), false, b"\xA7"),
        (
            "w2.txt",
            "\u{a7}",
            Some("utf-8"),
            true,
            b"\xEF\xBB\xBF\xC2\xA7",
        ),
        (
            "w3.txt",
            "\u{a7}",
            Some("utf-16le"),
            true,
            b"\xFF\xFE\xA7\x00",
        ),
        ("w4.txt", "\u{65e5}\u{672c}", Some("shift_jis"), false, SJIS),
        // No `encoding` is the pre-amendment contract: UTF-8 (and a client
        // from before the amendment never sends `bom`).
        ("w5.txt", "\u{a7}", None, false, b"\xC2\xA7"),
    ];
    for (path, text, encoding, bom, bytes) in cases {
        let reply = write(&url, &slug, path, text, encoding, bom).await;
        assert_eq!(reply["status"], "ok", "{path}: {reply}");
        assert_eq!(std::fs::read(root.join(path)).unwrap(), bytes, "{path}");
    }
}

#[tokio::test]
async fn file_write_refuses_an_unrepresentable_char_and_writes_nothing() {
    let (url, slug, root) = serve_repo().await;
    let reply = write(
        &url,
        &slug,
        "cp1252.md",
        "ol\u{e1} \u{2192}",
        Some("windows-1252"),
        false,
    )
    .await;
    assert_eq!(reply["status"], "error", "{reply}");
    assert_eq!(reply["reason"], "unencodable");
    assert_eq!(reply["char_index"], 4);
    assert_eq!(
        std::fs::read(root.join("cp1252.md")).unwrap(),
        CP1252,
        "untouched"
    );

    let reply = write(&url, &slug, "cp1252.md", "x", Some("klingon"), false).await;
    assert_eq!(reply["reason"], "unknown encoding");
    assert_eq!(
        std::fs::read(root.join("cp1252.md")).unwrap(),
        CP1252,
        "untouched"
    );
}

#[tokio::test]
async fn every_read_writes_back_byte_for_byte() {
    let (url, slug, root) = serve_repo().await;
    for path in [
        "cp1252.md",
        "utf16.txt",
        "bare16.txt",
        "bom8.txt",
        "plain.txt",
    ] {
        let before = std::fs::read(root.join(path)).unwrap();
        let read = read(&url, &slug, path).await;
        let reply = write(
            &url,
            &slug,
            path,
            read["content"].as_str().unwrap(),
            read["encoding"].as_str(),
            read["bom"].as_bool().unwrap(),
        )
        .await;
        assert_eq!(reply["status"], "ok", "{path}: {reply}");
        assert_eq!(std::fs::read(root.join(path)).unwrap(), before, "{path}");
    }
}
