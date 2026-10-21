use super::*;

// --- `image.write`: the clipboard drop (ADR-0055) ---------------------------

/// The smallest byte string that passes the PNG magic check.
const PNG_BYTES: &[u8] = b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR";

fn b64(bytes: &[u8]) -> String {
    data_encoding::BASE64.encode(bytes)
}

/// The drop end to end: one reply on the id, zero spawns, the file under
/// `.ralphy/clipboard/` with the bytes intact (`.ralphy/` created on the way,
/// this repo never having run) — and the path the daemon chose
/// readable back through `file.image`, the D4 round-trip: whatever the console
/// pastes, the viewer can display.
#[tokio::test]
async fn image_write_lands_a_png_under_the_clipboard_dir() {
    let (url, slug, root) = serve_repo().await;
    let (replies, spawned) = round_trip(
        &url,
        30,
        "image.write",
        serde_json::json!({ "repo": slug, "base64": b64(PNG_BYTES) }),
    )
    .await;

    assert_eq!(replies.len(), 1, "exactly one reply on the id");
    assert_eq!(spawned, 0, "a Write must never spawn");
    assert_eq!(replies[0]["status"], "ok", "{:?}", replies[0]);
    let path = replies[0]["path"]
        .as_str()
        .expect("the daemon names the file");
    assert!(path.starts_with(".ralphy/clipboard/paste-"), "{path}");
    assert!(
        path.ends_with(".png"),
        "the extension follows the VERIFIED type: {path}"
    );
    assert_eq!(
        std::fs::read(root.join(path)).unwrap(),
        PNG_BYTES,
        "the bytes hit disk"
    );

    let (back, _) = round_trip(
        &url,
        31,
        "file.image",
        serde_json::json!({ "repo": slug, "path": path }),
    )
    .await;
    assert_eq!(back[0]["status"], "ok", "the viewer can read the drop back");
    assert_eq!(back[0]["mediaType"], "image/png");
}

/// Two pastes are two files: a drop never overwrites.
#[tokio::test]
async fn image_write_never_overwrites_an_earlier_drop() {
    let (url, slug, root) = serve_repo().await;
    let mut paths = Vec::new();
    for (id, payload) in [
        (32, b"\xff\xd8\xff\xe0one".as_slice()),
        (33, b"\xff\xd8\xff\xe0two"),
    ] {
        let (replies, _) = round_trip(
            &url,
            id,
            "image.write",
            serde_json::json!({ "repo": slug, "base64": b64(payload) }),
        )
        .await;
        assert_eq!(replies[0]["status"], "ok");
        paths.push(replies[0]["path"].as_str().unwrap().to_string());
    }
    assert_ne!(paths[0], paths[1]);
    assert!(paths.iter().all(|p| p.ends_with(".jpg")), "{paths:?}");
    assert_eq!(
        std::fs::read(root.join(&paths[0])).unwrap(),
        b"\xff\xd8\xff\xe0one"
    );
    assert_eq!(
        std::fs::read(root.join(&paths[1])).unwrap(),
        b"\xff\xd8\xff\xe0two"
    );
}

/// The magic-byte check in the write direction (ADR-0055 §2): HTML dressed as
/// an image is refused and nothing lands — the directory is not even created.
#[tokio::test]
async fn image_write_refuses_html_dressed_as_an_image() {
    let (url, slug, root) = serve_repo().await;
    let (replies, spawned) = round_trip(
        &url,
        34,
        "image.write",
        serde_json::json!({ "repo": slug, "base64": b64(b"<html><script>x</script>") }),
    )
    .await;

    assert_eq!(replies.len(), 1);
    assert_eq!(spawned, 0, "a refused write must never spawn");
    assert_eq!(replies[0]["status"], "error");
    assert_eq!(replies[0]["reason"], "not an image");
    assert!(replies[0].get("path").is_none(), "a refusal names no file");
    assert!(
        !root.join(".ralphy/clipboard").exists(),
        "nothing was written"
    );
}

/// The verb fixes its own target (ADR-0055 §1). A `path` is sent anyway, to
/// prove it is IGNORED rather than honoured: if it were read, the drop would
/// land on `a.txt` — or, with a traversal, outside the root.
#[tokio::test]
async fn image_write_takes_no_path_from_the_client() {
    let (url, slug, root) = serve_repo().await;
    for (id, path) in [(39, "a.txt"), (40, "../evil.png"), (41, ".ralphy/x.png")] {
        let (replies, _) = round_trip(
            &url,
            id,
            "image.write",
            serde_json::json!({ "repo": slug, "path": path, "base64": b64(PNG_BYTES) }),
        )
        .await;
        assert_eq!(replies[0]["status"], "ok", "{path}: {:?}", replies[0]);
        let landed = replies[0]["path"].as_str().unwrap();
        assert!(
            landed.starts_with(".ralphy/clipboard/"),
            "{path} -> {landed}"
        );
    }
    assert_eq!(
        std::fs::read_to_string(root.join("a.txt")).unwrap(),
        "x",
        "a.txt untouched"
    );
    assert!(!root.parent().unwrap().join("evil.png").exists());
    assert!(
        !root.join(".ralphy").join("x.png").exists(),
        "the client-named `.ralphy/x.png` was never honoured"
    );
}

// --- the transport cap (ADR-0072 D11) ----------------------------------------

/// A whole 4 MiB image, base64 and envelope included, is under the cap of
/// `/ws/command`: the paste gets its normal reply.
#[tokio::test]
async fn a_4_mib_image_paste_still_replies() {
    let (url, slug, root) = serve_repo().await;
    let mut png = PNG_BYTES.to_vec();
    png.resize(ralphy_daemon::tree::MAX_IMAGE_BYTES as usize, 0);
    let (replies, _) = round_trip(
        &url,
        40,
        "image.write",
        serde_json::json!({ "repo": slug, "base64": b64(&png) }),
    )
    .await;
    assert_eq!(replies.len(), 1, "{replies:?}");
    assert_eq!(replies[0]["status"], "ok", "{:?}", replies[0]);
    let path = replies[0]["path"].as_str().unwrap();
    assert_eq!(std::fs::read(root.join(path)).unwrap().len(), png.len());
}

/// A command over the cap is refused before it is parsed: the socket closes
/// and no reply comes. The command is well formed, so without the cap the
/// daemon would decode it and answer.
#[tokio::test]
async fn a_message_over_the_cap_closes_the_socket() {
    let (url, slug, _root) = serve_repo().await;
    let (mut ws, _resp) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("connecting to /ws/command");
    let big = protocol::encode(&Frame::Command(Command {
        id: 41,
        verb: "image.write".into(),
        payload: serde_json::json!({
            "repo": slug,
            "base64": "A".repeat(ralphy_daemon::tree::MAX_COMMAND_BYTES),
        }),
    }));
    assert!(big.len() > ralphy_daemon::tree::MAX_COMMAND_BYTES);
    // The server may close while the frame is still being written.
    let sent = ws.send(Message::Binary(big.into())).await;
    let closed = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            match ws.next().await {
                None | Some(Err(_)) | Some(Ok(Message::Close(_))) => return true,
                Some(Ok(Message::Binary(_))) => return false,
                Some(Ok(_)) => continue,
            }
        }
    })
    .await
    .expect("the socket must close within 10s");
    assert!(
        closed || sent.is_err(),
        "a message over the cap got a reply"
    );
}
